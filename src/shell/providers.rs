//! Which file types this machine can draw a picture of, asked once per type.
//!
//! The half of [`crate::pane::AutoTiles`]' count that no table could carry.
//! [`crate::fs::fmt::shows_a_picture`] answers for the types that are pictures by name; every other
//! type is a *maybe* that depends on what is installed, and this is where the maybes get answered on
//! the machine they are a question about: a `.pdf` where a reader is installed, a `.psd`, a camera's
//! `.cr2`, a `.3dr` where Cyclone 3DR is.
//!
//! # Per type, cached, and off the registry rather than off the file
//!
//! Two decisions, and they are the same decision [`crate::shell::icons`] makes at length in its own
//! header: **the answer belongs to the extension, not to the file**, so a folder of five hundred
//! `.pdf` files asks once and a folder of five hundred distinct types is not a folder anybody has.
//!
//! The reading is `windows/providers.rs`'s `registered` — four registry opens, microseconds,
//! nothing loaded and nothing run. The alternative was the *true* answer, `GetImage` on a real file
//! with `SIIGBF_THUMBNAILONLY`, which is what [`crate::shell::thumbs::rendered`] does to draw a tile.
//! It was not chosen here and the reason is not tidiness:
//!
//! - **It costs 12–22 ms per type when the answer is no**, measured in [`crate::preview::visual`].
//!   That is the answer this needs most often, and it would be spent on the UI thread at the moment a
//!   folder opens — the one moment this program's whole design is about not spending anything at.
//! - **It runs a stranger's DLL in this process**, in-proc and synchronously, with nothing to cancel.
//!   See [`crate::shell::thumbs`] on the `.svg` provider that takes 1.2 seconds a file.
//! - **It needs a file**, so the answer for a type would depend on which file happened to be asked
//!   about — and a truncated `.pdf` would take `.pdf` out of the count for the whole session.
//!
//! What the registry answer gives up is exactness: a registered provider can still fail on a
//! particular file, and then the tile shows that file's icon. Which is the same thing the tiles have
//! always done for a file with no thumbnail, and a great deal better than a threshold that means
//! something different depending on which file it sampled.
//!
//! # This is the behaviour and not a setting
//!
//! It was a tick for a while, off by default, on the reasoning that what it does to the count is a
//! decision about somebody's folders: with an Office and a PDF handler installed, a folder of
//! documents becomes a folder of "pictures" and opens as a grid of page-one renders. Measured, that
//! reasoning does not survive the numbers — **184 µs a type and cached**, so the whole question costs
//! less than a fifth of a millisecond once per type per session, and turning it off makes the rule
//! count *this program's table* rather than what the machine can actually show. A rule that called a
//! folder of documents 0% on a machine that draws every one of them was the wrong answer, and a
//! setting to correct it was a setting to ask people to find. So both questions are asked, always,
//! and the threshold is the only thing there is to tune.

use std::collections::HashMap;

#[cfg(windows)]
#[path = "../windows/providers.rs"]
mod win;

/// Answers about file types, remembered for the session.
///
/// One per application, beside [`crate::shell::icons`] and for the same reason. Bounded by the number
/// of distinct extensions this window has been asked about — tens, not thousands — and every entry is
/// a short string and a bool. Nothing here is keyed by path, so nothing here outlives a folder in the
/// way that mattered so much to the icon cache.
#[derive(Default)]
pub struct Providers {
    /// Extension, lowercased, to whether this machine has a thumbnail provider for it.
    known: HashMap<String, bool>,
    /// How many types have actually been asked about.
    ///
    /// **Under `cfg(test)` only**, because the one thing it is for is checking the claim this whole
    /// module rests on — that the registry is touched once per *type* and never once per file — and
    /// nothing in the window has any use for it. It was a line in the view switch's menu for a while,
    /// beside a tick that has since become the behaviour; a counter kept in a shipped build for a
    /// message nobody reads is state pretending to be a feature.
    #[cfg(test)]
    asked: usize,
}

