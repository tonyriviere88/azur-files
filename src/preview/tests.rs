use super::search::HITS;
use super::text::{code_of, sniff};
use super::*;
use std::path::{PathBuf, MAIN_SEPARATOR};

fn scratch(name: &str) -> PathBuf {
    let root = crate::sandbox::dir("preview");
    std::fs::create_dir_all(&root).expect("a directory in the temp folder");
    root.join(name)
}

/// What a search finds, as the matched text rather than as offsets — a list of byte ranges is
/// not something you can read an assertion off.
fn found<'a>(body: &'a str, text: &str, case: bool, word: bool, regex: bool) -> Vec<&'a str> {
    let search = Search {
        text: text.to_owned(),
        case,
        word,
        regex,
    };
    hits(body, &search)
        .at
        .into_iter()
        .map(|at| &body[at])
        .collect()
}

#[test]
fn a_search_is_case_insensitive_until_it_is_told_otherwise() {
    let body = "Foo foo FOO";
    assert_eq!(found(body, "foo", false, false, false), ["Foo", "foo", "FOO"]);
    assert_eq!(found(body, "foo", true, false, false), ["foo"]);
    assert_eq!(found(body, "FOO", true, false, false), ["FOO"]);
    // And past ASCII, which is the whole reason the fold is per character.
    assert_eq!(found("Élan élan", "élan", false, false, false), ["Élan", "élan"]);
    assert_eq!(found("Élan élan", "élan", true, false, false), ["élan"]);
}

#[test]
fn hits_never_overlap() {
    // Two matches sharing a character are not two places to step to.
    assert_eq!(found("aaaa", "aa", false, false, false), ["aa", "aa"]);
    assert_eq!(found("aaa", "aa", false, false, false), ["aa"]);
}

#[test]
fn whole_word_means_what_a_word_boundary_means() {
    let body = "word wording sword _word word_";
    assert_eq!(found(body, "word", false, false, false).len(), 5);
    assert_eq!(
        found(body, "word", false, true, false),
        ["word"],
        "only the one standing on its own"
    );
    // The boundary is where word-ness *changes*, so a needle that starts with punctuation is
    // still findable — the tempting rule refuses this one.
    assert_eq!(found("a-x b", "-x", false, true, false), ["-x"]);
}

#[test]
fn a_regex_is_a_pattern_and_a_literal_is_not() {
    let body = "a1 b22 c333";
    assert_eq!(found(body, r"\d+", false, false, true), ["1", "22", "333"]);
    assert_eq!(
        found(body, r"\d+", false, false, false),
        Vec::<&str>::new(),
        "the literal characters `\\d+` are not in there"
    );
    // The two flags compose the way they read: `\b(?:..)\b`.
    assert_eq!(found("in inn", "in", false, true, true), ["in"]);
    assert_eq!(found("in inn", "in", false, false, true), ["in", "in"]);
    // And case still applies to a pattern.
    assert_eq!(found("Ab ab", "a.", true, false, true), ["ab"]);
}

#[test]
fn a_pattern_that_is_not_a_pattern_says_so() {
    let bad = hits(
        "anything",
        &Search {
            text: "(unclosed".to_owned(),
            regex: true,
            ..Search::default()
        },
    );
    assert!(bad.bad);
    assert!(bad.at.is_empty());
    // The same text as a literal is fine, which is what makes the flag worth having a state for
    // rather than clearing the field.
    assert_eq!(found("a (unclosed b", "(unclosed", false, false, false).len(), 1);
}

#[test]
fn a_pattern_that_matches_nothing_at_all_finds_nothing() {
    // `a*` matches emptily at every one of the 1,048,577 positions in a full body. None of them
    // is a place to go, and collecting them would be the slowest thing in the program.
    assert_eq!(found("bbb", "a*", false, false, true), Vec::<&str>::new());
    // But a pattern that can match emptily *and* substantially still finds the substance.
    assert_eq!(found("b1b22", r"\d*", false, false, true), ["1", "22"]);
}

#[test]
fn the_search_stops_counting_at_its_budget() {
    // The first keystroke of any search over a large file is this case.
    let body = "a".repeat(HITS * 2);
    let all = hits(
        &body,
        &Search {
            text: "a".to_owned(),
            ..Search::default()
        },
    );
    assert_eq!(all.at.len(), HITS);
    assert!(all.capped, "it stopped without saying so");
    // Which the counter has to be able to tell from a search that simply found that many.
    let exact = hits(
        &"a".repeat(HITS),
        &Search {
            text: "a".to_owned(),
            ..Search::default()
        },
    );
    assert_eq!(exact.at.len(), HITS);
    assert!(!exact.capped);
}

/// **And it stops walking as well as counting**, because the walk's cost is the product of two numbers
/// and only one of them is bounded.
///
/// The literal search compares the needle at every position of the body, so a needle that nearly matches
/// nearly everywhere costs `body × needle` — which for a pasted thousand-character needle over a
/// megabyte of minified source is 10^9 comparisons, on the UI thread, on every keystroke. This body and
/// needle are that shape: every position matches for a thousand characters and then fails.
#[test]
fn a_literal_search_stops_walking_as_well_as_counting() {
    let body = "a".repeat(200_000);
    let needle = format!("{}b", "a".repeat(1_000));
    let stopped = hits(
        &body,
        &Search {
            text: needle,
            ..Search::default()
        },
    );
    assert!(stopped.at.is_empty(), "the needle is not in there");
    assert!(
        stopped.capped,
        "it walked the whole 2×10^8 rather than stopping"
    );

    // The budget is nowhere near an ordinary search, which is what makes it invisible: a needle that
    // fails on its first character costs one comparison per position, and a short one that keeps failing
    // late costs a handful. Neither of these is capped, and the second still finds what is there.
    let plain = hits(
        &body,
        &Search {
            text: "b".to_owned(),
            ..Search::default()
        },
    );
    assert!(!plain.capped && plain.at.is_empty());
    let late = hits(
        &format!("{body}aaab"),
        &Search {
            text: "aaab".to_owned(),
            ..Search::default()
        },
    );
    assert!(!late.capped, "an ordinary needle ran out of budget");
    assert_eq!(late.at.len(), 1);
}

