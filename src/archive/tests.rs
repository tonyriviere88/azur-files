//! Every archive here is **built by the test that reads it**, in the sandbox.
//!
//! No binary fixtures in the repository, for two reasons. One is readability: what the assertion is
//! about is the twelve lines of `Builder` calls above it, rather than a `.zip` somebody has to
//! extract to understand the test. The other is that a fixture cannot be reviewed — a checked-in
//! archive is an opaque blob in a diff, and this module's whole subject is reading files that came
//! from somewhere else.
//!
//! Writing them needs encoders the shipped binary deliberately does not have; see the
//! `[dev-dependencies]` block in `Cargo.toml`, which adds exactly one and says why that costs the
//! executable nothing.

use super::*;
use std::io::Write;

/// A directory of this test's own, inside the sandbox. See [`crate::sandbox`].
fn work(name: &str) -> PathBuf {
    crate::sandbox::fresh(&format!("archive-{name}"))
}

/// Build a zip. `(name, contents)`, and a name ending in `/` is a stored directory entry.
fn zip_at(path: &Path, entries: &[(&str, &str)]) {
    let file = std::fs::File::create(path).expect("sandbox");
    let mut writer = zip::ZipWriter::new(file);
    // Stored rather than deflated for most of these: it keeps the test about the *directory* — the
    // names, sizes and dates this module reads — rather than about a codec. One deflated entry
    // appears in `a_zip_lists_its_entries`, so the compressed path is exercised too.
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (name, body) in entries {
        if let Some(folder) = name.strip_suffix('/') {
            writer.add_directory(folder, stored).expect("zip");
            continue;
        }
        writer.start_file(*name, stored).expect("zip");
        writer.write_all(body.as_bytes()).expect("zip");
    }
    writer.finish().expect("zip");
}

/// Build a `.tar.gz`.
fn targz_at(path: &Path, entries: &[(&str, &str)]) {
    let file = std::fs::File::create(path).expect("sandbox");
    let gz = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
    let mut builder = tar::Builder::new(gz);
    for (name, body) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o644);
        // 2021-01-01T00:00:00Z, so the date assertions have something definite to be about.
        header.set_mtime(1_609_459_200);
        header.set_cksum();
        builder
            .append_data(&mut header, name, body.as_bytes())
            .expect("tar");
    }
    builder.into_inner().expect("tar").finish().expect("gz");
}

/// The listing of one folder inside an archive, by the route the pane uses.
fn listing_of(path: &Path) -> Dir {
    crate::fs::scan::scan(path)
}

/// Every row's name, in the order the listing built them.
fn names(dir: &Dir) -> Vec<&str> {
    (0..dir.len()).map(|i| dir.name(i)).collect()
}

// ---------------------------------------------------------------------------
// Recognising an archive, without touching the disk
// ---------------------------------------------------------------------------

/// The suffix table has to be longest-first, or `.tar.gz` is found as a `.gz` and a source
/// tarball lists as a single file called `pkg.tar`.
///
/// Asserted about the table itself rather than only about its answers, because the table is the
/// kind of thing somebody adds a line to in the middle.
#[test]
fn the_suffix_table_is_ordered_longest_first() {
    for pair in SUFFIXES.windows(2) {
        let (before, after) = (pair[0].0, pair[1].0);
        assert!(
            before.len() >= after.len(),
            "{before} must not come before the longer {after}: \
             a shorter suffix earlier in the table shadows every longer one after it"
        );
    }
}

#[test]
fn a_name_says_which_format_it_is() {
    let format = |name: &str| format_of(name);

    assert_eq!(format("pkg.zip"), Some(Format::Zip));
    assert_eq!(format("PKG.ZIP"), Some(Format::Zip), "case does not matter");
    assert_eq!(format("thing.7z"), Some(Format::SevenZ));
    assert_eq!(format("pkg.tar"), Some(Format::Tar));

    // The whole reason for the ordering test above.
    assert_eq!(format("pkg.tar.gz"), Some(Format::TarGz));
    assert_eq!(format("pkg.tgz"), Some(Format::TarGz));
    assert_eq!(format("pkg.tar.xz"), Some(Format::TarXz));
    assert_eq!(format("pkg.tar.bz2"), Some(Format::TarBz2));
    assert_eq!(format("pkg.tar.zst"), Some(Format::TarZst));

    // A single stream is not a tar, and `access.log.gz` is the case that matters: the `.log` must
    // not make it anything else.
    assert_eq!(format("access.log.gz"), Some(Format::Gz));
    assert_eq!(format("core.xz"), Some(Format::Xz));

    // Developer packages, which people open to look inside.
    assert_eq!(format("lib.jar"), Some(Format::Zip));
    assert_eq!(format("thing-1.0-py3-none-any.whl"), Some(Format::Zip));
    assert_eq!(format("Package.nupkg"), Some(Format::Zip));

    assert_eq!(format("notes.txt"), None);
    assert_eq!(format("Makefile"), None);
    // A dotfile called `zip`, not an archive: there is nothing in front of the suffix.
    assert_eq!(format(".zip"), None);
}

