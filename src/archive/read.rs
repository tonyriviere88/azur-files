//! Reading an archive's table of contents, once, into an [`Index`].
//!
//! Four shapes of read, and the difference between them is the whole reason [`super::CACHE`]
//! exists:
//!
//! | format | what this does |
//! | --- | --- |
//! | `.zip` | seeks to the central directory and walks it — no entry data is touched |
//! | `.7z` | reads the end header, which is the same kind of directory |
//! | `.tar` and its four wrappings | walks the **entire stream**, because a tar has no index |
//! | `.gz`, `.bz2`, `.xz`, `.zst`, `.lzma` | nothing at all: the listing is one row, named after the file |
//!
//! # Every read here is of a file somebody else wrote
//!
//! Which makes this the most hostile input the program takes, and it is guarded on three sides:
//!
//! - **A panic is caught.** [`index`] wraps the decoders in [`std::panic::catch_unwind`], so a
//!   malformed header that walks a slice off its end becomes a listing that says the archive is
//!   damaged rather than a window that vanishes. The crates below are pure Rust — see `Cargo.toml`,
//!   where that was the rule that chose them — so the failure mode really is a panic and not
//!   something worse.
//! - **The entry count is capped** at [`BUDGET`], because a central directory claiming four billion
//!   files is a 22 KB file.
//! - **The decompressed byte count and the wall clock are capped**, by [`Capped`], because a
//!   decompressor is a machine for turning a small file into a large one and there are only two to
//!   four [`crate::loader`] workers to lose.
//!
//! Nothing here trusts a path either: [`interior`] is what stands between an entry called
//! `..\..\Windows\System32\evil.dll` and [`super::extract`].

use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;
use std::time::{Duration, Instant};

use super::{Format, Index, Item, BUDGET};
use crate::fs::time::{DateTime, LocalZone};

/// How much a single archive read may decompress before it is called a bomb.
///
/// Generous on purpose — a DVD image inside a `.tar.gz` is a real thing somebody keeps — and the
/// point is only that the number exists. Without it a 42 KB gzip that expands to a petabyte
/// occupies a worker until the process ends.
const CAP: u64 = 4 << 30;

/// And how long, for the same reason from the other side: a stream can be slow to inflate without
/// being large.
///
/// A partial listing marked [`Index::truncated`] is the outcome, which the status line reports. The
/// alternative is a pane that never answers, and a folder that is honest about being incomplete is
/// worth more than one that is still hoping.
const PATIENCE: Duration = Duration::from_secs(45);

/// The dictionary memory a `.lzma` may ask for, in KiB.
///
/// A `.lzma` header states its own dictionary size and the decoder allocates from it, so an
/// attacker picks this number unless somebody else does. 768 MiB is past every real encoder's
/// largest setting and short of a number that takes the process down.
const LZMA_MEM_KB: u32 = 768 * 1024;

/// Read the archive's entries.
///
/// Never fails: everything that can go wrong comes back as [`Index::error`], in words meant for the
/// middle of an empty pane.
pub fn index(file: &Path, format: Format) -> Index {
    let started = Instant::now();

    // `AssertUnwindSafe` is sound here because nothing observable is shared with the closure: it
    // takes two `Copy` arguments and returns an owned `Index`, and a panic part way through leaves
    // only a half-built `Vec` that is dropped with it. There is no lock held — [`super::index`]
    // releases the cache before calling this — and no state that could be left inconsistent.
    let read = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match format {
        Format::Zip => zip_index(file),
        Format::SevenZ => sevenz_index(file),
        _ if format.is_tar() => tar_index(file, format),
        _ => stream_index(file, format),
    }));

    let mut index = read.unwrap_or_else(|_| {
        Index::failed("This archive is damaged and could not be read past its header")
    });
    index.micros = started.elapsed().as_micros() as u64;
    index
}