impl Providers {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether this machine can draw a picture of a file of this type.
    ///
    /// The extension without its dot, as [`crate::fs::Dir::ext`] stores it. Matched
    /// case-insensitively, because `.PDF` and `.pdf` are one type and a cache keyed by the spelling
    /// on disk would ask twice and hold two entries.
    pub fn has_one(&mut self, ext: &str) -> bool {
        if ext.is_empty() {
            return false;
        }
        // Looked up before anything is allocated: the hit is the common case by a wide margin — every
        // row after the first of its type — and a `to_lowercase` per row would be an allocation per
        // row, which is the cost this whole design exists to avoid.
        if let Some(&known) = self.known.get(ext) {
            return known;
        }
        let key = ext.to_lowercase();
        if let Some(&known) = self.known.get(&key) {
            // Seen under a different spelling. Remembered under this one too, so the next row with
            // the same capitalisation takes the cheap path above.
            self.known.insert(ext.to_owned(), known);
            return known;
        }
        let answer = Self::ask(&key);
        #[cfg(test)]
        {
            self.asked += 1;
        }
        self.known.insert(key, answer);
        if ext.chars().any(char::is_uppercase) {
            self.known.insert(ext.to_owned(), answer);
        }
        answer
    }

    /// How many types have been asked about since this was created. See the field.
    #[cfg(test)]
    pub fn asked(&self) -> usize {
        self.asked
    }

    #[cfg(windows)]
    fn ask(ext: &str) -> bool {
        win::registered(ext)
    }

    /// Off Windows there are no registered providers to ask about, so the tick can only ever add
    /// nothing — exactly as [`crate::preview::kind_of`] answers `None` for the shell's own kind there.
    #[cfg(not(windows))]
    fn ask(_ext: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The registry is touched once per type**, whatever the spelling, and never once per file.
    ///
    /// The claim the whole module rests on, and the one thing about it that a reader cannot check: a
    /// version of this that asked per file would be correct, would pass every other test, and would
    /// put four registry opens on every row of every folder that is opened.
    ///
    /// Deliberately not an assertion about *which* types come back true — that is a fact about the
    /// machine the test is running on, which is why `providers_on_this_machine` prints it instead.
    #[test]
    fn a_type_is_asked_about_once() {
        let mut providers = Providers::new();
        let first = providers.has_one("pdf");
        assert_eq!(providers.asked(), 1, "one type, one question");

        assert_eq!(providers.has_one("pdf"), first);
        assert_eq!(providers.has_one("PDF"), first, "case is not a second type");
        assert_eq!(providers.has_one("Pdf"), first);
        assert_eq!(
            providers.asked(),
            1,
            "the same type was asked about {} times",
            providers.asked()
        );

        // A type nobody has, which is the answer this must be able to give cheaply and often.
        assert!(!providers.has_one("zzzznosuchtype"));
        assert!(!providers.has_one(""), "no extension is no provider");
        assert_eq!(providers.asked(), 2, "an empty extension is not a question");
    }

    /// What **this** machine says, and what asking it cost.
    ///
    /// A diagnostic rather than a test, for the same reason `what_is_under_the_pointer` is one: the
    /// answers are facts about the machine, so there is nothing here to assert. Run it to find out why
    /// a folder of yours opened as a grid — or did not.
    ///
    /// ```text
    /// cargo test --bin azur-file-explorer providers_on_this_machine -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "diagnostic; run explicitly"]
    fn providers_on_this_machine() {
        // The types no table could answer for, and two the table does — so the list also shows what
        // would have been counted anyway.
        const TYPES: [&str; 24] = [
            "pdf", "docx", "doc", "xlsx", "xls", "pptx", "ppt", "rtf", "odt", "epub", "psd", "ai",
            "eps", "3dr", "dwg", "dxf", "ifc", "stl", "fbx", "obj", "e57", "las", "png", "mp4",
        ];
        let mut providers = Providers::new();
        let mut yes = Vec::new();
        let mut total = 0u128;
        for ext in TYPES {
            let started = std::time::Instant::now();
            let has = providers.has_one(ext);
            let micros = started.elapsed().as_micros();
            total += micros;
            println!("  .{ext:<6} {:<5} {micros:>6} µs", if has { "yes" } else { "no" });
            if has {
                yes.push(ext);
            }
        }
        println!(
            "\n{} of {} types have a thumbnail provider here: {}",
            yes.len(),
            TYPES.len(),
            yes.join(" ")
        );
        println!(
            "{} asked, {:.2} ms in all — {:.0} µs a type",
            providers.asked(),
            total as f64 / 1000.0,
            total as f64 / providers.asked().max(1) as f64
        );
    }
}
