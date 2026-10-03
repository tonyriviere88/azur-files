1//! What is in the file the keyboard is on, read off the UI thread.
2//!
3//! The panel that shows it is [`crate::ui::preview`]; this is the half that decides **what a
4//! file is** and then **goes and gets it**. Three kinds, and they have nothing in common except
5//! that answering takes long enough that the window must not wait for it:
6//!
7//! - **A picture.** Decoded by `image`, or rasterised by `resvg` where it is vector art, and
8//!   handed over as pixels for the UI thread to upload. A 40-megapixel photograph takes a
9//!   quarter of a second to decode and 160 MB to hold, so [`CAP`] is not optional.
10//! - **Text.** Read up to [`TEXT_CAP`], and only if it really is text — a preview panel must
11//!   never paint a megabyte of `\0` into a wrapped paragraph.
12//! - **A binary.** [`crate::pe`]'s dependency walk, which is the interesting thing a `.dll`
13//!   has inside it.
14//!
15//! # One service, one token, one answer
16//!
17//! Every kind goes through [`Previews`] and comes back as a [`Payload`] tagged with the token
18//! that asked for it, so the panel has one thing to poll and one rule for staleness: an answer
19//! to a token nobody holds any more is dropped. That is the same shape as
20//! [`crate::shell::links`] and [`crate::loader`], for the same reason — and it is what lets the
21//! panel debounce the selection in one place rather than three.
22//!
23//! Detached threads rather than a pool. A preview is one file at a time and the request rate is
24//! bounded by a human moving a selection; there is nothing to queue, and there is nothing useful
25//! to do with a decode that is still running when the window closes.
26
27use std::path::Path;
28use std::sync::mpsc::{channel, Receiver, Sender};
29use std::sync::Arc;
30
31/// What a file will be shown as.
32#[derive(Clone, Copy, PartialEq, Eq, Debug)]
33pub enum Kind {
34    /// A raster image, or vector art to be rasterised.
35    Picture,
36    /// Something to read.
37    Text,
38    /// A Windows binary: what it imports, and from where.
39    Binary,
40    /// No extension, or one nothing here has heard of. **Decided on the worker** by looking at
41    /// the first few kilobytes, because "is this text?" is a question about contents and the
42    /// answer for `README`, `LICENSE`, `Makefile` and `.gitignore` is yes.
43    Unknown,
44}
45
46/// Every extension shown as a picture.
47///
48/// `image`'s pure-Rust codecs plus `resvg`'s, and nothing that would need a C library. `svgz` is
49/// not here: it is gzip, `resvg`'s decompression is behind the feature this build leaves off, and
50/// a compressed SVG is rare enough not to be worth a second decompressor.
51const PICTURES: [&str; 21] = [
52    "png", "jpg", "jpeg", "jfif", "gif", "bmp", "dib", "ico", "cur", "tif", "tiff", "webp", "svg",
53    "tga", "dds", "hdr", "qoi", "ff", "pbm", "pgm", "ppm",
54];
55
56/// Every extension read as text **where the columns mean something**: source, configuration,
57/// data, logs, diffs. Shown in the monospace role.
58///
59/// Long on purpose, and it is still not the rule — see [`Kind::Unknown`], which is what catches
60/// the ones that matter and cannot be listed.
61const CODE: [&str; 59] = [
62    "log", "csv", "tsv", "json", "jsonc", "yaml", "yml", "toml", "ini", "cfg", "conf",
63    "properties", "env", "xml", "xsd", "xsl", "svgz", "html", "htm", "css", "scss", "less", "js",
64    "mjs", "cjs", "ts", "tsx", "jsx", "rs", "c", "h", "cc", "cpp", "cxx", "hpp", "hxx", "cs",
65    "java", "kt", "py", "rb", "go", "php", "pl", "lua", "sh", "bash", "zsh", "ps1", "psm1", "bat",
66    "cmd", "sql", "diff", "patch", "gitignore", "gitattributes", "editorconfig", "cmake",
67];
68
69/// And every extension read as text where it does **not**: prose, shown in the body role.
70///
71/// The line between the two lists is a question with an answer, not a matter of taste: does moving
72/// a character sideways change what the file means? In a log, a table, a diff or any source file it
73/// does — a column that no longer lines up is information lost — so those get the monospace role
74/// even though it is the less comfortable one to read a paragraph in. Markdown and a `.txt` are
75/// paragraphs, and paragraphs are what the proportional face is for.
76const PROSE: [&str; 4] = ["txt", "md", "markdown", "rst"];
77
78/// Names with no extension that are prose all the same.
79///
80/// A file with nothing to go on is monospaced, because most of what has no extension in a source
81/// folder is a build or configuration file where the columns matter — `Makefile`, `Dockerfile`,
82/// `.npmrc`. These are the exceptions, and they are exceptions worth listing: a `README` set in a
83/// monospace face is the one file in the folder somebody is going to sit and read.
84const PROSE_NAMES: [&str; 8] = [
85    "readme",
86    "license",
87    "licence",
88    "copying",
89    "notice",
90    "changelog",
91    "authors",
92    "contributing",
93];
94
95/// Whether a file read as text has columns that mean something, and so wants the monospace role.
96///
97/// Asked of the *name* rather than carried on [`Kind`], because a sniffed file has no extension to
98/// classify and the answer for it comes from the same place: what it is called.
99pub fn is_code(name: &str, ext: &str) -> bool {
100    let is = |list: &[&str], what: &str| list.iter().any(|known| what.eq_ignore_ascii_case(known));
101    if is(&PROSE, ext) {
102        return false;
103    }
104    if ext.is_empty() && is(&PROSE_NAMES, name) {
105        return false;
106    }
107    true
108}
109
110/// What a file is, from its name alone. `None` for something with no preview at all.
111///
112/// A folder is not previewed: what a folder contains is what the listing beside the panel is
113/// already showing, and a second copy of it would be the same answer twice.
114pub fn kind_of(name: &str, ext: &str, is_dir: bool) -> Option<Kind> {
115    if is_dir {
116        return None;
117    }
118    let is = |list: &[&str]| list.iter().any(|known| ext.eq_ignore_ascii_case(known));
119    if is(&PICTURES) {
120        Some(Kind::Picture)
121    } else if is(&CODE) || is(&PROSE) {
122        Some(Kind::Text)
123    } else if crate::pe::is_image(ext) {
124        Some(Kind::Binary)
125    } else if ext.is_empty() || name.starts_with('.') {
126        // `README`, `LICENSE`, `Makefile`, `.gitignore`, `.npmrc`. A leading dot makes a dotfile
127        // rather than an extension, so `Dir::ext` is empty for those anyway — the second test is
128        // for `.gitignore`-style names whose *extension* is a word this list does happen to know.
129        Some(Kind::Unknown)
130    } else {
131        None
132    }
133}
134
135/// The most pixels a decoded picture may hold.
136///
137/// Four megapixels, which is 16 MB as RGBA. Not a limit on what can be *opened* — anything
138/// larger is scaled down to fit inside it and says so on the canvas — but on what a preview is
139/// allowed to cost. A phone photograph is 12 Mpx and a scanned drawing can be 200; holding one of
140/// those at full size to show it in a 400-point panel would be most of the memory this program
141/// uses on a thing nobody asked to keep.
142///
143/// The panel is at most a few hundred points wide, so 2048 on the long edge still leaves room to
144/// zoom several times past fit before the softness shows.
145pub const CAP: u32 = 2048;
146
147/// How much of a text file is read.
148///
149/// A megabyte, which is about fifteen thousand lines. Past that a preview is not what you want —
150/// and egui lays out the whole galley whether or not it is on screen, so a 200 MB log would be a
151/// frozen window rather than a slow one. Truncation is reported.
152pub const TEXT_CAP: usize = 1 << 20;
153
154/// How much is looked at before deciding an unknown file is text.
155const SNIFF: usize = 4096;
156
157/// The most pixels this will *decode*, as opposed to hold.
158///
159/// A different and larger bound than [`CAP`], and it has to exist separately: `image` has no
160/// streaming resize, so scaling something down to the cap means decoding all of it first. Forty
161/// megapixels is 160 MB while that is happening, which is a camera's full output and a great deal
162/// more than any preview needs. Past it the panel says how large the thing is instead, which is
163/// more useful than a window that stops for two seconds and then shows a thumbnail.
164const DECODE_MAX: u64 = 40_000_000;
165
166/// A decoded picture, waiting to be uploaded.
167pub struct Picture {
168    pub pixels: egui::ColorImage,
169    /// What it is on disk, which is what the panel reports — `pixels` may be smaller.
170    pub natural: [u32; 2],
171    /// It was larger than [`CAP`] and has been scaled down.
172    pub scaled: bool,
173    /// Vector art, rasterised at [`CAP`] rather than decoded at a natural size.
174    pub vector: bool,
175}
176
177/// A file read as text.
178pub struct Text {
179    pub body: String,
180    /// It is longer than [`TEXT_CAP`] and this is the front of it.
181    pub truncated: bool,
182    /// Its columns mean something, so it wants the monospace role. See [`is_code`].
183    pub code: bool,
184}
185
186/// Two pictures, and where they differ.
187pub struct Diff {
188    pub a: Picture,
189    pub b: Picture,
190    /// **Where they differ, as a mask**: white, with the alpha carrying how much. Transparent
191    /// wherever the two agree.
192    ///
193    /// A mask and not a coloured image, deliberately. What colour "different" is painted in is a
194    /// decision for [`crate::ui::preview`], where every other colour in this program is decided —
195    /// so what comes off the worker is a measurement and the panel tints it.
196    pub mask: Picture,
197    /// The share of pixels that differ at all, 0..1. The number you actually want: "they are the
198    /// same file" and "0.02% of it moved" are different answers and a picture shows neither.
199    pub differing: f32,
200}
201
202/// What came back.
203pub enum Payload {
204    Picture(Box<Picture>),
205    /// Two pictures compared. Boxed: three decoded images is up to 48 MB, and an enum is as large
206    /// as its largest variant everywhere it is passed.
207    Diff(Box<Diff>),
208    Text(Text),
209    Binary(Arc<crate::pe::Graph>),
210    /// Nothing to show, and why — short enough to put in the middle of the panel.
211    Failed(String),
212}
213
214/// What a panel has asked for: one file, or two to be compared.
215#[derive(Clone, PartialEq, Eq, Debug)]
216pub enum Ask {
217    One(std::path::PathBuf, Kind),
218    /// Two pictures, in the order they appear in the listing.
219    Pair(std::path::PathBuf, std::path::PathBuf),
220}
221
222impl Ask {
223    /// The file the panel is about, for anything that needs one path — the title's folder, a
224    /// staleness test.
225    pub fn first(&self) -> &Path {
226        match self {
227            Self::One(path, _) => path,
228            Self::Pair(a, _) => a,
229        }
230    }
231
232    /// What the panel's bar calls it.
233    pub fn title(&self) -> String {
234        let name = |path: &Path| {
235            path.file_name()
236                .map(|n| n.to_string_lossy().into_owned())
237                .unwrap_or_default()
238        };
239        match self {
240            Self::One(path, _) => name(path),
241            // The two names, and a mark that says they are being compared rather than listed.
242            Self::Pair(a, b) => format!("{} \u{2194} {}", name(a), name(b)),
243        }
244    }
245}
246
247pub struct Loaded {
248    pub token: u64,
249    pub payload: Payload,
250}
251
252/// The preview-reading service. One per application.
253pub struct Previews {
254    answers: Receiver<Loaded>,
255    /// Kept so each request can be given a live channel to answer on.
256    replies: Sender<Loaded>,
257    next: u64,
258    ctx: egui::Context,
259}
260
261impl Previews {
262    pub fn new(ctx: &egui::Context) -> Self {
263        let (replies, answers) = channel();
264        Self {
265            answers,
266            replies,
267            next: 1,
268            ctx: ctx.clone(),
269        }
270    }
271
272    /// Go and get it. The returned token identifies the answer.
273    pub fn request(&mut self, ask: &Ask) -> u64 {
274        let token = self.next;
275        self.next += 1;
276        let ask = ask.clone();
277        let replies = self.replies.clone();
278        let ctx = self.ctx.clone();
279        let spawned = std::thread::Builder::new()
280            .name("preview".to_owned())
281            .spawn(move || {
282                crate::fs::scan::silence_device_dialogs();
283                let payload = read(&ask);
284                if replies.send(Loaded { token, payload }).is_ok() {
285                    ctx.request_repaint();
286                }
287            });
288        // A machine that will not give us a thread leaves the panel waiting for a token that
289        // never arrives, which the next request clears. Nothing else is affected.
290        let _ = spawned;
291        token
292    }
293
294    /// Everything that has come back since the last call.
295    pub fn drain(&self) -> impl Iterator<Item = Loaded> + '_ {
296        self.answers.try_iter()
297    }
298}
299
300/// Do the reading. On a worker, always.
301fn read(ask: &Ask) -> Payload {
302    let (path, kind) = match ask {
303        Ask::One(path, kind) => (path.as_path(), *kind),
304        Ask::Pair(a, b) => return compare(a, b),
305    };
306    match kind {
307        Kind::Picture => picture(path),
308        Kind::Text => text(path, code_of(path)),
309        Kind::Binary => {
310            let graph = crate::pe::walk(path, crate::pe::BUDGET, crate::pe::PATIENCE);
311            // A root that is not a binary has no tree to draw, and one lonely row marked
312            // unreadable is a poor way to answer a question. The reason goes in the middle of
313            // the panel instead — which is what happens to a 16-bit `.exe`, a `.sys` that is
314            // really something else, and a file on a share that cannot be read.
315            match graph.root().state {
316                crate::pe::State::Found => Payload::Binary(Arc::new(graph)),
317                crate::pe::State::Unreadable(why) => Payload::Failed(capitalise(why)),
318                _ => Payload::Failed("Cannot be read".to_owned()),
319            }
320        }
321        // Text if it looks like text, and nothing if it does not.
322        Kind::Unknown => match sniff(path) {
323            Some(true) => text(path, code_of(path)),
324            Some(false) => Payload::Failed("Not something this can show".to_owned()),
325            None => Payload::Failed("Cannot be read".to_owned()),
326        },
327    }
328}
329
330/// Whether the front of `path` reads as text. `None` if it cannot be opened.
331///
332/// Two tests, and both matter. **A NUL byte** is the oldest and still the best binary tell: no
333/// text encoding this would show puts one in the middle of a document, and every executable,
334/// archive and database is full of them. **Valid UTF-8** over the same window catches the rest —
335/// with the last few bytes forgiven, because a 4 KB window will usually cut a multi-byte
336/// character in half and that is not a reason to refuse the file.
337fn sniff(path: &Path) -> Option<bool> {
338    use std::io::Read as _;
339
340    let mut file = std::fs::File::open(path).ok()?;
341    let mut head = vec![0u8; SNIFF];
342    let read = file.read(&mut head).ok()?;
343    head.truncate(read);
344    if head.contains(&0) {
345        return Some(false);
346    }
347    Some(match std::str::from_utf8(&head) {
348        Ok(_) => true,
349        // `valid_up_to` past all but the last few bytes means the only invalid sequence is the
350        // character the window cut in half.
351        Err(why) => why.valid_up_to() + 4 >= head.len(),
352    })
353}
354
355/// Whether the file at `path` wants the monospace role, from its name.
356fn code_of(path: &Path) -> bool {
357    let name = path
358        .file_stem()
359        .map(|n| n.to_string_lossy().into_owned())
360        .unwrap_or_default();
361    is_code(&name, &extension_of(path))
362}
363
364fn text(path: &Path, code: bool) -> Payload {
365    use std::io::Read as _;
366
367    let Ok(mut file) = std::fs::File::open(path) else {
368        return Payload::Failed("Cannot be opened".to_owned());
369    };
370    // One byte past the cap, so a file exactly at it is not reported as truncated.
371    let mut buffer = Vec::new();
372    if file
373        .by_ref()
374        .take(TEXT_CAP as u64 + 1)
375        .read_to_end(&mut buffer)
376        .is_err()
377    {
378        return Payload::Failed("Cannot be read".to_owned());
379    }
380    let truncated = buffer.len() > TEXT_CAP;
381    buffer.truncate(TEXT_CAP);
382    // Lossy rather than a refusal: a file that is text apart from one bad byte is still worth
383    // reading, and `from_utf8_lossy` puts a replacement character where the byte was.
384    let mut body = String::from_utf8_lossy(&buffer).into_owned();
385    // A `\r` that survives into a galley is laid out as a glyph — a hollow box, at the end of
386    // every line of every file written on this platform.
387    if body.contains('\r') {
388        body = body.replace("\r\n", "\n").replace('\r', "\n");
389    }
390    // And a tab, which egui lays out as a single space. Four, because the alternative is that
391    // every indented file in the preview is flat.
392    if body.contains('\t') {
393        body = body.replace('\t', "    ");
394    }
395    Payload::Text(Text {
396        body,
397        truncated,
398        code,
399    })
400}
401
402/// Two pictures, and a mask of where they differ.
403///
404/// **Compared at the larger of the two sizes**, with a pixel that exists in only one of them
405/// counted as differing. Two files of different dimensions are not the same picture, and saying so
406/// by lighting up the region one of them does not reach is more useful than either refusing to
407/// compare them or quietly cropping to the overlap and reporting a small difference.
408fn compare(a: &Path, b: &Path) -> Payload {
409    let (Payload::Picture(a), Payload::Picture(b)) = (picture(a), picture(b)) else {
410        // Whichever failed, its own complaint is the useful one — so it is read again rather than
411        // guessed at. Both are page-cache warm by now.
412        return match picture(a) {
413            Payload::Failed(why) => Payload::Failed(why),
414            _ => picture(b),
415        };
416    };
417
418    let size = [
419        a.pixels.size[0].max(b.pixels.size[0]),
420        a.pixels.size[1].max(b.pixels.size[1]),
421    ];
422    let at = |picture: &Picture, x: usize, y: usize| -> Option<egui::Color32> {
423        let [w, h] = picture.pixels.size;
424        (x < w && y < h).then(|| picture.pixels.pixels[y * w + x])
425    };
426
427    let mut pixels = Vec::with_capacity(size[0] * size[1]);
428    let mut differing = 0usize;
429    for y in 0..size[1] {
430        for x in 0..size[0] {
431            let delta = match (at(&a, x, y), at(&b, x, y)) {
432                (Some(one), Some(other)) => {
433                    // The largest single-channel difference, alpha included. A per-channel maximum
434                    // rather than a sum, so a picture that differs in one channel by a lot is not
435                    // averaged down towards one that differs in three by a little.
436                    let ([p, q], [r, s]) = ([one.r(), one.g()], [other.r(), other.g()]);
437                    (p.abs_diff(r))
438                        .max(q.abs_diff(s))
439                        .max(one.b().abs_diff(other.b()))
440                        .max(one.a().abs_diff(other.a()))
441                }
442                // Present in one and not the other: as different as it gets.
443                _ => 255,
444            };
445            if delta > 0 {
446                differing += 1;
447            }
448            // **Amplified eightfold.** A one-level difference at alpha 1 is invisible, and the
449            // whole job of this image is to be *findable*: past a delta of 32 it is fully opaque,
450            // and below that it fades rather than vanishing. The count above is the honest
451            // measurement; this is the visible one.
452            pixels.push(egui::Color32::from_white_alpha(
453                ((delta as u32) * 8).min(255) as u8,
454            ));
455        }
456    }
457
458    let total = (size[0] * size[1]).max(1);
459    Payload::Diff(Box::new(Diff {
460        differing: differing as f32 / total as f32,
461        mask: Picture {
462            pixels: egui::ColorImage {
463                size,
464                pixels,
465                source_size: egui::vec2(size[0] as f32, size[1] as f32),
466            },
467            natural: [size[0] as u32, size[1] as u32],
468            scaled: false,
469            vector: false,
470        },
471        a: *a,
472        b: *b,
473    }))
474}
475
476fn picture(path: &Path) -> Payload {
477    let vector = path
478        .extension()
479        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"));
480    match if vector { vector_art(path) } else { raster(path) } {
481        Ok(picture) => Payload::Picture(Box::new(picture)),
482        Err(why) => Payload::Failed(why),
483    }
484}
485
486/// Anything `image` can decode.
487fn raster(path: &Path) -> Result<Picture, String> {
488    // The format from the *contents* rather than from the extension, which is how a `.jpg` that
489    // is really a PNG — and there are a great many of those — still opens.
490    let open = || -> Result<image::ImageReader<std::io::BufReader<std::fs::File>>, String> {
491        image::ImageReader::open(path)
492            .map_err(|_| "Cannot be opened".to_owned())?
493            .with_guessed_format()
494            .map_err(|_| "Cannot be read".to_owned())
495    };
496
497    // **How big it is, before deciding to decode it.** Only the header is read for this, and it
498    // is the one bound that has to come first: `image` has no streaming resize, so a decode is
499    // the full size in memory however small the answer is going to be. A 40-megapixel image is
500    // 160 MB while it is being scaled down, and that is as far as a preview gets to go.
501    let natural = open()?
502        .into_dimensions()
503        .map_err(|why| short(&why.to_string()))?;
504    let pixels = natural.0 as u64 * natural.1 as u64;
505    if pixels > DECODE_MAX {
506        return Err(format!(
507            "Too large to preview: {:.0} megapixels",
508            pixels as f64 / 1e6
509        ));
510    }
511
512    let decoded = open()?
513        .decode()
514        .map_err(|why| short(&why.to_string()))?
515        .into_rgba8();
516
517    let (width, height) = (decoded.width(), decoded.height());
518    let scaled = width > CAP || height > CAP;
519    let decoded = if scaled {
520        // `Triangle` rather than `Lanczos3`: this is a preview, the difference is invisible at
521        // the size it is shown, and Lanczos over a 200-megapixel scan is seconds rather than
522        // milliseconds.
523        let ratio = (CAP as f32 / width.max(height) as f32).min(1.0);
524        image::imageops::resize(
525            &decoded,
526            ((width as f32 * ratio) as u32).max(1),
527            ((height as f32 * ratio) as u32).max(1),
528            image::imageops::FilterType::Triangle,
529        )
530    } else {
531        decoded
532    };
533
534    Ok(Picture {
535        pixels: egui::ColorImage::from_rgba_unmultiplied(
536            [decoded.width() as usize, decoded.height() as usize],
537            decoded.as_raw(),
538        ),
539        natural: [natural.0, natural.1],
540        scaled,
541        vector: false,
542    })
543}
544
545/// The extension, lowercased, for the panel's "no preview for this" line.
546pub fn extension_of(path: &Path) -> String {
547    path.extension()
548        .map(|ext| ext.to_string_lossy().to_lowercase())
549        .unwrap_or_default()
550}
551
552/// SVG, rasterised.
553///
554/// At [`CAP`] on the long edge of the document's own aspect, rather than at the size the panel
555/// happens to be: the panel is resized and zoomed, and re-rasterising on every frame that changes
556/// would be a parse and a fill per frame. The cost is that zooming far past fit goes soft, which
557/// is what happens to a raster image too.
558///
559/// **Text inside the SVG is not drawn.** `usvg`'s text support means a font database, shaping and
560/// system font enumeration — see the dependency's justification in `Cargo.toml` — and the honest
561/// thing is to say so, which [`Picture::vector`] is for.
562fn vector_art(path: &Path) -> Result<Picture, String> {
563    use resvg::tiny_skia;
564    use resvg::usvg;
565
566    let bytes = std::fs::read(path).map_err(|_| "Cannot be opened".to_owned())?;
567    let tree = usvg::Tree::from_data(&bytes, &usvg::Options::default())
568        .map_err(|why| short(&why.to_string()))?;
569
570    let size = tree.size();
571    if size.width() < 1.0 || size.height() < 1.0 {
572        return Err("The drawing has no size".to_owned());
573    }
574    // Always to the cap on the long edge, up as well as down. A 16-point icon is the commonest
575    // thing in a folder of SVGs and rasterising it at its nominal size would put a 16-pixel
576    // square in the middle of a 400-point panel — the whole point of vector art is that there is
577    // no natural size to respect. The cap is what bounds the cost, and it is the same cap every
578    // other picture here is held at.
579    let ratio = CAP as f32 / size.width().max(size.height());
580    let width = ((size.width() * ratio) as u32).max(1);
581    let height = ((size.height() * ratio) as u32).max(1);
582
583    let mut canvas =
584        tiny_skia::Pixmap::new(width, height).ok_or_else(|| "Too large to draw".to_owned())?;
585    resvg::render(
586        &tree,
587        tiny_skia::Transform::from_scale(ratio, ratio),
588        &mut canvas.as_mut(),
589    );
590
591    // `tiny_skia` hands back premultiplied RGBA, which is what egui wants — so the bytes go
592    // across as they are rather than through the unmultiplied constructor, which would divide
593    // the alpha back out and then multiply it in again.
594    let pixels = egui::ColorImage {
595        size: [width as usize, height as usize],
596        pixels: canvas
597            .pixels()
598            .iter()
599            .map(|p| {
600                egui::Color32::from_rgba_premultiplied(p.red(), p.green(), p.blue(), p.alpha())
601            })
602            .collect(),
603        source_size: egui::vec2(width as f32, height as f32),
604    };
605    Ok(Picture {
606        pixels,
607        natural: [size.width() as u32, size.height() as u32],
608        scaled: false,
609        vector: true,
610    })
611}
612
613/// A word or two, as a sentence for the middle of a panel.
614fn capitalise(text: &str) -> String {
615    let mut chars = text.chars();
616    match chars.next() {
617        Some(first) => first.to_uppercase().chain(chars).collect(),
618        None => "Cannot be read".to_owned(),
619    }
620}
621
622/// A decoder's complaint, cut to something that fits on one line of a panel.
623fn short(why: &str) -> String {
624    let first = why.split(['\n', ':']).next().unwrap_or(why).trim();
625    let mut out = if first.is_empty() { why } else { first }.to_owned();
626    out.truncate(80);
627    // Capitalised, because it goes in the middle of a panel as a sentence rather than into a log.
628    capitalise(&out)
629}
630
631#[cfg(test)]
632mod tests {
633    use super::*;
634    use std::path::PathBuf;
635
636    fn scratch(name: &str) -> PathBuf {
637        let root = std::env::temp_dir().join(format!("yafe-preview-{}", std::process::id()));
638        std::fs::create_dir_all(&root).expect("a directory in the temp folder");
639        root.join(name)
640    }
641
642    #[test]
643    fn a_name_says_what_it_will_be_shown_as() {
644        assert_eq!(kind_of("a.png", "png", false), Some(Kind::Picture));
645        assert_eq!(kind_of("a.PNG", "PNG", false), Some(Kind::Picture));
646        assert_eq!(kind_of("a.svg", "svg", false), Some(Kind::Picture));
647        assert_eq!(kind_of("a.md", "md", false), Some(Kind::Text));
648        assert_eq!(kind_of("a.rs", "rs", false), Some(Kind::Text));
649        assert_eq!(kind_of("a.dll", "dll", false), Some(Kind::Binary));
650        assert_eq!(kind_of("a.exe", "exe", false), Some(Kind::Binary));
651        // Nothing to go on: the worker looks instead. This is what makes `README` and
652        // `Makefile` previewable, which is most of what is in a source folder without one.
653        assert_eq!(kind_of("README", "", false), Some(Kind::Unknown));
654        assert_eq!(kind_of("Makefile", "", false), Some(Kind::Unknown));
655        assert_eq!(kind_of(".gitignore", "gitignore", false), Some(Kind::Text));
656        assert_eq!(kind_of(".npmrc", "npmrc", false), Some(Kind::Unknown));
657        // And things that genuinely have no preview.
658        assert_eq!(kind_of("a.zip", "zip", false), None);
659        assert_eq!(kind_of("a.mp4", "mp4", false), None);
660        // A folder is what the listing beside the panel is already showing.
661        assert_eq!(kind_of("src", "", true), None);
662    }
663
664    /// Text is read, its line endings are made harmless, and a long one is cut and says so.
665    #[test]
666    fn text_comes_back_readable_and_bounded() {
667        let path = scratch("notes.txt");
668        std::fs::write(&path, "one\r\ntwo\ttabbed\r\nthree").expect("a file");
669        let Payload::Text(text) = read(&Ask::One(path.clone(), Kind::Text)) else {
670            panic!("a text file did not come back as text");
671        };
672        assert!(!text.truncated);
673        assert!(
674            !text.body.contains('\r'),
675            "a CR survived into the galley: {:?}",
676            text.body
677        );
678        assert!(!text.body.contains('\t'), "a tab survived");
679        assert_eq!(text.body, "one\ntwo    tabbed\nthree");
680
681        // Past the cap: the front of it, and it says so.
682        let long = scratch("long.txt");
683        std::fs::write(&long, "x".repeat(TEXT_CAP + 100)).expect("a big file");
684        let Payload::Text(text) = read(&Ask::One(long.clone(), Kind::Text)) else {
685            panic!("not text");
686        };
687        assert!(text.truncated, "a file past the cap did not admit it");
688        assert_eq!(text.body.len(), TEXT_CAP);
689
690        // Exactly at it is not truncated, which is the off-by-one worth pinning.
691        let exact = scratch("exact.txt");
692        std::fs::write(&exact, "y".repeat(TEXT_CAP)).expect("a file at the cap");
693        let Payload::Text(text) = read(&Ask::One(exact.clone(), Kind::Text)) else {
694            panic!("not text");
695        };
696        assert!(!text.truncated);
697        let _ = std::fs::remove_file(path);
698        let _ = std::fs::remove_file(long);
699        let _ = std::fs::remove_file(exact);
700    }
701
702    /// An extensionless file is text if it reads as text, and refused if it does not.
703    ///
704    /// The refusal is the half that matters: without it, pointing the panel at a file with no
705    /// extension would paint whatever bytes it holds into a wrapped paragraph.
706    #[test]
707    fn an_unknown_file_is_sniffed_rather_than_guessed() {
708        let readme = scratch("README");
709        std::fs::write(&readme, "# A project\n\nWith some prose in it.\n").expect("a file");
710        assert_eq!(sniff(&readme), Some(true));
711        assert!(matches!(read(&Ask::One(readme.clone(), Kind::Unknown)), Payload::Text(_)));
712
713        // A NUL is the tell, and it is checked before UTF-8 — the bytes below are valid UTF-8.
714        let blob = scratch("blob");
715        std::fs::write(&blob, b"MZ\x90\x00\x03\x00\x00\x00").expect("a file");
716        assert_eq!(sniff(&blob), Some(false));
717        assert!(matches!(read(&Ask::One(blob.clone(), Kind::Unknown)), Payload::Failed(_)));
718
719        // Invalid UTF-8 well inside the window, with no NUL anywhere.
720        let latin = scratch("latin");
721        let mut bytes = b"caf\xe9 and more prose after it, at length, so the bad byte is not near the end".to_vec();
722        bytes.extend(std::iter::repeat_n(b'x', 200));
723        std::fs::write(&latin, &bytes).expect("a file");
724        assert_eq!(sniff(&latin), Some(false));
725
726        // But a multi-byte character cut in half by the window is forgiven, or every UTF-8 file
727        // longer than the sniff window would be refused about a quarter of the time.
728        let cut = scratch("cut");
729        let mut bytes = "é".repeat(SNIFF).into_bytes();
730        bytes.truncate(SNIFF - 1);
731        std::fs::write(&cut, &bytes).expect("a file");
732        assert_eq!(sniff(&cut), Some(true));
733
734        assert_eq!(sniff(Path::new("no-such-file-at-all")), None);
735        for path in [readme, blob, latin, cut] {
736            let _ = std::fs::remove_file(path);
737        }
738    }
739
740    /// A picture decodes, keeps its alpha, and is scaled to fit the cap rather than refused.
741    ///
742    /// The fixture is written here rather than checked in: a binary blob in the repository is
743    /// something nobody can read, and `image` is already in the graph to write one with.
744    #[test]
745    fn a_picture_keeps_its_alpha_and_is_bounded() {
746        let path = scratch("swatch.png");
747        // Four pixels: opaque red, half-transparent green, transparent, opaque white.
748        let mut buffer = image::RgbaImage::new(2, 2);
749        buffer.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
750        buffer.put_pixel(1, 0, image::Rgba([0, 255, 0, 128]));
751        buffer.put_pixel(0, 1, image::Rgba([0, 0, 0, 0]));
752        buffer.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
753        buffer.save(&path).expect("a PNG in the temp folder");
754
755        let Payload::Picture(picture) = read(&Ask::One(path.clone(), Kind::Picture)) else {
756            panic!("a PNG did not come back as a picture");
757        };
758        assert_eq!(picture.pixels.size, [2, 2]);
759        assert_eq!(picture.natural, [2, 2]);
760        assert!(!picture.scaled && !picture.vector);
761        // The alpha survived, which is what the checkerboard behind the canvas is for. egui
762        // premultiplies on the way in, so the half-transparent green is checked by its alpha.
763        assert_eq!(picture.pixels.pixels[0], egui::Color32::from_rgb(255, 0, 0));
764        assert_eq!(picture.pixels.pixels[1].a(), 128);
765        assert_eq!(picture.pixels.pixels[2].a(), 0);
766
767        // And one past the cap comes back at the cap, saying so.
768        let big = scratch("big.png");
769        image::RgbaImage::from_pixel(CAP + 40, 10, image::Rgba([1, 2, 3, 255]))
770            .save(&big)
771            .expect("a wide PNG");
772        let Payload::Picture(picture) = read(&Ask::One(big.clone(), Kind::Picture)) else {
773            panic!("not a picture");
774        };
775        assert!(picture.scaled, "an oversized picture did not admit it");
776        assert_eq!(picture.pixels.size[0], CAP as usize);
777        assert_eq!(
778            picture.natural,
779            [CAP + 40, 10],
780            "the reported size is the scaled one rather than the file's"
781        );
782
783        // Something that is not an image at all says so instead of panicking.
784        let lie = scratch("lie.png");
785        std::fs::write(&lie, b"this is not a PNG").expect("a file");
786        assert!(matches!(read(&Ask::One(lie.clone(), Kind::Picture)), Payload::Failed(_)));
787        for path in [path, big, lie] {
788            let _ = std::fs::remove_file(path);
789        }
790    }
791
792    /// SVG is rasterised, and its alpha comes through premultiplied the way egui wants it.
793    #[test]
794    fn vector_art_is_rasterised_to_fit() {
795        let path = scratch("mark.svg");
796        // A red square on the left half of a 100×50 canvas, and nothing on the right — so the
797        // transparent half is there to check.
798        std::fs::write(
799            &path,
800            r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
801                  <rect x="0" y="0" width="50" height="50" fill="#ff0000"/>
802                </svg>"##,
803        )
804        .expect("an SVG in the temp folder");
805
806        let Payload::Picture(picture) = read(&Ask::One(path.clone(), Kind::Picture)) else {
807            panic!("an SVG did not come back as a picture");
808        };
809        assert!(picture.vector, "it was not reported as vector art");
810        assert_eq!(picture.natural, [100, 50]);
811        // Rasterised to the cap on its long edge, keeping its aspect.
812        assert_eq!(picture.pixels.size, [CAP as usize, CAP as usize / 2]);
813        let at = |x: usize, y: usize| picture.pixels.pixels[y * picture.pixels.size[0] + x];
814        assert_eq!(at(10, 10), egui::Color32::from_rgb(255, 0, 0), "the fill");
815        assert_eq!(at(picture.pixels.size[0] - 10, 10).a(), 0, "the empty half");
816
817        let broken = scratch("broken.svg");
818        std::fs::write(&broken, b"<svg").expect("a file");
819        assert!(matches!(read(&Ask::One(broken.clone(), Kind::Picture)), Payload::Failed(_)));
820        let _ = std::fs::remove_file(path);
821        let _ = std::fs::remove_file(broken);
822    }
823
824    /// Which files are set in monospace, and which are not.
825    ///
826    /// The line is a question with an answer rather than a matter of taste: does moving a character
827    /// sideways change what the file means?
828    #[test]
829    fn columns_that_mean_something_get_the_monospace_face() {
830        for (name, ext) in [
831            ("main", "rs"),
832            ("build", "log"),
833            ("data", "csv"),
834            ("package", "json"),
835            ("Makefile", ""),
836            (".npmrc", "npmrc"),
837            ("setup", "bat"),
838        ] {
839            assert!(is_code(name, ext), "{name}.{ext} has columns that matter");
840        }
841        for (name, ext) in [
842            ("notes", "txt"),
843            ("README", "md"),
844            ("readme", ""),
845            ("LICENSE", ""),
846            ("CHANGELOG", ""),
847        ] {
848            assert!(!is_code(name, ext), "{name}.{ext} is prose");
849        }
850    }
851
852    /// Two pictures come back as three images and a number.
853    ///
854    /// The number is the half that a picture cannot show: "they are the same file" and "0.02% of it
855    /// moved" look identical at the size a preview is drawn at.
856    #[test]
857    fn two_pictures_are_compared_at_the_larger_of_the_two_sizes() {
858        let a = scratch("left.png");
859        let b = scratch("right.png");
860        // 4×2 and 6×2, so the comparison has to reach past the end of the first one.
861        let mut one = image::RgbaImage::from_pixel(4, 2, image::Rgba([10, 20, 30, 255]));
862        let mut other = image::RgbaImage::from_pixel(6, 2, image::Rgba([10, 20, 30, 255]));
863        // One pixel differs inside the overlap, by a lot.
864        other.put_pixel(1, 1, image::Rgba([200, 20, 30, 255]));
865        one.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
866        one.save(&a).expect("a PNG");
867        other.save(&b).expect("another PNG");
868
869        let Payload::Diff(diff) = read(&Ask::Pair(a.clone(), b.clone())) else {
870            panic!("two pictures did not come back as a comparison");
871        };
872        assert_eq!(diff.a.pixels.size, [4, 2]);
873        assert_eq!(diff.b.pixels.size, [6, 2]);
874        // The mask is the larger of the two, which is what the panel places all three against.
875        assert_eq!(diff.mask.pixels.size, [6, 2]);
876
877        let at = |x: usize, y: usize| diff.mask.pixels.pixels[y * 6 + x];
878        assert_eq!(at(0, 0).a(), 0, "identical pixels are transparent");
879        assert_eq!(at(1, 1).a(), 255, "a large difference is opaque");
880        // Present in one and not the other: as different as it gets.
881        assert_eq!(at(5, 0).a(), 255, "past the end of the narrower one");
882        // Five of twelve differ: the one inside the overlap, and the two columns of two that only
883        // `b` reaches. Reported honestly, before the amplification the mask uses.
884        assert!(
885            (diff.differing - 5.0 / 12.0).abs() < 1e-6,
886            "{} of the pixels were reported as differing",
887            diff.differing
888        );
889
890        // A difference of one level is *visible* rather than exact: the mask is amplified so it can
891        // be found, and the count above is the honest measurement.
892        let faint = scratch("faint.png");
893        image::RgbaImage::from_pixel(4, 2, image::Rgba([11, 20, 30, 255]))
894            .save(&faint)
895            .expect("a PNG");
896        let Payload::Diff(diff) = read(&Ask::Pair(a.clone(), faint.clone())) else {
897            panic!("not a comparison");
898        };
899        assert_eq!(diff.mask.pixels.pixels[0].a(), 8, "one level, amplified");
900        assert!((diff.differing - 1.0).abs() < 1e-6, "every pixel differs");
901
902        // And if either side is not a picture at all, its own complaint is what comes back.
903        let lie = scratch("lie.png");
904        std::fs::write(&lie, b"not a PNG").expect("a file");
905        assert!(matches!(
906            read(&Ask::Pair(a.clone(), lie.clone())),
907            Payload::Failed(_)
908        ));
909        for path in [a, b, faint, lie] {
910            let _ = std::fs::remove_file(path);
911        }
912    }
913
914    #[test]
915    fn a_complaint_is_cut_to_something_that_fits_in_a_panel() {
916        assert_eq!(short("Format error: the header is wrong"), "Format error");
917        assert_eq!(short("bad magic"), "Bad magic");
918        assert_eq!(short(""), "Cannot be read");
919        assert!(short(&"very long complaint ".repeat(20)).len() <= 80);
920    }
921}