/// A reader that refuses to go on for ever. See [`CAP`] and [`PATIENCE`].
struct Capped<R> {
    inner: R,
    left: u64,
    until: Instant,
    /// Checked every so many reads rather than on each one, because `Instant::now` is a syscall on
    /// some platforms and this sits under a decompressor asking for 8 KB at a time.
    countdown: u32,
}

impl<R: Read> Capped<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            left: CAP,
            until: Instant::now() + PATIENCE,
            countdown: CHECK_EVERY,
        }
    }
}

/// How many reads between clock checks.
const CHECK_EVERY: u32 = 512;

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.left == 0 {
            return Err(io::Error::other("stopped at the size limit"));
        }
        self.countdown = self.countdown.saturating_sub(1);
        if self.countdown == 0 {
            self.countdown = CHECK_EVERY;
            if Instant::now() >= self.until {
                return Err(io::Error::other("stopped at the time limit"));
            }
        }
        let want = buf.len().min(self.left as usize);
        let read = self.inner.read(&mut buf[..want])?;
        self.left -= read as u64;
        Ok(read)
    }
}

/// A decompressed byte stream for the formats that are one — the tar wrappings and the single
/// streams.
///
/// Also what [`super::extract`] pulls a single-stream archive's bytes out of, which is why it is
/// here rather than inline: the listing and the extraction must agree about what a `.tar.xz` is.
///
/// The `Multi` decoders for gzip and bzip2 rather than the plain ones, because concatenating two
/// compressed files is a valid way to make a third and `cat a.gz b.gz > c.gz` is a thing people do
/// to logs. The plain decoders stop at the first member and would silently list half an archive.
pub fn stream(file: &Path, format: Format) -> io::Result<Box<dyn Read>> {
    // 64 KB rather than the default 8, because every one of these decoders reads in small bites and
    // the syscall is the expensive part.
    let handle = BufReader::with_capacity(64 * 1024, File::open(file)?);
    Ok(match format {
        Format::Gz | Format::TarGz => Box::new(flate2::read::MultiGzDecoder::new(handle)),
        Format::Bz2 | Format::TarBz2 => Box::new(bzip2::read::MultiBzDecoder::new(handle)),
        Format::Xz | Format::TarXz => Box::new(lzma_rust2::XzReader::new(handle, true)),
        Format::Zst | Format::TarZst => Box::new(
            ruzstd::decoding::StreamingDecoder::new(handle)
                .map_err(|e| io::Error::other(e.to_string()))?,
        ),
        Format::Lzma => Box::new(lzma_rust2::LzmaReader::new_mem_limit(
            handle,
            LZMA_MEM_KB,
            None,
        )?),
        // A plain `.tar` is already the stream.
        Format::Tar => Box::new(handle),
        // Both have a directory and are read by seeking, not streaming. Unreachable: every caller
        // reaches this through [`Format::is_tar`] or through [`index`]'s final arm, which the two
        // above are matched out of before it.
        Format::Zip | Format::SevenZ => {
            return Err(io::Error::other("this format is not a stream"))
        }
    })
}

// ---------------------------------------------------------------------------
// The formats
// ---------------------------------------------------------------------------