/// **A document that happens to be a zip must open as a document.**
///
/// `.docx`, `.xlsx`, `.pptx`, `.odt` and `.epub` are all zip containers, and every one of them is
/// left out of [`SUFFIXES`] on purpose — a file manager that answered a double-clicked Word file
/// with a listing of `word/document.xml` would be broken rather than clever. This is the assertion
/// that says so, because the temptation to "just add the zip family" is exactly how it would be
/// undone.
#[test]
fn a_document_that_is_secretly_a_zip_is_not_an_archive() {
    for name in [
        "report.docx",
        "budget.xlsx",
        "deck.pptx",
        "notes.odt",
        "sheet.ods",
        "book.epub",
    ] {
        assert_eq!(
            format_of(name),
            None,
            "{name} is a document: double-clicking it must open its application"
        );
    }
}

/// The formats nothing here can read stay unrecognised, so they go on opening with whatever the
/// machine has. See the dependency block in `Cargo.toml`, which turned RAR down on licence grounds.
#[test]
fn the_formats_this_program_cannot_read_are_not_claimed() {
    for name in ["disc.iso", "old.rar", "setup.cab", "mac.dmg", "comic.cbr"] {
        assert_eq!(format_of(name), None, "{name} has no reader here");
    }
}

#[test]
fn a_path_splits_at_the_archive() {
    let at = |text: &str| split(Path::new(text));

    // The archive itself: a root, and the interior is empty.
    let root = at(r"D:\dl\pkg.zip").expect("an archive");
    assert_eq!(root.file, PathBuf::from(r"D:\dl\pkg.zip"));
    assert_eq!(root.within, "");
    assert!(root.is_root());
    assert!(browsable(Path::new(r"D:\dl\pkg.zip")));

    // Inside it. The interior is `/`-separated whatever the path used.
    let deep = at(r"D:\dl\pkg.tar.gz\src\ui\main.rs").expect("an archive");
    assert_eq!(deep.file, PathBuf::from(r"D:\dl\pkg.tar.gz"));
    assert_eq!(deep.within, "src/ui/main.rs");
    assert_eq!(deep.format, Format::TarGz);
    assert!(!deep.is_root());
    // Browsable is about the archive, not about a file in it.
    assert!(!browsable(Path::new(r"D:\dl\pkg.tar.gz\src\ui\main.rs")));
    // **And `is_virtual` is not asked here**, though it once was. It is the write guards' question
    // and it needs more than an extension: nothing on `D:\dl` exists, so no archive has been read
    // there and the honest answer is `false`. See
    // [`a_real_folder_named_like_an_archive_can_still_be_written_to`], which is where that half is
    // tested, against archives and folders that are really on the disk.
    assert!(!is_virtual(Path::new(r"D:\dl\pkg.tar.gz\src\ui\main.rs")));

    // A UNC path: the machine and the share are a prefix, not names, and must not be searched for
    // an extension.
    let unc = at(r"\\server\share\pkg.zip\readme").expect("an archive");
    assert_eq!(unc.file, PathBuf::from(r"\\server\share\pkg.zip"));
    assert_eq!(unc.within, "readme");

    // Nothing in it at all.
    assert!(at(r"D:\Sources\project\src").is_none());
    assert!(at("").is_none());
    assert!(!is_virtual(Path::new(r"D:\Sources")));
}

/// The **first** archive wins, so an archive inside an archive names a file rather than being
/// descended into twice.
///
/// Which is the truth about the format — the inner one has to be inflated somewhere before it can
/// be read — and is what makes nesting work through extraction instead. See [`split`].
#[test]
fn a_nested_archive_is_a_file_and_not_a_second_level() {
    let nested = split(Path::new(r"D:\a.zip\inner.zip\readme")).expect("an archive");
    assert_eq!(nested.file, PathBuf::from(r"D:\a.zip"));
    assert_eq!(
        nested.within, "inner.zip/readme",
        "the inner archive is an entry of the outer one"
    );
}