#[test]
fn an_empty_search_finds_nothing_rather_than_everything() {
    let nothing = hits("some text", &Search::default());
    assert!(nothing.at.is_empty() && !nothing.capped && !nothing.bad);
}

#[test]
fn every_hit_is_a_range_of_the_body() {
    // The ranges become sections of a layout job over this exact string, where one that is out
    // of bounds or reversed is a panic rather than a wrong colour.
    let body = "héllo wörld héllo";
    let found = hits(
        body,
        &Search {
            text: "héllo".to_owned(),
            ..Search::default()
        },
    );
    assert_eq!(found.at.len(), 2);
    let mut last = 0;
    for at in &found.at {
        assert!(at.start < at.end, "reversed");
        assert!(at.end <= body.len(), "past the end");
        assert!(body.is_char_boundary(at.start) && body.is_char_boundary(at.end));
        assert!(at.start >= last, "out of order");
        last = at.end;
    }
}

#[test]
fn a_name_says_what_it_will_be_shown_as() {
    assert_eq!(kind_of("a.png", "png", false), Some(Kind::Picture));
    assert_eq!(kind_of("a.PNG", "PNG", false), Some(Kind::Picture));
    assert_eq!(kind_of("a.svg", "svg", false), Some(Kind::Picture));
    assert_eq!(kind_of("a.md", "md", false), Some(Kind::Text));
    assert_eq!(kind_of("a.rs", "rs", false), Some(Kind::Text));
    assert_eq!(kind_of("a.dll", "dll", false), Some(Kind::Binary));
    assert_eq!(kind_of("a.exe", "exe", false), Some(Kind::Binary));
    // Nothing to go on: the worker looks instead. This is what makes `README` and
    // `Makefile` previewable, which is most of what is in a source folder without one.
    assert_eq!(kind_of("README", "", false), Some(Kind::Unknown));
    assert_eq!(kind_of("Makefile", "", false), Some(Kind::Unknown));
    assert_eq!(kind_of(".gitignore", "gitignore", false), Some(Kind::Text));
    assert_eq!(kind_of(".npmrc", "npmrc", false), Some(Kind::Unknown));
    // **And everything else is looked at**, which is the point of the lists above being short: they
    // are what this program has a better answer for than looking would give, not what has a preview.
    // A `.tex` is text, a `.pdf` has a registered visualizer, a `.zip` has neither — and not one of
    // those three is knowable from the name, so all of them come here and `read` finds out. On either
    // platform: whether a file is text is not a question about the platform.
    assert_eq!(kind_of("a.zip", "zip", false), Some(Kind::Shell));
    assert_eq!(kind_of("a.pdf", "pdf", false), Some(Kind::Shell));
    assert_eq!(kind_of("a.docx", "docx", false), Some(Kind::Shell));
    assert_eq!(kind_of("a.tex", "tex", false), Some(Kind::Shell));
    assert_eq!(kind_of("a.vcxproj", "vcxproj", false), Some(Kind::Shell));
    if cfg!(windows) {
        // Except a video, which is neither decoded here nor rendered by the shell: it is *played*,
        // by Media Foundation, which is a player and not a read. See `Kind::Video`.
        assert_eq!(kind_of("a.mp4", "mp4", false), Some(Kind::Video));
        assert_eq!(kind_of("a.MKV", "MKV", false), Some(Kind::Video));
        assert_eq!(kind_of("a.mov", "mov", false), Some(Kind::Video));
        assert_eq!(kind_of("a.webm", "webm", false), Some(Kind::Video));
        // A video by name that no ordinary machine has a decoder for is still a video by name —
        // what a file *is* does not depend on what is installed, and the engine says so in the
        // panel when it cannot open one.
        assert_eq!(kind_of("a.flv", "flv", false), Some(Kind::Video));
    } else {
        // No engine off Windows, so a video is whatever looking at it makes of it — which for an
        // `.mp4` is the shell's kind, and off Windows the shell has nothing. Same "No preview" the
        // panel has always shown for one there.
        assert_eq!(kind_of("a.mp4", "mp4", false), Some(Kind::Shell));
    }
    // And an animated GIF stays a picture, because this program decodes those itself and can
    // zoom, diff and compare the result. **This is the assertion that guards the order of the
    // tests in `kind_of`**: the video question is asked first, so a `.gif` that `crate::fs::fmt`
    // ever came to call a video would lose all of that — and would fail here first.
    assert_eq!(kind_of("a.gif", "gif", false), Some(Kind::Picture));
    // A folder is what the listing beside the panel is already showing, on either platform.
    assert_eq!(kind_of("src", "", true), None);
    assert_eq!(kind_of("pics", "", true), None);
}