fn zip_index(file: &Path) -> Index {
    let handle = match File::open(file) {
        Ok(handle) => handle,
        Err(why) => return Index::failed(reason(&why)),
    };
    let mut archive = match zip::ZipArchive::new(BufReader::new(handle)) {
        Ok(archive) => archive,
        Err(why) => return Index::failed(format!("This is not a readable zip file ({why})")),
    };

    // Once per archive, for the reason [`LocalZone::filetime_from_local`] exists: a zip stores
    // local wall-clock time and this is what turns it back into the `FILETIME` a row wants.
    let zone = LocalZone::current();
    let mut items = Vec::with_capacity(archive.len().min(BUDGET));
    let mut truncated = false;

    for at in 0..archive.len() {
        if items.len() >= BUDGET {
            truncated = true;
            break;
        }
        // `by_index_raw`, not `by_index`: this wants the central directory's record and not the
        // entry's bytes, and the plain one sets up a decompressor per call to hand back a reader
        // nothing here reads. It also cannot fail on an encrypted entry, which is how those come to
        // be listed at all.
        let Ok(entry) = archive.by_index_raw(at) else {
            continue;
        };
        let Some(path) = interior(entry.name()) else {
            continue;
        };
        let modified = entry
            .last_modified()
            .filter(|stamp| stamp.is_valid())
            .map(|stamp| {
                zone.filetime_from_local(DateTime {
                    year: stamp.year() as i32,
                    month: stamp.month() as u32,
                    day: stamp.day() as u32,
                    hour: stamp.hour() as u32,
                    minute: stamp.minute() as u32,
                    second: stamp.second() as u32,
                })
            })
            .unwrap_or(0);
        items.push(Item {
            path,
            size: entry.size(),
            modified,
            is_dir: entry.is_dir(),
            unsized_: false,
            encrypted: entry.encrypted(),
            link: false,
            at: at as u32,
        });
    }

    Index {
        items,
        error: None,
        truncated,
        micros: 0,
    }
}

fn sevenz_index(file: &Path) -> Index {
    let handle = match File::open(file) {
        Ok(handle) => handle,
        Err(why) => return Index::failed(reason(&why)),
    };
    let mut handle = BufReader::new(handle);
    // The metadata only. A `.7z` keeps its directory in an end header, so this reads no entry data
    // — which matters more here than for a zip, because a `.7z` groups entries into solid blocks
    // and touching one entry can mean decompressing several.
    let archive = match sevenz_rust2::Archive::read(&mut handle, &sevenz_rust2::Password::empty()) {
        Ok(archive) => archive,
        Err(why) => return Index::failed(format!("This 7z archive could not be read ({why})")),
    };

    let mut truncated = false;
    let mut items = Vec::with_capacity(archive.files.len().min(BUDGET));
    for (at, entry) in archive.files.iter().enumerate() {
        if items.len() >= BUDGET {
            truncated = true;
            break;
        }
        let Some(path) = interior(&entry.name) else {
            continue;
        };
        items.push(Item {
            path,
            size: entry.size,
            // Already a `FILETIME`: this is the one format that stores dates in the units this
            // program uses, being the one written for this operating system.
            modified: if entry.has_last_modified_date {
                entry.last_modified_date.into()
            } else {
                0
            },
            is_dir: entry.is_directory,
            unsized_: false,
            // A `.7z` encrypts its *header* when it encrypts names, in which case the read above
            // failed and there is nothing to mark. An entry inside a readable header may still have
            // encrypted data, and the archive does not say so anywhere this crate exposes — so
            // extraction is where that is found out. See [`super::extract`].
            encrypted: false,
            link: false,
            at: at as u32,
        });
    }

    Index {
        items,
        error: None,
        truncated,
        micros: 0,
    }
}