/// Up out of an archive works with no code of its own, which is the claim the module doc makes
/// about addressing an interior by extending the path. This is that claim, asserted.
#[test]
fn up_walks_out_of_an_archive_by_itself() {
    let up = |text: &str| crate::fs::parent_of(Path::new(text)).expect("a parent");

    assert_eq!(up(r"D:\dl\pkg.zip\src\ui"), PathBuf::from(r"D:\dl\pkg.zip\src"));
    assert_eq!(up(r"D:\dl\pkg.zip\src"), PathBuf::from(r"D:\dl\pkg.zip"));
    // And out of the archive, into the folder holding it.
    assert_eq!(up(r"D:\dl\pkg.zip"), PathBuf::from(r"D:\dl"));
}

// ---------------------------------------------------------------------------
// Not trusting a name
// ---------------------------------------------------------------------------

/// An entry cannot name anything outside the archive. The table in [`read::interior`]'s doc, as a
/// test — this is the security boundary, and the row that matters is the third.
#[test]
fn an_entry_name_cannot_climb_out() {
    let clean = |raw: &str| read::interior(raw);

    assert_eq!(clean("src/main.rs").as_deref(), Some("src/main.rs"));
    assert_eq!(clean("./src/main.rs").as_deref(), Some("src/main.rs"));
    assert_eq!(clean("/etc/passwd").as_deref(), Some("etc/passwd"));
    assert_eq!(
        clean(r"..\..\Windows\System32\evil.dll").as_deref(),
        Some("Windows/System32/evil.dll"),
        "zip slip: the `..` must be dropped, never followed"
    );
    assert_eq!(
        clean(r"C:\Windows\evil.dll").as_deref(),
        Some("Windows/evil.dll"),
        "a stored absolute path loses its drive"
    );
    assert_eq!(clean("src//deep///file").as_deref(), Some("src/deep/file"));
    assert_eq!(clean("dir/").as_deref(), Some("dir"));

    // Nothing left once the above is applied.
    for nothing in ["..", ".", "/", "", "../../..", r"C:\"] {
        assert_eq!(clean(nothing), None, "{nothing:?} names nothing");
    }

    // A trailing dot or space cannot exist on Windows: the shell trims both, so a listing that
    // showed them would name a file that extraction could not produce.
    assert_eq!(clean("name. ").as_deref(), Some("name"));
    assert_eq!(clean("dir /file").as_deref(), Some("dir/file"));
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

#[test]
fn a_zip_lists_its_entries() {
    let root = work("zip-listing");
    let pkg = root.join("pkg.zip");
    zip_at(
        &pkg,
        &[
            ("readme.txt", "hello"),
            ("src/main.rs", "fn main() {}"),
            ("src/ui/mod.rs", "// ui"),
            ("docs/guide.md", "# guide"),
        ],
    );

    // The archive's own root.
    let top = listing_of(&pkg);
    assert!(top.error.is_none(), "{:?}", top.error);
    let mut rows = names(&top);
    rows.sort_unstable();
    assert_eq!(rows, ["docs", "readme.txt", "src"]);
    assert_eq!(top.dir_count, 2, "src and docs are folders");
    assert_eq!(top.file_count, 1);

    // One level in, and `ui` is a folder no entry of this archive mentions by itself.
    let src = listing_of(&pkg.join("src"));
    let mut rows = names(&src);
    rows.sort_unstable();
    assert_eq!(
        rows,
        ["main.rs", "ui"],
        "`ui` has to be inferred from `src/ui/mod.rs`"
    );

    // And the file's size is the uncompressed one, which is what a listing means by size.
    let at = (0..src.len()).find(|&i| src.name(i) == "main.rs").expect("a row");
    assert_eq!(src.entries[at].size, "fn main() {}".len() as u64);
    assert!(!src.entries[at].is_dir());
    // Everything in an archive is read-only, which is what the rename and delete guards read.
    assert!(src.entries[at].flags & crate::fs::dir::FLAG_READONLY != 0);
}

/// A folder that the archive *does* store an entry for must not appear twice beside the one
/// inferred from its children.
#[test]
fn a_stored_directory_and_an_inferred_one_are_one_row() {
    let root = work("zip-stored-dirs");
    let pkg = root.join("pkg.zip");
    zip_at(
        &pkg,
        &[
            ("src/", ""),
            ("src/main.rs", "fn main() {}"),
            ("src/ui/", ""),
            ("src/ui/mod.rs", "// ui"),
        ],
    );

    let top = listing_of(&pkg);
    assert_eq!(names(&top), ["src"], "one row, not two");
    assert_eq!(top.dir_count, 1);

    let src = listing_of(&pkg.join("src"));
    let mut rows = names(&src);
    rows.sort_unstable();
    assert_eq!(rows, ["main.rs", "ui"]);
    let ui = (0..src.len()).find(|&i| src.name(i) == "ui").expect("a row");
    assert!(src.entries[ui].is_dir(), "the stored entry is still a folder");
}

/// A `.tar.gz` has no index at all, so this is the case where listing means inflating — and where
/// the [`CACHE`] earns its place.
#[test]
fn a_tarball_lists_and_is_read_only_once() {
    // [`CACHE`] is one static shared by every test in this process, so the count below has to start
    // from a known place rather than from whatever the test before this one left.
    forget_all();

    let root = work("targz-listing");
    let pkg = root.join("pkg.tar.gz");
    targz_at(
        &pkg,
        &[
            ("pkg/README", "read me"),
            ("pkg/src/lib.rs", "// lib"),
            ("pkg/src/bin/main.rs", "fn main() {}"),
        ],
    );

    // A tar of a single top-level folder, which is what a source download is.
    let top = listing_of(&pkg);
    assert!(top.error.is_none(), "{:?}", top.error);
    assert_eq!(names(&top), ["pkg"]);

    let inner = listing_of(&pkg.join("pkg"));
    let mut rows = names(&inner);
    rows.sort_unstable();
    assert_eq!(rows, ["README", "src"]);

    // The date came out of the tar header rather than being left at zero, which would draw as a
    // dash on every row.
    let readme = (0..inner.len())
        .find(|&i| inner.name(i) == "README")
        .expect("a row");
    assert!(
        inner.entries[readme].modified > 0,
        "a tar records an mtime and the listing has to carry it"
    );

    // Three folders of one archive listed, and **one** archive held: the second and third came out
    // of the index the first read. Without that, each click would inflate the whole file again.
    let deeper = listing_of(&pkg.join("pkg").join("src"));
    let (archives, entries) = held();
    assert_eq!(
        archives, 1,
        "three listings of one tarball must be one read of it"
    );
    assert_eq!(entries, 3, "the index holds every entry, once");

    // And the read is charged **once**, to the listing that paid for it. A later folder reporting
    // the inflate again would claim the first click's cost for ever, which is the wrong lesson to
    // teach about a cache that is working.
    assert!(
        deeper.scan_micros < top.scan_micros,
        "the first listing pays for the inflate ({} µs) and a later one does not ({} µs)",
        top.scan_micros,
        deeper.scan_micros
    );
}

/// A lone `.gz` is a file, not a folder of files: one row, named without the suffix, sized from the
/// footer.
#[test]
fn a_single_stream_lists_as_the_file_it_holds() {
    let root = work("gz-single");
    let log = root.join("access.log.gz");
    let body = "GET / 200\n".repeat(500);
    {
        let file = std::fs::File::create(&log).expect("sandbox");
        let mut gz = flate2::write::GzEncoder::new(file, flate2::Compression::fast());
        gz.write_all(body.as_bytes()).expect("gz");
        gz.finish().expect("gz");
    }

    let listing = listing_of(&log);
    assert!(listing.error.is_none(), "{:?}", listing.error);
    assert_eq!(
        names(&listing),
        ["access.log"],
        "the `.gz` comes off and what is left is the file"
    );
    // The gzip footer states the uncompressed length, so this one is not blank.
    assert_eq!(listing.entries[0].size, body.len() as u64);
    assert!(!listing.entries[0].is_unsized());
}

/// The formats whose container does not record what comes out of them get a **blank** size cell
/// rather than a confident `0 B`. See [`crate::fs::dir::FLAG_UNSIZED`].
#[test]
fn a_stream_with_no_recorded_size_says_so_rather_than_claiming_zero() {
    let root = work("bz2-unsized");
    let path = root.join("notes.bz2");
    {
        let file = std::fs::File::create(&path).expect("sandbox");
        let mut bz = bzip2::write::BzEncoder::new(file, bzip2::Compression::fast());
        bz.write_all(b"some notes").expect("bz2");
        bz.finish().expect("bz2");
    }

    let listing = listing_of(&path);
    assert_eq!(names(&listing), ["notes"]);
    assert!(
        listing.entries[0].is_unsized(),
        "a bzip2 stream records only what went in"
    );
    // Which is what makes the cell blank: the one function every size cell asks.
    let measure = crate::sizes::Measurement::default();
    assert_eq!(
        measure.shown(&listing, 0),
        None,
        "an unsized entry draws no size, exactly as a folder does"
    );
}

/// A folder that merely *looks* like an archive is still a folder.
///
/// [`split`] answers by extension and never touches the disk, so this is the one wrong answer it
/// can give — and [`listing`] is where it is caught, with the single stat it can afford.
#[test]
fn a_folder_named_like_an_archive_is_listed_as_a_folder() {
    let root = work("folder-named-zip");
    let pretend = root.join("stuff.zip");
    std::fs::create_dir_all(&pretend).expect("sandbox");
    std::fs::write(pretend.join("inside.txt"), b"real file").expect("sandbox");

    // The guess is that it is an archive...
    assert!(browsable(&pretend), "by name alone, this looks like one");

    // ...and the listing knows better.
    let listing = listing_of(&pretend);
    assert!(listing.error.is_none(), "{:?}", listing.error);
    assert_eq!(names(&listing), ["inside.txt"]);
    assert!(
        crate::archive::listing(&pretend, std::time::Instant::now()).is_none(),
        "a directory is not this module's to answer for"
    );
}

/// A file that is not the archive it claims to be comes back as a listing that says so, rather than
/// as a panic or an empty folder.
#[test]
fn a_broken_archive_is_a_listing_that_says_why() {
    let root = work("broken");
    let pkg = root.join("pkg.zip");
    std::fs::write(&pkg, b"this is not a zip file, it is a sentence").expect("sandbox");

    let listing = listing_of(&pkg);
    assert!(listing.is_empty());
    assert!(
        listing.error.is_some(),
        "an unreadable archive has to explain itself"
    );
}

// ---------------------------------------------------------------------------
// Getting the bytes out
// ---------------------------------------------------------------------------

#[test]
fn a_file_is_extracted_with_its_contents_and_left_read_only() {
    let root = work("extract-zip");
    let pkg = root.join("pkg.zip");
    zip_at(&pkg, &[("src/main.rs", "fn main() { }"), ("readme.txt", "hi")]);

    let real = extracted(&pkg.join("src").join("main.rs")).expect("extraction");
    assert!(real.is_file(), "{} should exist", real.display());
    assert_eq!(
        std::fs::read_to_string(&real).expect("read back"),
        "fn main() { }"
    );
    assert!(
        real.starts_with(extract::temp_root()),
        "everything extracted lands under the temp root: {}",
        real.display()
    );
    assert!(
        std::fs::metadata(&real).expect("metadata").permissions().readonly(),
        "an extracted copy is read-only, so editing it cannot look like editing the archive"
    );

    // Asking again is not a second extraction, and does not trip over the read-only file it left.
    let again = extracted(&pkg.join("src").join("main.rs")).expect("extraction");
    assert_eq!(again, real);
}

/// The archive itself is already a real file, so nothing is read and nothing is written — which is
/// what makes [`extracted`] safe to call on a path that has not been checked.
#[test]
fn extracting_the_archive_itself_is_the_archive_itself() {
    let root = work("extract-root");
    let pkg = root.join("pkg.zip");
    zip_at(&pkg, &[("a.txt", "a")]);
    assert_eq!(extracted(&pkg).expect("the archive"), pkg);
    // And a path with no archive in it is its own answer too.
    let plain = root.join("a.txt");
    std::fs::write(&plain, b"a").expect("sandbox");
    assert_eq!(extracted(&plain).expect("a real file"), plain);
}

/// A folder is extracted with everything under it, which is what copying one out has to mean.
#[test]
fn a_folder_is_extracted_with_its_contents() {
    let root = work("extract-folder");
    let pkg = root.join("pkg.zip");
    zip_at(
        &pkg,
        &[
            ("src/main.rs", "fn main() {}"),
            ("src/ui/mod.rs", "// ui"),
            ("elsewhere.txt", "not this one"),
        ],
    );

    let folder = extracted(&pkg.join("src")).expect("extraction");
    assert!(folder.is_dir(), "{} should be a folder", folder.display());
    assert_eq!(
        std::fs::read_to_string(folder.join("main.rs")).expect("read back"),
        "fn main() {}"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("ui").join("mod.rs")).expect("read back"),
        "// ui"
    );
    assert!(
        !folder.parent().expect("a parent").join("elsewhere.txt").exists(),
        "only what was asked for comes out"
    );
}

