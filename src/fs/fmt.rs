//! Turning raw values into the text the details view shows.
//!
//! Every function here writes into a caller-owned buffer rather than returning a
//! `String`. The list re-formats its visible rows every frame — that is what
//! makes scrolling correct without a cache to invalidate — so the per-frame cost
//! has to be zero allocations. egui's galley cache is keyed on the finished text,
//! so a row that has not changed is not laid out twice either.

use std::fmt::Write as _;

use super::time::{DateTime, LocalZone};

/// Write a byte count in the largest unit that leaves a number worth reading.
///
/// Explorer rounds everything to whole kilobytes, which turns every small file
/// into "1 KB" and loses the distinction between a 40-byte stub and a 900-byte
/// one. This keeps three significant figures instead: `847 B`, `9.34 KB`,
/// `72.1 MB`, `1.42 GB`.
pub fn size(bytes: u64, out: &mut String) {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];

    if bytes < 1024 {
        let _ = write!(out, "{bytes} B");
        return;
    }

    // Decimal place chosen so the number is always 3-4 characters wide, which is
    // what keeps the column from jumping as you scroll.
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    let suffix = UNITS[unit];
    let _ = if value < 10.0 {
        write!(out, "{value:.2} {suffix}")
    } else if value < 100.0 {
        write!(out, "{value:.1} {suffix}")
    } else {
        write!(out, "{value:.0} {suffix}")
    };
}

/// `dd/MM/yyyy HH:mm` — fixed width, so the Modified column can be measured once
/// and never re-measured.
///
/// Not locale-aware: reading the user's short-date pattern out of the registry
/// means `GetDateFormatEx` per cell, and a column whose width depends on the
/// month name is a column that reflows. One unambiguous fixed format is the
/// better trade for a dense table.
pub fn date(dt: DateTime, out: &mut String) {
    let _ = write!(
        out,
        "{:02}/{:02}/{:04} {:02}:{:02}",
        dt.day, dt.month, dt.year, dt.hour, dt.minute
    );
}

/// The widest string [`date`] can produce, for measuring the column.
pub const DATE_TEMPLATE: &str = "00/00/0000 00:00";

/// Write the Modified cell for a raw `FILETIME`, or a dash when there is no date.
pub fn modified(filetime: u64, zone: &LocalZone, out: &mut String) {
    match zone.convert(filetime) {
        Some(dt) => date(dt, out),
        None => out.push('—'),
    }
}

// ---------------------------------------------------------------------------
// Type names
// ---------------------------------------------------------------------------

/// Broad kinds, for choosing an icon and a colour without a second lookup.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Folder,
    Image,
    Audio,
    Video,
    Archive,
    Code,
    Document,
    Executable,
    Font,
    Model,
    Data,
    Other,
}

/// The type label and kind for an extension.
///
/// Explorer gets this from the registry, via `SHGetFileInfo` with `SHGFI_TYPENAME`
/// — a `HKEY_CLASSES_ROOT` walk per *file*. [`crate::fs::scan`]'s benchmark measures
/// that at over a millisecond each, which is 67 seconds for a folder of 60,000 and
/// the single biggest reason a folder of mixed files can take that long to appear.
/// This is a sorted table and a binary search: 45 nanoseconds, same answer.
///
/// The extension is matched case-insensitively. Anything unknown becomes
/// `"XYZ file"`, which is what the registry would have said anyway.
pub fn file_type(ext: &str) -> Option<(&'static str, Kind)> {
    if ext.is_empty() || ext.len() > MAX_EXT {
        return None;
    }
    // Lowercase into a stack buffer: no allocation, and the table is all ASCII so
    // a non-ASCII extension simply will not match, which is correct.
    let mut key = [0u8; MAX_EXT];
    for (slot, byte) in key.iter_mut().zip(ext.as_bytes()) {
        *slot = byte.to_ascii_lowercase();
    }
    let key = &key[..ext.len()];

    TYPES
        .binary_search_by(|(candidate, _, _)| candidate.as_bytes().cmp(key))
        .ok()
        .map(|i| (TYPES[i].1, TYPES[i].2))
}