/// Walk a tar, through whatever is wrapped around it.
///
/// # A tar that stops half way still lists
///
/// The walk keeps whatever it read before the error and reports it as [`Index::truncated`], and only
/// an archive that yielded *nothing* comes back as a failure. That is not politeness: a
/// part-downloaded tarball and a tarball whose last member is corrupt are both common, both still
/// have readable contents, and the alternative answer — an empty pane saying "damaged" over an
/// archive with 4,000 perfectly good files in it — is the less true of the two.
fn tar_index(file: &Path, format: Format) -> Index {
    let stream = match stream(file, format) {
        Ok(stream) => stream,
        Err(why) => return Index::failed(reason(&why)),
    };
    let mut archive = tar::Archive::new(Capped::new(stream));
    let entries = match archive.entries() {
        Ok(entries) => entries,
        Err(why) => return Index::failed(reason(&why)),
    };

    let mut items = Vec::new();
    let mut truncated = false;

    // `(0u32..).zip` rather than `enumerate`, whose counter is a `usize`: [`Item::at`] is a `u32`,
    // and the conversion would be a cast on every entry of every archive.
    for (at, entry) in (0_u32..).zip(entries) {
        if items.len() >= BUDGET {
            truncated = true;
            break;
        }
        // The stream gave up: the cap, the clock, a corrupt header, or a file that stops early.
        let Ok(entry) = entry else {
            truncated = true;
            break;
        };

        // The raw bytes rather than `entry.path()`, which goes through `Path` and would normalise
        // away the very things [`interior`] has to see — a leading `/`, a `..`, a trailing
        // separator on a directory.
        let raw = String::from_utf8_lossy(&entry.path_bytes()).into_owned();
        let Some(path) = interior(&raw) else {
            continue;
        };

        let kind = entry.header().entry_type();
        items.push(Item {
            path,
            size: entry.size(),
            // Unix seconds, and the header may refuse to say.
            modified: entry
                .header()
                .mtime()
                .map(|secs| {
                    secs.saturating_mul(10_000_000)
                        .saturating_add(crate::fs::time::UNIX_EPOCH_FILETIME)
                })
                .unwrap_or(0),
            is_dir: kind.is_dir(),
            unsized_: false,
            encrypted: false,
            // A tar records a link as an entry with no data and a target in the header. Shown as a
            // link so a row that reads `0 B` is explained rather than looking like an empty file.
            link: kind.is_symlink() || kind.is_hard_link(),
            at,
        });
    }

    if items.is_empty() && truncated {
        return Index::failed("This archive is damaged and could not be read");
    }
    Index {
        items,
        error: None,
        truncated,
        micros: 0,
    }
}