/// **Zip slip**: an entry that names its way out of the archive is written *inside* the temp root
/// regardless.
///
/// The most important test in this module. `interior` is what prevents it and `place` checks the
/// result anyway; this asserts the two together, through the real extraction path.
#[test]
fn an_entry_that_names_its_way_out_is_written_inside_anyway() {
    let root = work("zip-slip");
    let pkg = root.join("evil.zip");
    // Written with the raw name the attack uses. `zip` will not sanitise it for us — which is the
    // point: this is what a hostile archive looks like on disk.
    {
        let file = std::fs::File::create(&pkg).expect("sandbox");
        let mut writer = zip::ZipWriter::new(file);
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer
            .start_file(r"../../../../evil.txt", stored)
            .expect("zip");
        writer.write_all(b"pwned").expect("zip");
        writer.finish().expect("zip");
    }

    // It lists under a harmless name, with the climbing removed.
    let listing = listing_of(&pkg);
    assert_eq!(names(&listing), ["evil.txt"]);

    let real = extracted(&pkg.join("evil.txt")).expect("extraction");
    assert!(
        real.starts_with(extract::temp_root()),
        "a `..` in an entry name must not escape the temp root: {}",
        real.display()
    );
    // And nothing landed where the archive was aiming.
    assert!(
        !extract::temp_root()
            .parent()
            .expect("a parent")
            .join("evil.txt")
            .exists(),
        "the attack's target must be untouched"
    );
}