/// **What this program cannot decode, Windows draws** — and what Windows cannot draw either says so
/// rather than pretending.
///
/// The two halves of [`visual`], which are the two things a panel over an unknown file can be. The
/// positive one goes through a `.png`, deliberately and not through the `.pdf` that motivated the
/// feature: a PNG thumbnail provider ships with Windows, so this tests *this program's* plumbing —
/// the apartment on the worker, the size asked for, the `Picture` that comes out — rather than which
/// PDF reader the machine happens to have installed. Measured here with a real one for the record:
/// a valid single-page PDF came back 724 × 1024 in 708 ms cold and 64 ms warm.
///
/// `.png` reaches [`visual::load`] only because this calls it directly. Through [`kind_of`] it is a
/// [`Kind::Picture`] and always will be — see that function for why a decoder here beats a render
/// there whenever there is one.
#[cfg(windows)]
#[test]
fn the_shell_draws_what_this_program_cannot_and_says_so_when_it_cannot_either() {
    let _one = crate::shell::serialised();
    let dir = crate::sandbox::fresh("preview-visual");

    // Larger than `visual::SIZE` on both edges, so the answer proves the cap as well as the render.
    let wide = dir.join("wide.png");
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::RgbaImage::from_fn(3000, 2000, |x, y| {
        image::Rgba([(x % 256) as u8, (y % 256) as u8, 64, 255])
    })
    .write_to(&mut encoded, image::ImageFormat::Png)
    .expect("`image` can encode a PNG");
    std::fs::write(&wide, encoded.into_inner()).expect("a file in the sandbox");

    let Payload::Picture(picture) = visual::load(&wide) else {
        panic!("the shell drew nothing for a PNG, which every Windows has a provider for");
    };
    assert!(picture.shell, "the bar would report the render's size as the file's");
    let [w, h] = picture.pixels.size;
    assert!(
        w as u32 <= visual::SIZE && h as u32 <= visual::SIZE,
        "the render came back {w} × {h}, past the {} that was asked for — which is also past the \
         bound in `windows::icons::read_bgra`, so the next size up would come back as nothing",
        visual::SIZE
    );
    assert!(w > 1 && h > 1, "the render is {w} × {h}, which is not a picture");
    // 3000 × 2000 fits its own aspect inside the cap, so the long edge *is* the cap: anything else
    // means `SIIGBF_RESIZETOFIT` was not honoured and the panel is holding a stretched picture.
    assert_eq!(w as u32, visual::SIZE, "the long edge is not the size asked for");
    assert!(h < w, "the aspect was not kept: {w} × {h} from a 3:2 picture");
    // The render's own size, because a document has no pixel size of its own — which is the whole
    // reason `shell` has to travel with it.
    assert_eq!(picture.natural, [w as u32, h as u32]);
    assert!(!picture.scaled, "nothing here threw pixels away to fit `CAP`");

    // And the other half: an extension nothing on earth has a provider for. Not `Failed` — nothing
    // went wrong, so the panel says "No preview for a .nosuchthing" rather than showing an error.
    let unknown = dir.join("mystery.nosuchthing");
    std::fs::write(&unknown, vec![0u8; 4096]).expect("a file in the sandbox");
    assert!(
        matches!(read(&Ask::One(unknown, Kind::Shell)), Payload::Unsupported),
        "a type with no visualizer was reported as a failure rather than as having no preview"
    );

    // A file with nothing to go on that turns out not to be text falls through to the shell too,
    // rather than stopping at "not something this can show" — a renamed `.psd` is still a picture.
    let nameless = dir.join("binary-blob");
    std::fs::write(&nameless, [0u8, 1, 2, 0, 255, 0, 7]).expect("a file in the sandbox");
    assert!(
        matches!(read(&Ask::One(nameless, Kind::Unknown)), Payload::Unsupported),
        "a sniffed binary never reached the shell"
    );
}

/// Text is read, its line endings are made harmless, and a long one is cut and says so.
#[test]
fn text_comes_back_readable_and_bounded() {
    let path = scratch("notes.txt");
    std::fs::write(&path, "one\r\ntwo\ttabbed\r\nthree").expect("a file");
    let Payload::Text(text) = read(&Ask::One(path.clone(), Kind::Text)) else {
        panic!("a text file did not come back as text");
    };
    assert!(!text.truncated);
    assert!(
        !text.body.contains('\r'),
        "a CR survived into the galley: {:?}",
        text.body
    );
    assert!(!text.body.contains('\t'), "a tab survived");
    assert_eq!(text.body, "one\ntwo    tabbed\nthree");

    // Past the cap: the front of it, and it says so.
    let long = scratch("long.txt");
    std::fs::write(&long, "x".repeat(TEXT_CAP + 100)).expect("a big file");
    let Payload::Text(text) = read(&Ask::One(long.clone(), Kind::Text)) else {
        panic!("not text");
    };
    assert!(text.truncated, "a file past the cap did not admit it");
    assert_eq!(text.body.len(), TEXT_CAP);

    // Exactly at it is not truncated, which is the off-by-one worth pinning.
    let exact = scratch("exact.txt");
    std::fs::write(&exact, "y".repeat(TEXT_CAP)).expect("a file at the cap");
    let Payload::Text(text) = read(&Ask::One(exact.clone(), Kind::Text)) else {
        panic!("not text");
    };
    assert!(!text.truncated);
    crate::sandbox::remove_file(&path);
    crate::sandbox::remove_file(&long);
    crate::sandbox::remove_file(&exact);
}

/// **An extension in none of the tables is read as text when it is text**, which is the whole of the
/// preview priority below the four name tables: video, picture, binary, listed text — and then a look
/// at the bytes.
///
/// Every extension here is deliberately one nobody thought to list, and every one of them used to be
/// "No preview for a .tex" — not because the shell was asked and said no, but because the shell has
/// no visualizer for a build script either. The `code` flag travels with the answer because it is the
/// same question [`is_code`] always answered from the name, and a sniffed file still has one.
///
/// The other half — an unlisted extension that is *not* text going on to the shell — is
/// `the_shell_draws_what_this_program_cannot_and_says_so_when_it_cannot_either`, which asks about a
/// `.nosuchthing` full of NUL bytes through this very kind. It is not repeated here, because
/// asserting it costs a real shell call and this test is meant to cost a `read`.
#[test]
fn an_unlisted_extension_is_read_as_text_when_it_is_text() {
    let cases = [
        // A build script, a subtitle file, an MSBuild project, a desktop entry: four formats with
        // nothing in common except that a panel showing them is better than a panel saying nothing.
        ("paper.tex", "documentclass{article}\n\nSome prose.\n"),
        ("film.srt", "1\n00:00:01,000 --> 00:00:04,000\nA line of dialogue.\n"),
        ("app.vcxproj", "<Project ToolsVersion=\"4.0\">\n  <ItemGroup/>\n</Project>\n"),
        ("app.desktop", "[Desktop Entry]\nName=A thing\nExec=/usr/bin/thing\n"),
    ];
    for (name, body) in cases {
        let path = scratch(name);
        std::fs::write(&path, body).expect("a file");
        // Through `kind_of` rather than with the kind written out, so this pins the *priority* and
        // not just the arm in `read`: none of these is a video, a picture, a binary or a listed
        // text extension, so every one of them has to land on `Kind::Shell`.
        let ext = extension_of(&path);
        let kind = kind_of(name, &ext, false).expect("a file always has some preview to try");
        assert_eq!(kind, Kind::Shell, "{name} was claimed by one of the name tables");
        let Payload::Text(text) = read(&Ask::One(path.clone(), kind)) else {
            panic!("{name} is text and the panel would have said there was no preview for it");
        };
        assert!(text.body.starts_with(body.split('\n').next().expect("a line")));
        assert!(!text.truncated);
        // Columns that mean something, for all four: none of these extensions is in `PROSE`.
        assert!(text.code, "{name} would be laid out as a paragraph");
        crate::sandbox::remove_file(&path);
    }
}