/// A `.gz`, `.bz2`, `.xz`, `.zst` or `.lzma` on its own: one file, compressed, with no container
/// and therefore no names.
///
/// **Nothing is decompressed here.** The row's name comes from the archive's own — `access.log.gz`
/// holds `access.log`, which is the convention every tool follows and the one 7-Zip shows — and its
/// date is the archive file's, there being nowhere else for one to be recorded.
///
/// The size is the interesting part. A gzip's footer states the uncompressed length, so that one
/// gets a real figure; the other three record only what went in, and the only way to learn what
/// comes out is to decompress the whole thing. Doing that to fill a cell would mean inflating a
/// gigabyte to draw a listing of one row, so those are marked [`Item::unsized_`] and draw a blank
/// cell instead of a confident `0 B`.
fn stream_index(file: &Path, format: Format) -> Index {
    let name = file
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        // `.gz` with nothing in front of it: there is no inner name to be had, so the row keeps the
        // archive's own and at least says what it is.
        .or_else(|| {
            file.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "(contents)".to_owned());

    let (size, unknown) = match format {
        Format::Gz => match gzip_size(file) {
            Some(size) => (size, false),
            None => (0, true),
        },
        _ => (0, true),
    };

    Index {
        items: vec![Item {
            path: name,
            size,
            modified: std::fs::metadata(file)
                .map(|found| crate::fs::time::filetime_of(&found))
                .unwrap_or(0),
            is_dir: false,
            unsized_: unknown,
            encrypted: false,
            link: false,
            at: 0,
        }],
        error: None,
        truncated: false,
        micros: 0,
    }
}

/// The uncompressed length in a gzip's footer: the last four bytes, little-endian.
///
/// Exact for everything under 4 GiB and **wrong above it**, the field being the length modulo 2³²
/// — a limit baked into the format in 1992 and not fixable here. Wrong for a multi-member gzip too,
/// where it is the last member's length rather than the total.
///
/// Both are why this is used for the *size cell* and never for an allocation: [`super::extract`]
/// writes what comes out of the decoder and counts it as it goes, so a lying footer costs a
/// slightly wrong number in a column and nothing else.
fn gzip_size(file: &Path) -> Option<u64> {
    use std::io::{Seek, SeekFrom};
    let mut handle = File::open(file).ok()?;
    let len = handle.seek(SeekFrom::End(0)).ok()?;
    // A gzip is at least a 10-byte header, a 2-byte deflate block and an 8-byte footer.
    if len < 20 {
        return None;
    }
    handle.seek(SeekFrom::End(-4)).ok()?;
    let mut tail = [0u8; 4];
    handle.read_exact(&mut tail).ok()?;
    Some(u32::from_le_bytes(tail) as u64)
}

// ---------------------------------------------------------------------------
// Paths, and not trusting them
// ---------------------------------------------------------------------------

/// An entry's name as this program will use it: `/`-separated, relative, and incapable of naming
/// anything outside the archive.
///
/// `None` for a name with nothing left after that, which is how the `./` entry at the top of many
/// tars and the `..`-only names of a malicious one both disappear.
///
/// # This is the security boundary
///
/// [`super::extract`] joins these onto a temp directory and writes files there. An archive is a
/// list of paths chosen by whoever built it, and the classic attack — "zip slip" — is an entry
/// called `..\..\..\Windows\System32\something.dll`, which a naive join turns into a write outside
/// the directory that was meant to contain it. Sanitising **here**, at the one door every reader
/// pushes its names through, is what makes that impossible for every format at once rather than
/// three times over:
///
/// | in the archive | what this yields |
/// | --- | --- |
/// | `src/main.rs` | `src/main.rs` |
/// | `./src/main.rs` | `src/main.rs` |
/// | `/etc/passwd` | `etc/passwd` |
/// | `..\..\Windows\System32\evil.dll` | `Windows/System32/evil.dll` |
/// | `C:\Windows\evil.dll` | `Windows/evil.dll` |
/// | `src//deep///file` | `src/deep/file` |
/// | `dir/` | `dir` |
/// | `..`, `.`, `/`, `` | `None` |
///
/// A backslash is treated as a separator, which is what Explorer does and what the zip written by
/// a tool that forgot the specification means by it. The cost is that a Unix file legitimately
/// *named* `a\b` — legal there, impossible on Windows — lists as a folder `a` holding `b`. That is
/// the better failure: the alternative is a row whose name cannot be joined onto a Windows path at
/// all.
pub fn interior(raw: &str) -> Option<String> {
    let mut out = String::with_capacity(raw.len());
    for part in raw.split(['/', '\\']) {
        // Empty covers a leading slash, a trailing one, and `//` in the middle. `.` is a no-op
        // component. `..` is dropped rather than made to climb — see above.
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        // A drive letter or a device name that has survived this far. `C:` as a component is only
        // ever the head of an absolute Windows path that was stored where a relative one belonged.
        if part.len() == 2 && part.as_bytes()[1] == b':' {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        // A trailing space or dot is legal in an archive and cannot be created on Windows: the
        // shell silently trims both, so a file written as `name.` opens as `name` and a later
        // lookup by the untrimmed name fails. Trimmed here so the listing shows what extraction
        // will actually produce.
        out.push_str(part.trim_end_matches([' ', '.']));
        // ...unless trimming emptied it, in which case the separator just added is wrong.
        if out.ends_with('/') {
            out.pop();
        }
    }
    (!out.is_empty()).then_some(out)
}

/// A failed file operation, in the words the pane shows.
///
/// [`crate::fs::scan::error_text`] is the equivalent for a Win32 code and is the better answer
/// where there is one; this is for [`std::io`], which is what these crates return.
fn reason(why: &io::Error) -> String {
    match why.kind() {
        io::ErrorKind::NotFound => "This archive is no longer there".to_owned(),
        io::ErrorKind::PermissionDenied => "Access to this archive was denied".to_owned(),
        _ => {
            let mut text = why.to_string();
            // `io::Error`'s messages are sentences already, but not capitalised ones.
            if let Some(first) = text.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            text
        }
    }
}