/// A `.7z` read end to end, which is the format with no other pure-Rust implementation and the one
/// this test module needs a dev-dependency to be able to write at all.
#[test]
fn a_seven_zip_lists_and_extracts() {
    let root = work("sevenz");
    let pkg = root.join("pkg.7z");

    {
        let file = std::fs::File::create(&pkg).expect("sandbox");
        let mut writer = sevenz_rust2::ArchiveWriter::new(file).expect("7z");
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file("src/main.rs"),
                Some(&b"fn main() {}"[..]),
            )
            .expect("7z");
        writer
            .push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file("readme.txt"),
                Some(&b"hello"[..]),
            )
            .expect("7z");
        writer.finish().expect("7z");
    }

    let top = listing_of(&pkg);
    assert!(top.error.is_none(), "{:?}", top.error);
    let mut rows = names(&top);
    rows.sort_unstable();
    assert_eq!(rows, ["readme.txt", "src"]);

    // A `.7z` groups entries into solid blocks, so extraction goes through the block walk rather
    // than a seek — and matches by name, the block order not being the file order.
    let real = extracted(&pkg.join("src").join("main.rs")).expect("extraction");
    assert_eq!(
        std::fs::read_to_string(&real).expect("read back"),
        "fn main() {}"
    );
}

/// The flatten button inside an archive gives every entry under the folder, at every depth, with the
/// relative paths a flattened listing is made of.
///
/// Worth its own test because the flatten goes through a *different* entry point —
/// [`crate::fs::scan::scan_deep`], which for a real folder is a threaded tree walk — and an archive
/// that listed correctly but flattened to nothing would look like the button was broken.
#[test]
fn flattening_inside_an_archive_gives_the_whole_tree() {
    let root = work("flatten");
    let pkg = root.join("pkg.zip");
    zip_at(
        &pkg,
        &[
            ("README", "read me"),
            ("src/main.rs", "fn main() {}"),
            ("src/ui/mod.rs", "// ui"),
            ("src/ui/list.rs", "// list"),
        ],
    );

    let flat = crate::fs::scan::scan_deep(&pkg, crate::fs::scan::FLATTEN_BUDGET, PATIENCE_FOR_TESTS);
    let mut rows = names(&flat);
    rows.sort_unstable();
    assert_eq!(
        rows,
        [
            "README",
            r"src",
            r"src\main.rs",
            r"src\ui",
            r"src\ui\list.rs",
            r"src\ui\mod.rs",
        ],
        "every entry and every folder on the way to it, `\\`-separated as a flat listing stores them"
    );

    // Which is what the tree view indents by, and what the Name column shows dimmed after the name.
    let deep = (0..flat.len())
        .find(|&i| flat.name(i) == r"src\ui\mod.rs")
        .expect("a row");
    assert_eq!(flat.leaf(deep), "mod.rs");
    assert_eq!(flat.within(deep), r"src\ui");
    assert_eq!(flat.depth(deep), 2);
}