/// **XML and JSON are coloured under a name that does not say so** — and a name that *does* say
/// something is never overruled by the body.
///
/// The wiring rather than the probe, which is
/// `syntax::tests::the_language_comes_from_the_body_when_the_name_says_nothing`. What this pins is
/// the precedence in [`text::lang_and_face`] and the one thing that surprises about it: the face
/// moves with the language. A `.txt` is prose by [`is_code`] and gets the proportional role, but a
/// `.txt` holding an XML document is columns that mean something, and laying that out in a
/// proportional face would throw away the indentation that is most of what the file says.
#[test]
fn a_body_that_declares_itself_is_coloured_whatever_it_is_called() {
    let read_as_text = |name: &str, body: &str| {
        let path = scratch(name);
        std::fs::write(&path, body).expect("a file");
        let ext = extension_of(&path);
        let kind = kind_of(name, &ext, false).expect("a file always has some preview to try");
        let Payload::Text(text) = read(&Ask::One(path.clone(), kind)) else {
            panic!("{name} did not come back as text");
        };
        crate::sandbox::remove_file(&path);
        (text.lang, text.code)
    };

    // A structured log and a REST capture: the two shapes this exists for. Neither extension is in
    // `syntax::lang_of`'s table and neither ever will be — `.log` is whatever a program writes.
    assert_eq!(
        read_as_text("service.log", "{\"level\": \"warn\", \"msg\": \"disk full\"}\n"),
        (crate::syntax::Lang::Json, true)
    );
    assert_eq!(
        read_as_text("capture.txt", "<?xml version=\"1.0\"?>\n<soap:Envelope/>\n"),
        (crate::syntax::Lang::Markup, true),
        "a `.txt` is prose by name, and this one is a document with indentation that means something"
    );
    // A file with no extension at all goes down the same road, through `Kind::Unknown` and its
    // sniff — which is the other half of what makes an unnamed dump readable.
    assert_eq!(
        read_as_text("dump", "<root>\n  <item id=\"1\"/>\n</root>\n"),
        (crate::syntax::Lang::Markup, true)
    );

    // **And the name wins wherever it said anything.** A `.md` is a document even when it opens
    // with a brace, which is what keeps the markdown renderer reachable — see `Lang::Markdown`.
    assert_eq!(
        read_as_text("doc.md", "{\"looks\": \"like json\"}\n"),
        (crate::syntax::Lang::Markdown, false)
    );
    // Prose that declares nothing is left exactly as it was: no colour, no monospace.
    assert_eq!(
        read_as_text("prose.txt", "Just some notes about the thing.\n"),
        (crate::syntax::Lang::None, false)
    );
    // And a `.log` of ordinary lines keeps the face its name earned and gains no language.
    assert_eq!(
        read_as_text("build.log", "warning: unused variable `x`\n"),
        (crate::syntax::Lang::None, true)
    );
}

/// An extensionless file is text if it reads as text, and **never** text if it does not.
///
/// The refusal is the half that matters: without it, pointing the panel at a file with no
/// extension would paint whatever bytes it holds into a wrapped paragraph.
///
/// What happens to the refused file is [`visual`]'s business rather than this test's — it goes to the
/// shell, which is what makes a renamed `.psd` previewable — so the assertions below are on [`sniff`]
/// and on the answer *not* being text. `the_shell_draws_what_this_program_cannot_and_says_so_when_it_cannot_either`
/// is where the fall-through itself is pinned.
#[test]
fn an_unknown_file_is_sniffed_rather_than_guessed() {
    let readme = scratch("README");
    std::fs::write(&readme, "# A project\n\nWith some prose in it.\n").expect("a file");
    assert_eq!(sniff(&readme), Some(true));
    assert!(matches!(read(&Ask::One(readme.clone(), Kind::Unknown)), Payload::Text(_)));

    // A NUL is the tell, and it is checked before UTF-8 — the bytes below are valid UTF-8.
    let blob = scratch("blob");
    std::fs::write(&blob, b"MZ\x90\x00\x03\x00\x00\x00").expect("a file");
    assert_eq!(sniff(&blob), Some(false));
    assert!(
        !matches!(read(&Ask::One(blob.clone(), Kind::Unknown)), Payload::Text(_)),
        "a file full of NULs was painted into a wrapped paragraph"
    );

    // Invalid UTF-8 well inside the window, with no NUL anywhere.
    let latin = scratch("latin");
    let mut bytes = b"caf\xe9 and more prose after it, at length, so the bad byte is not near the end".to_vec();
    bytes.extend(std::iter::repeat_n(b'x', 200));
    std::fs::write(&latin, &bytes).expect("a file");
    assert_eq!(sniff(&latin), Some(false));

    // But a multi-byte character cut in half by the window is forgiven, or every UTF-8 file
    // longer than the sniff window would be refused about a quarter of the time.
    let cut = scratch("cut");
    let mut bytes = "é".repeat(SNIFF).into_bytes();
    bytes.truncate(SNIFF - 1);
    std::fs::write(&cut, &bytes).expect("a file");
    assert_eq!(sniff(&cut), Some(true));

    assert_eq!(sniff(Path::new("no-such-file-at-all")), None);
    for path in [readme, blob, latin, cut] {
        crate::sandbox::remove_file(&path);
    }
}