/// The kind of an entry, for its icon.
pub fn kind_of(ext: &str, is_dir: bool) -> Kind {
    if is_dir {
        return Kind::Folder;
    }
    file_type(ext).map_or(Kind::Other, |(_, kind)| kind)
}

/// Write the Type cell.
pub fn type_label(ext: &str, is_dir: bool, out: &mut String) {
    if is_dir {
        out.push_str("File folder");
        return;
    }
    match file_type(ext) {
        Some((label, _)) => out.push_str(label),
        None if ext.is_empty() => out.push_str("File"),
        None => {
            // `PNG file`, the shell's own fallback shape. Upper-cased because an
            // extension is a name here, not a word.
            for c in ext.chars().take(MAX_EXT) {
                for upper in c.to_uppercase() {
                    out.push(upper);
                }
            }
            out.push_str(" file");
        }
    }
}

/// Longest extension the table holds, and the cap on what a fallback label will
/// echo back.
const MAX_EXT: usize = 12;

/// Extension, label, kind — **sorted by extension**, which [`file_type`]'s binary
/// search depends on. A `debug_assert` in the tests below keeps it that way.
///
/// The labels follow what Windows itself installs, so the column reads the same as
/// Explorer's for everything common.
#[rustfmt::skip]
static TYPES: &[(&str, &str, Kind)] = &[
    ("7z",    "7-Zip archive",            Kind::Archive),
    ("aac",   "AAC audio",                Kind::Audio),
    ("ai",    "Adobe Illustrator file",   Kind::Image),
    ("aiff",  "AIFF audio",               Kind::Audio),
    ("apk",   "Android package",          Kind::Archive),
    ("asm",   "Assembly source",          Kind::Code),
    ("avi",   "AVI video",                Kind::Video),
    ("avif",  "AVIF image",               Kind::Image),
    ("bat",   "Windows batch file",       Kind::Executable),
    ("bin",   "Binary file",              Kind::Data),
    ("blend", "Blender scene",            Kind::Model),
    ("bmp",   "Bitmap image",             Kind::Image),
    ("bz2",   "bzip2 archive",            Kind::Archive),
    ("c",     "C source",                 Kind::Code),
    ("cab",   "Cabinet archive",          Kind::Archive),
    ("cbr",   "Comic book archive",       Kind::Archive),
    ("cc",    "C++ source",               Kind::Code),
    ("cfg",   "Configuration file",       Kind::Code),
    ("cmake", "CMake script",             Kind::Code),
    ("cmd",   "Windows command script",   Kind::Executable),
    ("com",   "MS-DOS application",       Kind::Executable),
    ("cpp",   "C++ source",               Kind::Code),
    ("cs",    "C# source",                Kind::Code),
    ("css",   "Cascading style sheet",    Kind::Code),
    ("csv",   "Comma separated values",   Kind::Data),
    ("cur",   "Cursor",                   Kind::Image),
    ("cxx",   "C++ source",               Kind::Code),
    ("dae",   "COLLADA model",            Kind::Model),
    ("dart",  "Dart source",              Kind::Code),
    ("db",    "Database file",            Kind::Data),
    ("dll",   "Application extension",    Kind::Executable),
    ("dmg",   "Apple disk image",         Kind::Archive),
    ("doc",   "Word 97-2003 document",    Kind::Document),
    ("docx",  "Word document",            Kind::Document),
    ("dwg",   "AutoCAD drawing",          Kind::Model),
    ("dxf",   "AutoCAD exchange file",    Kind::Model),
    ("e57",   "E57 point cloud",          Kind::Model),
    ("eot",   "Embedded OpenType font",   Kind::Font),
    ("eps",   "Encapsulated PostScript",  Kind::Image),
    ("epub",  "EPUB book",                Kind::Document),
    ("exe",   "Application",              Kind::Executable),
    ("fbx",   "FBX model",                Kind::Model),
    ("flac",  "FLAC audio",               Kind::Audio),
    ("flv",   "Flash video",              Kind::Video),
    ("fnt",   "Font file",                Kind::Font),
    ("gif",   "GIF image",                Kind::Image),
    ("glb",   "glTF binary model",        Kind::Model),
    ("gltf",  "glTF model",               Kind::Model),
    ("go",    "Go source",                Kind::Code),
    ("gz",    "gzip archive",             Kind::Archive),
    ("h",     "C/C++ header",             Kind::Code),
    ("heic",  "HEIF image",               Kind::Image),
    ("hpp",   "C++ header",               Kind::Code),
    ("htm",   "HTML document",            Kind::Code),
    ("html",  "HTML document",            Kind::Code),
    ("hxx",   "C++ header",               Kind::Code),
    ("ico",   "Icon",                     Kind::Image),
    ("ifc",   "IFC building model",       Kind::Model),
    ("ini",   "Configuration settings",   Kind::Code),
    ("iso",   "Disc image",               Kind::Archive),
    ("java",  "Java source",              Kind::Code),
    ("jfif",  "JPEG image",               Kind::Image),
    ("jpeg",  "JPEG image",               Kind::Image),
    ("jpg",   "JPEG image",               Kind::Image),
    ("js",    "JavaScript file",          Kind::Code),
    ("json",  "JSON file",                Kind::Code),
    ("jsx",   "JavaScript JSX file",      Kind::Code),
    ("kt",    "Kotlin source",            Kind::Code),
    ("las",   "LAS point cloud",          Kind::Model),
    ("laz",   "LAZ point cloud",          Kind::Model),
    ("lib",   "Object library",           Kind::Data),
    ("licx",  "Licences file",            Kind::Data),
    ("lnk",   "Shortcut",                 Kind::Other),
    ("log",   "Log file",                 Kind::Document),
    ("lua",   "Lua source",               Kind::Code),
    ("lz4",   "LZ4 archive",              Kind::Archive),
    ("m4a",   "MPEG-4 audio",             Kind::Audio),
    ("m4v",   "MPEG-4 video",             Kind::Video),
    ("md",    "Markdown document",        Kind::Document),
    ("mid",   "MIDI sequence",            Kind::Audio),
    ("mkv",   "Matroska video",           Kind::Video),
    ("mov",   "QuickTime movie",          Kind::Video),
    ("mp3",   "MP3 audio",                Kind::Audio),
    ("mp4",   "MPEG-4 video",             Kind::Video),
    ("mpg",   "MPEG video",               Kind::Video),
    ("msi",   "Windows installer package", Kind::Executable),
    ("obj",   "Wavefront model",          Kind::Model),
    ("odp",   "OpenDocument presentation", Kind::Document),
    ("ods",   "OpenDocument spreadsheet", Kind::Document),
    ("odt",   "OpenDocument text",        Kind::Document),
    ("ogg",   "Ogg audio",                Kind::Audio),
    ("opus",  "Opus audio",               Kind::Audio),
    ("otf",   "OpenType font",            Kind::Font),
    ("parq",  "Parquet file",             Kind::Data),
    ("pdb",   "Program debug database",   Kind::Data),
    ("pdf",   "PDF document",             Kind::Document),
    ("php",   "PHP source",               Kind::Code),
    ("pl",    "Perl source",              Kind::Code),
    ("ply",   "PLY model",                Kind::Model),
    ("png",   "PNG image",                Kind::Image),
    ("ppt",   "PowerPoint 97-2003",       Kind::Document),
    ("pptx",  "PowerPoint presentation",  Kind::Document),
    ("ps1",   "PowerShell script",        Kind::Executable),
    ("psd",   "Photoshop image",          Kind::Image),
    ("pts",   "PTS point cloud",          Kind::Model),
    ("py",    "Python source",            Kind::Code),
    ("rar",   "RAR archive",              Kind::Archive),
    ("raw",   "Camera raw image",         Kind::Image),
    ("rb",    "Ruby source",              Kind::Code),
    ("rs",    "Rust source",              Kind::Code),
    ("rtf",   "Rich text document",       Kind::Document),
    ("scss",  "Sass style sheet",         Kind::Code),
    ("sh",    "Shell script",             Kind::Executable),
    ("sql",   "SQL script",               Kind::Code),
    ("sqlite","SQLite database",          Kind::Data),
    ("stl",   "STL model",                Kind::Model),
    ("svg",   "SVG image",                Kind::Image),
    ("swift", "Swift source",             Kind::Code),
    ("sys",   "System file",              Kind::Executable),
    ("tar",   "tar archive",              Kind::Archive),
    ("tga",   "Targa image",              Kind::Image),
    ("tif",   "TIFF image",               Kind::Image),
    ("tiff",  "TIFF image",               Kind::Image),
    ("toml",  "TOML file",                Kind::Code),
    ("ts",    "TypeScript file",          Kind::Code),
    ("tsx",   "TypeScript JSX file",      Kind::Code),
    ("ttc",   "TrueType font collection", Kind::Font),
    ("ttf",   "TrueType font",            Kind::Font),
    ("txt",   "Text document",            Kind::Document),
    ("url",   "Internet shortcut",        Kind::Other),
    ("vb",    "Visual Basic source",      Kind::Code),
    ("wasm",  "WebAssembly module",       Kind::Code),
    ("wav",   "Wave audio",               Kind::Audio),
    ("webm",  "WebM video",               Kind::Video),
    ("webp",  "WebP image",               Kind::Image),
    ("wma",   "Windows Media audio",      Kind::Audio),
    ("wmv",   "Windows Media video",      Kind::Video),
    ("woff",  "Web open font",            Kind::Font),
    ("woff2", "Web open font 2",          Kind::Font),
    ("xls",   "Excel 97-2003 worksheet",  Kind::Document),
    ("xlsx",  "Excel worksheet",          Kind::Document),
    ("xml",   "XML document",             Kind::Code),
    ("xz",    "xz archive",               Kind::Archive),
    ("yaml",  "YAML file",                Kind::Code),
    ("yml",   "YAML file",                Kind::Code),
    ("zip",   "Zip archive",              Kind::Archive),
    ("zst",   "Zstandard archive",        Kind::Archive),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sorted_and_ascii() {
        for window in TYPES.windows(2) {
            assert!(
                window[0].0.as_bytes() < window[1].0.as_bytes(),
                "`{}` and `{}` are out of order -- the binary search needs byte order",
                window[0].0,
                window[1].0
            );
        }
        for (ext, _, _) in TYPES {
            assert!(
                ext.is_ascii() && ext.len() <= MAX_EXT,
                "`{ext}` cannot ever be matched: the lookup lowercases ASCII into a {MAX_EXT}-byte buffer"
            );
        }
    }

    #[test]
    fn lookup_ignores_case() {
        assert_eq!(file_type("PNG").unwrap().0, "PNG image");
        assert_eq!(file_type("Rs").unwrap().0, "Rust source");
        assert!(file_type("nope").is_none());
        assert!(file_type("").is_none());
    }

    #[test]
    fn labels_fall_back_to_the_extension() {
        let mut out = String::new();
        type_label("qqq", false, &mut out);
        assert_eq!(out, "QQQ file");

        out.clear();
        type_label("", false, &mut out);
        assert_eq!(out, "File");

        out.clear();
        type_label("anything", true, &mut out);
        assert_eq!(out, "File folder");
    }

    #[test]
    fn sizes_keep_three_figures() {
        let mut out = String::new();
        for (bytes, expected) in [
            (0, "0 B"),
            (847, "847 B"),
            (1024, "1.00 KB"),
            (9_560, "9.34 KB"),
            (75_612_345, "72.1 MB"),
            (1_524_760_000, "1.42 GB"),
            (900 * 1024 * 1024, "900 MB"),
        ] {
            out.clear();
            size(bytes, &mut out);
            assert_eq!(out, expected, "{bytes} bytes");
        }
    }

    #[test]
    fn date_matches_its_template() {
        let mut out = String::new();
        date(
            DateTime {
                year: 2026,
                month: 8,
                day: 2,
                hour: 9,
                minute: 5,
                second: 0,
            },
            &mut out,
        );
        assert_eq!(out, "02/08/2026 09:05");
        assert_eq!(out.len(), DATE_TEMPLATE.len());
    }
}