/// The patience a flatten is given. Irrelevant inside an archive — there is no walk to bound, the
/// index having been read already — and required by the signature.
const PATIENCE_FOR_TESTS: std::time::Duration = std::time::Duration::from_secs(5);

/// **A solid `.7z`, extracting one entry from the middle and one from the end.**
///
/// The regression test for the worst bug this feature had. A solid block is one compressed stream
/// with the entries laid end to end, and `sevenz-rust2` hands each one a reader bounded to its own
/// length over that shared stream — **without skipping what a caller does not read**. So an
/// extraction that ignored the entries it did not want left the stream short by their combined
/// length, and the next entry it did want was read from the wrong offset. The CRC in the format is
/// the only reason that surfaced as `ChecksumVerificationFailed` rather than as silently wrong bytes.
///
/// Found on a real 144 MB SDK archive whose `info.yml` and `version` sit at the end of a 363-entry
/// solid block, and which now extracts all 371 of its files byte-for-byte identically to 7-Zip.
///
/// Twelve entries with distinct contents, because the bug needs *preceding* entries to go unread:
/// a one-entry archive passes either way, which is exactly why the first round of tests missed it.
#[test]
fn an_entry_late_in_a_solid_block_extracts() {
    let root = work("sevenz-solid");
    let pkg = root.join("pkg.7z");

    let bodies: Vec<(String, String)> = (0..12)
        .map(|at| {
            (
                format!("file{at:02}.txt"),
                // Distinct per entry and long enough that a misaligned read cannot coincidentally
                // land on the right bytes.
                format!("contents of entry number {at}, repeated. ").repeat(40),
            )
        })
        .collect();

    {
        let file = std::fs::File::create(&pkg).expect("sandbox");
        let mut writer = sevenz_rust2::ArchiveWriter::new(file).expect("7z");
        // `push_archive_entries` is the solid one — one pack for all of them, which is what
        // `push_archive_entry` in a loop would *not* give and what the bug needs.
        let entries: Vec<sevenz_rust2::ArchiveEntry> = bodies
            .iter()
            .map(|(name, _)| sevenz_rust2::ArchiveEntry::new_file(name))
            .collect();
        let readers: Vec<sevenz_rust2::SourceReader<&[u8]>> = bodies
            .iter()
            .map(|(_, body)| sevenz_rust2::SourceReader::new(body.as_bytes()))
            .collect();
        writer
            .push_archive_entries(entries, readers)
            .expect("solid 7z");
        writer.finish().expect("7z");
    }

    // It really is one block, or this test is not about what it says it is.
    let inside = split(&pkg).expect("an archive");
    let (index, _) = index(&inside.file, inside.format);
    assert_eq!(index.items.len(), 12, "{:?}", index.error);

    // The last entry, which needs every one of the eleven before it to have been read through.
    let last = &bodies[11];
    let real = extracted(&pkg.join(&last.0)).expect("the last entry should extract");
    assert_eq!(
        std::fs::read_to_string(&real).expect("read back"),
        last.1,
        "an entry at the end of a solid block must not be read from the wrong offset"
    );

    // And one from the middle, asked for on its own — a fresh walk that stops early rather than
    // reading to the end of the block.
    let middle = &bodies[6];
    let real = extracted(&pkg.join(&middle.0)).expect("the middle entry should extract");
    assert_eq!(std::fs::read_to_string(&real).expect("read back"), middle.1);
}