/// A picture decodes, keeps its alpha, and is scaled to fit the cap rather than refused.
///
/// The fixture is written here rather than checked in: a binary blob in the repository is
/// something nobody can read, and `image` is already in the graph to write one with.
#[test]
fn a_picture_keeps_its_alpha_and_is_bounded() {
    let path = scratch("swatch.png");
    // Four pixels: opaque red, half-transparent green, transparent, opaque white.
    let mut buffer = image::RgbaImage::new(2, 2);
    buffer.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    buffer.put_pixel(1, 0, image::Rgba([0, 255, 0, 128]));
    buffer.put_pixel(0, 1, image::Rgba([0, 0, 0, 0]));
    buffer.put_pixel(1, 1, image::Rgba([255, 255, 255, 255]));
    buffer.save(&path).expect("a PNG in the temp folder");

    let Payload::Picture(picture) = read(&Ask::One(path.clone(), Kind::Picture)) else {
        panic!("a PNG did not come back as a picture");
    };
    assert_eq!(picture.pixels.size, [2, 2]);
    assert_eq!(picture.natural, [2, 2]);
    assert!(!picture.scaled && !picture.vector);
    // The alpha survived, which is what the checkerboard behind the canvas is for. egui
    // premultiplies on the way in, so the half-transparent green is checked by its alpha.
    assert_eq!(picture.pixels.pixels[0], egui::Color32::from_rgb(255, 0, 0));
    assert_eq!(picture.pixels.pixels[1].a(), 128);
    assert_eq!(picture.pixels.pixels[2].a(), 0);

    // And one past the cap comes back at the cap, saying so.
    let big = scratch("big.png");
    image::RgbaImage::from_pixel(CAP + 40, 10, image::Rgba([1, 2, 3, 255]))
        .save(&big)
        .expect("a wide PNG");
    let Payload::Picture(picture) = read(&Ask::One(big.clone(), Kind::Picture)) else {
        panic!("not a picture");
    };
    assert!(picture.scaled, "an oversized picture did not admit it");
    assert_eq!(picture.pixels.size[0], CAP as usize);
    assert_eq!(
        picture.natural,
        [CAP + 40, 10],
        "the reported size is the scaled one rather than the file's"
    );

    // Something that is not an image at all says so instead of panicking.
    let lie = scratch("lie.png");
    std::fs::write(&lie, b"this is not a PNG").expect("a file");
    assert!(matches!(read(&Ask::One(lie.clone(), Kind::Picture)), Payload::Failed(_)));
    for path in [path, big, lie] {
        crate::sandbox::remove_file(&path);
    }
}

/// **A photograph is shown the way up it was taken**, which is a tag rather than the pixels.
///
/// A camera writes the sensor's own landscape frame and records the rotation as EXIF `Orientation`, so
/// a portrait photograph is stored on its side and every reader is expected to turn it back. `image`
/// does not do that on its own, and the shell's codec behind
/// [`crate::shell::thumbs`] does — which is exactly how this was found: the tiles in the grid were
/// upright and the panel showing the same file was not.
///
/// The fixture is a landscape frame split red | blue with `Orientation` = 6, "rotate 90° clockwise",
/// spliced in by hand. Hand-built because `image` has no EXIF *encoder*, and split down the middle
/// rather than marked in a corner because JPEG is lossy and a half of the frame survives it in a way a
/// single pixel does not. 90° clockwise takes the left half to the top, so red on top is the assertion
/// and it cannot be satisfied by an accident of the aspect ratio alone.
#[test]
fn a_photograph_is_turned_the_way_the_camera_was_held() {
    /// An `APP1` segment holding the smallest EXIF that can say "rotate 90° clockwise".
    ///
    /// A camera puts this immediately after the `SOI`, ahead of any JFIF `APP0`, which is where it is
    /// spliced below. Every field is fixed, so it is written out as bytes rather than assembled.
    const TURNED: [u8; 36] = [
        0xFF, 0xE1, // APP1
        0x00, 0x22, // 34 bytes, a length that counts its own two
        b'E', b'x', b'i', b'f', 0x00, 0x00, // what makes an APP1 an EXIF one
        0x49, 0x49, 0x2A, 0x00, // `II*\0`: a TIFF header, little-endian
        0x08, 0x00, 0x00, 0x00, // IFD0 is eight bytes in — directly after this header
        0x01, 0x00, // holding one entry
        0x12, 0x01, // tag 0x0112, `Orientation`
        0x03, 0x00, // of type 3, SHORT
        0x01, 0x00, 0x00, 0x00, // one of them
        0x06, 0x00, 0x00, 0x00, // 6, "rotate 90° clockwise", in the first two of four value bytes
        0x00, 0x00, 0x00, 0x00, // and no IFD1 after it
    ];

    // Twice as wide as it is tall, so the shape alone says whether it was turned.
    let mut frame = std::io::Cursor::new(Vec::new());
    image::RgbImage::from_fn(64, 32, |x, _| {
        if x < 32 {
            image::Rgb([255, 0, 0])
        } else {
            image::Rgb([0, 0, 255])
        }
    })
    .write_to(&mut frame, image::ImageFormat::Jpeg)
    .expect("`image` can encode a JPEG");
    let frame = frame.into_inner();

    /// Which of red and blue won at a point, as a decoded picture is sampled.
    fn hue_at(picture: &Picture, x: usize, y: usize) -> egui::Color32 {
        picture.pixels.pixels[y * picture.pixels.size[0] + x]
    }

    // The control first: the same bytes with nothing spliced into them are shown as stored, so a
    // failure below is about the tag rather than about the fixture or the encoder.
    let flat = scratch("as-stored.jpg");
    std::fs::write(&flat, &frame).expect("a JPEG in the temp folder");
    let Payload::Picture(picture) = read(&Ask::One(flat.clone(), Kind::Picture)) else {
        panic!("a JPEG did not come back as a picture");
    };
    assert_eq!(picture.pixels.size, [64, 32], "an untagged JPEG was turned anyway");
    assert_eq!(picture.natural, [64, 32]);
    let (left, right) = (hue_at(&picture, 16, 16), hue_at(&picture, 48, 16));
    assert!(left.r() > left.b(), "the left half of the control is not red: {left:?}");
    assert!(right.b() > right.r(), "the right half of the control is not blue: {right:?}");

    // And the same frame, tagged.
    let turned = scratch("turned.jpg");
    let mut tagged = frame[..2].to_vec();
    tagged.extend_from_slice(&TURNED);
    tagged.extend_from_slice(&frame[2..]);
    std::fs::write(&turned, &tagged).expect("a JPEG in the temp folder");
    let Payload::Picture(picture) = read(&Ask::One(turned.clone(), Kind::Picture)) else {
        panic!("a JPEG carrying an EXIF orientation did not come back as a picture at all");
    };
    assert_eq!(
        picture.pixels.size, [32, 64],
        "the tag was ignored: a portrait photograph is still lying on its side in the panel"
    );
    // The bar reports this, and the zoom percentage divides by it — the stored 64 × 32 under a canvas
    // that is plainly taller than it is wide would be the two contradicting each other.
    assert_eq!(
        picture.natural, [32, 64],
        "the size on the bar is the stored one rather than the one on the canvas"
    );
    assert!(!picture.scaled && !picture.vector);
    // Clockwise and not anticlockwise: the left of the frame is the top of the picture.
    let (top, bottom) = (hue_at(&picture, 16, 16), hue_at(&picture, 16, 48));
    assert!(
        top.r() > top.b(),
        "the frame's left half did not land on top — it was turned the wrong way: {top:?}"
    );
    assert!(bottom.b() > bottom.r(), "the frame's right half is not at the bottom: {bottom:?}");

    for path in [flat, turned] {
        crate::sandbox::remove_file(&path);
    }
}

