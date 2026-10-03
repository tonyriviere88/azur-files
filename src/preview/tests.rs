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
    // **And everything else is the shell's**, which is the point of the lists above being short: they
    // are what this program decodes better than Windows would, not what has a preview. A `.pdf` and an
    // `.mp4` have a registered visualizer; a `.zip` does not, and the difference is not knowable from
    // the name — so all three come here and `visual::load` is what finds out.
    if cfg!(windows) {
        assert_eq!(kind_of("a.zip", "zip", false), Some(Kind::Shell));
        assert_eq!(kind_of("a.mp4", "mp4", false), Some(Kind::Shell));
        assert_eq!(kind_of("a.pdf", "pdf", false), Some(Kind::Shell));
        assert_eq!(kind_of("a.docx", "docx", false), Some(Kind::Shell));
    } else {
        assert_eq!(kind_of("a.pdf", "pdf", false), None);
    }
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
}