/// A whole selection out of one archive is **one** walk of it, not one per file.
///
/// Which is a correctness property as much as a performance one: for a solid `.7z` each separate
/// read decompresses the block from its start, so asking file by file turns a copy of a few hundred
/// entries into a few hundred walks — and a pane that appears to have hung. See
/// [`extract::all`].
#[test]
fn a_whole_selection_is_extracted_together() {
    let root = work("select-together");
    let pkg = root.join("pkg.zip");
    zip_at(
        &pkg,
        &[
            ("one.txt", "first"),
            ("two.txt", "second"),
            ("sub/three.txt", "third"),
            ("elsewhere.txt", "not asked for"),
        ],
    );

    // A folder and two files together, which is an ordinary selection — and note `sub` overlaps
    // nothing, while asking for both a folder and a file inside it is what the dedupe covers.
    let picked = vec![pkg.join("one.txt"), pkg.join("two.txt"), pkg.join("sub")];
    let done = extract::all(&picked).expect("extraction");

    assert_eq!(done.len(), 3, "one answer per path, in the order asked");
    assert_eq!(
        std::fs::read_to_string(&done[0]).expect("read back"),
        "first"
    );
    assert_eq!(
        std::fs::read_to_string(&done[1]).expect("read back"),
        "second"
    );
    assert!(done[2].is_dir(), "the third was a folder");
    assert_eq!(
        std::fs::read_to_string(done[2].join("three.txt")).expect("read back"),
        "third"
    );
    assert!(
        !done[0].with_file_name("elsewhere.txt").exists(),
        "only what was asked for comes out"
    );
}