/// **A `.cur` decodes, and its name is the whole reason it can.**
///
/// A cursor is an icon whose directory entries carry a hotspot where an icon's carry the colour planes
/// and the bit depth — which `image`'s ICO decoder reads past, so the pixels come out — but `cur` is in
/// neither of `image`'s tables: not in the extension list, and its magic is `00 00 02 00` where an
/// icon's is `00 00 01 00`. So the format has to be *named*, and until it was, the preview of the one
/// name in [`PICTURES`] nothing could recognise was a library's complaint about the extension.
///
/// The fixture is an icon with the two bytes that say "cursor" changed and a hotspot written into the
/// entry, which is what a `.cur` is. Written here rather than checked in, like the pictures above.
#[test]
fn a_cursor_decodes_because_the_name_says_what_it_is() {
    let path = scratch("pointer.cur");
    let mut art = image::RgbaImage::new(8, 8);
    art.put_pixel(0, 0, image::Rgba([255, 0, 0, 255]));
    art.put_pixel(7, 7, image::Rgba([0, 0, 255, 255]));
    let icon = scratch("pointer.ico");
    art.save(&icon).expect("an ICO in the temp folder");

    let mut bytes = std::fs::read(&icon).expect("what was just written");
    // `ICONDIR`: two reserved bytes, then the type — 1 for an icon and 2 for a cursor.
    bytes[2..4].copy_from_slice(&2u16.to_le_bytes());
    // And in the one directory entry that follows it, the hotspot: four bytes at offset 10 that an
    // icon spends on its colour planes and bit depth. A cursor points from somewhere.
    bytes[10..12].copy_from_slice(&3u16.to_le_bytes());
    bytes[12..14].copy_from_slice(&4u16.to_le_bytes());
    std::fs::write(&path, &bytes).expect("a cursor in the temp folder");

    let Payload::Picture(picture) = read(&Ask::One(path.clone(), Kind::Picture)) else {
        panic!("a .cur did not come back as a picture — the format was not named");
    };
    assert_eq!(picture.pixels.size, [8, 8]);
    assert_eq!(picture.pixels.pixels[0], egui::Color32::from_rgb(255, 0, 0));
    assert_eq!(picture.pixels.pixels[63], egui::Color32::from_rgb(0, 0, 255));

    // **The bytes still win.** A name is consulted only where the sniff came back with nothing, so a
    // `.cur` that is really a PNG opens as the PNG it is rather than being forced into the ICO decoder.
    let lying = scratch("actually-a-png.cur");
    art.save_with_format(&lying, image::ImageFormat::Png).expect("a PNG under a cursor's name");
    assert!(
        matches!(read(&Ask::One(lying.clone(), Kind::Picture)), Payload::Picture(_)),
        "the name overruled the contents"
    );

    for path in [path, icon, lying] {
        crate::sandbox::remove_file(&path);
    }
}

/// SVG is rasterised, and its alpha comes through premultiplied the way egui wants it.
#[test]
fn vector_art_is_rasterised_to_fit() {
    let path = scratch("mark.svg");
    // A red square on the left half of a 100×50 canvas, and nothing on the right — so the
    // transparent half is there to check.
    std::fs::write(
        &path,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
              <rect x="0" y="0" width="50" height="50" fill="#ff0000"/>
            </svg>"##,
    )
    .expect("an SVG in the temp folder");

    let Payload::Picture(picture) = read(&Ask::One(path.clone(), Kind::Picture)) else {
        panic!("an SVG did not come back as a picture");
    };
    assert!(picture.vector, "it was not reported as vector art");
    assert_eq!(picture.natural, [100, 50]);
    // Rasterised to the cap on its long edge, keeping its aspect.
    assert_eq!(picture.pixels.size, [CAP as usize, CAP as usize / 2]);
    let at = |x: usize, y: usize| picture.pixels.pixels[y * picture.pixels.size[0] + x];
    assert_eq!(at(10, 10), egui::Color32::from_rgb(255, 0, 0), "the fill");
    assert_eq!(at(picture.pixels.size[0] - 10, 10).a(), 0, "the empty half");

    let broken = scratch("broken.svg");
    std::fs::write(&broken, b"<svg").expect("a file");
    assert!(matches!(read(&Ask::One(broken.clone(), Kind::Picture)), Payload::Failed(_)));
    crate::sandbox::remove_file(&path);
    crate::sandbox::remove_file(&broken);
}

/// Which files are set in monospace, and which are not.
///
/// The line is a question with an answer rather than a matter of taste: does moving a character
/// sideways change what the file means?
#[test]
fn columns_that_mean_something_get_the_monospace_face() {
    for (name, ext) in [
        ("main", "rs"),
        ("build", "log"),
        ("data", "csv"),
        ("package", "json"),
        ("Makefile", ""),
        (".npmrc", "npmrc"),
        ("setup", "bat"),
        ("toolchain", "cmake"),
        // The one the extension gets wrong on its own: `txt` is prose and this is a build
        // script. Any case, because the name is written every way round in the wild — and the
        // *stem*, not the whole name, which is what the caller passes.
        ("CMakeLists", "txt"),
        ("cmakelists", "txt"),
        ("CMAKELISTS", "TXT"),
    ] {
        assert!(is_code(name, ext), "{name}.{ext} has columns that matter");
    }
    for (name, ext) in [
        ("notes", "txt"),
        // And the exception is the whole stem and not a word in it: a file *about* the build
        // script is still prose.
        ("about-CMakeLists", "txt"),
        ("CMakeLists-notes", "txt"),
        ("README", "md"),
        ("readme", ""),
        ("LICENSE", ""),
        ("CHANGELOG", ""),
    ] {
        assert!(!is_code(name, ext), "{name}.{ext} is prose");
    }
}

/// The same question asked the way the program asks it: of a **path**.
///
/// `is_code` takes the name with the extension already off, and its own test can hand it anything
/// — which is how `CODE_NAMES` first shipped holding `cmakelists.txt`, a string `code_of` cannot
/// produce and so a rule that could never have fired. This one goes through the function the
/// worker calls, so the two halves of the name have to be split the way the program splits them.
#[test]
fn the_face_is_chosen_from_the_path_the_worker_is_given() {
    // Both separators, because a path arrives here from `Dir::target` and from a command line.
    for path in [
        "C:/src/CMakeLists.txt",
        "CMakeLists.txt",
        "deep/cmake/cmakelists.TXT",
        "C:/src/toolchain.cmake",
    ] {
        assert!(
            code_of(Path::new(path)),
            "`{path}` is a build script and wants the monospace face"
        );
    }
    let native = format!("C:{MAIN_SEPARATOR}src{MAIN_SEPARATOR}CMakeLists.txt");
    assert!(code_of(Path::new(&native)), "`{native}` on this platform");
    for path in [
        "C:/src/notes.txt",
        "C:/src/README.md",
        "C:/src/about-cmakelists.txt",
    ] {
        assert!(!code_of(Path::new(path)), "`{path}` is prose");
    }
}

/// Two pictures come back as three images and a number.
///
/// The number is the half that a picture cannot show: "they are the same file" and "0.02% of it
/// moved" look identical at the size a preview is drawn at.
#[test]
fn two_pictures_are_compared_at_the_larger_of_the_two_sizes() {
    let a = scratch("left.png");
    let b = scratch("right.png");
    // 4×2 and 6×2, so the comparison has to reach past the end of the first one.
    let mut one = image::RgbaImage::from_pixel(4, 2, image::Rgba([10, 20, 30, 255]));
    let mut other = image::RgbaImage::from_pixel(6, 2, image::Rgba([10, 20, 30, 255]));
    // One pixel differs inside the overlap, by a lot.
    other.put_pixel(1, 1, image::Rgba([200, 20, 30, 255]));
    one.put_pixel(0, 0, image::Rgba([10, 20, 30, 255]));
    one.save(&a).expect("a PNG");
    other.save(&b).expect("another PNG");

    let Payload::Diff(diff) = read(&Ask::Pair(a.clone(), b.clone())) else {
        panic!("two pictures did not come back as a comparison");
    };
    assert_eq!(diff.a.pixels.size, [4, 2]);
    assert_eq!(diff.b.pixels.size, [6, 2]);
    // The mask is the larger of the two, which is what the panel places all three against.
    assert_eq!(diff.mask.pixels.size, [6, 2]);

    let at = |x: usize, y: usize| diff.mask.pixels.pixels[y * 6 + x];
    assert_eq!(at(0, 0).a(), 0, "identical pixels are transparent");
    assert_eq!(at(1, 1).a(), 255, "a large difference is opaque");
    // Present in one and not the other: as different as it gets.
    assert_eq!(at(5, 0).a(), 255, "past the end of the narrower one");
    // Five of twelve differ: the one inside the overlap, and the two columns of two that only
    // `b` reaches. Reported honestly, before the amplification the mask uses.
    assert!(
        (diff.differing - 5.0 / 12.0).abs() < 1e-6,
        "{} of the pixels were reported as differing",
        diff.differing
    );

    // A difference of one level is *visible* rather than exact: the mask is amplified so it can
    // be found, and the count above is the honest measurement.
    let faint = scratch("faint.png");
    image::RgbaImage::from_pixel(4, 2, image::Rgba([11, 20, 30, 255]))
        .save(&faint)
        .expect("a PNG");
    let Payload::Diff(diff) = read(&Ask::Pair(a.clone(), faint.clone())) else {
        panic!("not a comparison");
    };
    assert_eq!(diff.mask.pixels.pixels[0].a(), 8, "one level, amplified");
    assert!((diff.differing - 1.0).abs() < 1e-6, "every pixel differs");

    // And if either side is not a picture at all, its own complaint is what comes back.
    let lie = scratch("lie.png");
    std::fs::write(&lie, b"not a PNG").expect("a file");
    assert!(matches!(
        read(&Ask::Pair(a.clone(), lie.clone())),
        Payload::Failed(_)
    ));
    for path in [a, b, faint, lie] {
        crate::sandbox::remove_file(&path);
    }
}