/// Asking for a folder **and** a file inside it must not extract that file twice — which for a solid
/// block would read it a second time from the wrong place.
#[test]
fn an_overlapping_selection_extracts_each_entry_once() {
    let root = work("overlap");
    let pkg = root.join("pkg.zip");
    zip_at(&pkg, &[("sub/inner.txt", "inner"), ("sub/other.txt", "other")]);

    let picked = vec![pkg.join("sub"), pkg.join("sub").join("inner.txt")];
    let done = extract::all(&picked).expect("extraction");
    assert_eq!(done.len(), 2);
    assert!(done[0].is_dir());
    assert_eq!(
        std::fs::read_to_string(&done[1]).expect("read back"),
        "inner"
    );
    assert_eq!(
        std::fs::read_to_string(done[0].join("other.txt")).expect("read back"),
        "other"
    );
}

/// **A real folder named `stuff.zip` must not be treated as read-only.**
///
/// [`is_virtual`] is what every write guard in the program asks — the delete, the rename, the paste,
/// the clipboard, the shell menu — and [`split`] answers by extension without touching the disk. So
/// the question needs a second half, or a folder somebody happened to call `stuff.zip` becomes a
/// folder in which Delete silently refuses and no context menu comes up. Which is not a cosmetic
/// slip: it is a folder the user can no longer work in.
#[test]
fn a_real_folder_named_like_an_archive_can_still_be_written_to() {
    let root = work("real-folder-guard");
    let pretend = root.join("stuff.zip");
    std::fs::create_dir_all(&pretend).expect("sandbox");
    std::fs::write(pretend.join("inside.txt"), b"real file").expect("sandbox");

    // The listing is right about it — that much was already true.
    assert_eq!(names(&listing_of(&pretend)), ["inside.txt"]);

    // And so are the guards, which is the part that needed the second half.
    assert!(
        !is_virtual(&pretend),
        "a real directory is not an archive, whatever it is called"
    );
    assert!(
        !is_virtual(&pretend.join("inside.txt")),
        "nor is a real file inside one"
    );

    // While a real archive, once listed, is.
    let pkg = root.join("real.zip");
    zip_at(&pkg, &[("a.txt", "a")]);
    let _ = listing_of(&pkg);
    assert!(is_virtual(&pkg), "an archive that has been read is virtual");
    assert!(is_virtual(&pkg.join("a.txt")));
}

/// A rewritten archive is not served out of the cache, which is what F5 depends on.
#[test]
fn rewriting_an_archive_is_noticed() {
    let root = work("rewritten");
    let pkg = root.join("pkg.zip");
    zip_at(&pkg, &[("first.txt", "one")]);
    assert_eq!(names(&listing_of(&pkg)), ["first.txt"]);

    // The key is the path, the size and the timestamp — so the new archive has to differ in at
    // least one, and a different set of entries differs in size.
    zip_at(&pkg, &[("second.txt", "two"), ("third.txt", "three")]);
    forget(&pkg);

    let fresh = listing_of(&pkg);
    let mut rows = names(&fresh);
    rows.sort_unstable();
    assert_eq!(
        rows,
        ["second.txt", "third.txt"],
        "F5 has to re-read the archive, not the index of the old one"
    );
}