/// A picture against the version in the last commit: the same three views, from a blob rather
/// than from a second file.
///
/// End to end through `read`, because the interesting parts are the ones a unit test of the
/// pixels would skip: that the *committed* bytes are what `a` holds, and that a file git has
/// nothing older of comes back as one picture rather than as a failure.
#[test]
fn a_picture_is_compared_with_the_one_in_the_last_commit() {
    let root = crate::sandbox::dir("imgdiff");
    crate::sandbox::remove(&root);
    std::fs::create_dir_all(&root).expect("a temp folder");

    let run = |args: &[&str]| {
        let mut command = std::process::Command::new("git");
        command.args(args).current_dir(&root);
        crate::shell::no_window(&mut command);
        command
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    };
    if !run(&["init", "--quiet"]) {
        eprintln!("no git: skipping");
        return;
    }
    assert!(run(&["config", "user.email", "test@example.invalid"]));
    assert!(run(&["config", "user.name", "Test"]));
    assert!(run(&["config", "commit.gpgsign", "false"]));

    // 2×1, committed grey, and then one of its two pixels turned red on disk.
    let path = root.join("mark.png");
    let before = image::RgbaImage::from_pixel(2, 1, image::Rgba([10, 20, 30, 255]));
    before.save(&path).expect("a PNG");
    assert!(run(&["add", "-A"]));
    assert!(run(&["commit", "--quiet", "-m", "the picture"]));
    let mut after = before.clone();
    after.put_pixel(1, 0, image::Rgba([200, 20, 30, 255]));
    after.save(&path).expect("a changed PNG");

    let Payload::Diff(diff) = read(&Ask::AgainstHead(path.clone())) else {
        panic!("a changed picture did not come back as a comparison");
    };
    // `a` is the commit's and `b` is the file's, which is the order the captions are written in.
    assert_eq!(diff.a.pixels.pixels[1].r(), 10, "a is what was committed");
    assert_eq!(diff.b.pixels.pixels[1].r(), 200, "b is what is on disk");
    assert_eq!(
        diff.mask.pixels.pixels[0].a(),
        0,
        "the pixel that did not move"
    );
    assert_eq!(diff.mask.pixels.pixels[1].a(), 255, "and the one that did");
    assert!((diff.differing - 0.5).abs() < 1e-6, "one pixel of two");

    // A file git has nothing older of is still a picture. Every path through here has to end in
    // one: the file is there, and somebody asked to see it.
    let fresh = root.join("fresh.png");
    before.save(&fresh).expect("a PNG");
    assert!(
        matches!(read(&Ask::AgainstHead(fresh)), Payload::Picture(_)),
        "an untracked picture should still be shown"
    );
    // And one that will not decode is its own complaint rather than a comparison's.
    let lie = root.join("lie.png");
    std::fs::write(&lie, b"not a PNG").expect("a file");
    assert!(matches!(read(&Ask::AgainstHead(lie)), Payload::Failed(_)));

    crate::sandbox::remove(&root);
}

#[test]
fn a_complaint_is_cut_to_something_that_fits_in_a_panel() {
    assert_eq!(short("Format error: the header is wrong"), "Format error");
    assert_eq!(short("bad magic"), "Bad magic");
    assert_eq!(short(""), "Cannot be read");
    assert!(short(&"very long complaint ".repeat(20)).len() <= 80);
    // Byte 80 is inside the twenty-seventh `▽`; cutting there panicked.
    assert_eq!(short(&"▽".repeat(30)).chars().count(), 30);
    assert_eq!(short(&"é".repeat(100)).chars().count(), 80);
}


/// **A video frame is fetched at the size it will be drawn**, which is the bound that makes copying
/// every frame back from the graphics device affordable at all.
///
/// Four rules in one function, and each of them is a thing that goes wrong without it — see
/// [`video::wanted`]. The aspect ratio is checked on every answer rather than only where it is the
/// point: the canvas draws this texture into a rect it has fitted, so a size that has drifted off
/// the file's shape is a picture stretched by a pixel or two with nothing to say it happened.
#[test]
fn a_video_frame_is_asked_for_at_the_size_it_will_be_drawn() {
    use egui::vec2;
    use video::wanted;

    let ratio = |[w, h]: [u32; 2]| w as f32 / h as f32;
    let hd = [1920u32, 1080];

    // A panel a few hundred points wide gets a few hundred pixels, not two million: 400 points
    // against 1920 is a fit of 0.208, which ceils onto the second of eight steps — a quarter.
    let small = wanted(hd, vec2(400.0, 300.0), 1.0);
    assert_eq!(small, [480, 270]);
    assert!((ratio(small) - ratio(hd)).abs() < 0.01, "{small:?} is not 16:9");

    // The same panel on a 2× display asks for twice the pixels, because that is how many will be
    // drawn — 0.417 of the file, which is the fourth step.
    assert_eq!(wanted(hd, vec2(400.0, 300.0), 2.0), [960, 540]);

    // **Never past the file's own size.** A clip smaller than the panel is fetched at its size and
    // the canvas is what stretches it — enlarging on the device and then again in egui would be two
    // blurs for the price of one.
    assert_eq!(wanted([320, 240], vec2(1200.0, 900.0), 1.0), [320, 240]);

    // **And never past `CAP`**, which for a 4K file is the rule that decides the answer.
    let huge = wanted([3840, 2160], vec2(4000.0, 3000.0), 1.0);
    assert!(huge[0] <= CAP && huge[1] <= CAP, "{huge:?} is over the cap");
    assert!((ratio(huge) - ratio(hd)).abs() < 0.01, "{huge:?} is not 16:9");

    // **Quantised**, which is what keeps a splitter drag from reallocating a texture pair on every
    // frame of it: a range of panel widths has to come back with one answer.
    let steady: Vec<[u32; 2]> = (0..12)
        .map(|i| wanted(hd, vec2(400.0 + i as f32, 300.0), 1.0))
        .collect();
    assert_eq!(
        steady.iter().collect::<std::collections::BTreeSet<_>>().len(),
        1,
        "twelve panel widths one point apart wanted more than one frame size: {steady:?}"
    );

    // A panel dragged to nothing still asks for something the engine will accept.
    let tiny = wanted(hd, vec2(0.0, 0.0), 1.0);
    assert!(tiny[0] >= 1 && tiny[1] >= 1, "{tiny:?} is not a texture");
    // As does a file whose size is not known yet, which is what the caller checks for.
    assert_eq!(wanted([0, 0], vec2(400.0, 300.0), 1.0), [0, 0]);
}

/// The strip's clock: minutes for a clip, hours only when there are hours.
#[test]
fn a_position_in_a_video_reads_as_a_clock() {
    use video::clock;

    assert_eq!(clock(0.0), "0:00");
    assert_eq!(clock(7.4), "0:07");
    assert_eq!(clock(59.9), "0:59");
    assert_eq!(clock(60.0), "1:00");
    assert_eq!(clock(252.0), "4:12");
    // Padded to two digits inside the hour and not outside it: `1:03:20` reads as a position and
    // `0:00:07` reads as a stopwatch.
    assert_eq!(clock(3800.0), "1:03:20");
    // Nothing sensible is still a clock rather than a panic or an empty label: a stream's duration
    // is infinite and an unopened file's is not a number.
    assert_eq!(clock(f64::NAN), "0:00");
    assert_eq!(clock(f64::INFINITY), "0:00");
    assert_eq!(clock(-5.0), "0:00");
}
