//! Where you have been: every folder a pane has gone to, ranked by how often and how lately, for the
//! path field to offer back while you type — what a browser's address bar does with its history.
//!
//! **Ranked by frecency, the way `zoxide` ranks.** Each visit adds one to a folder's rank, and the
//! rank is weighted by how long ago the last visit was when it is read: four times within the hour,
//! twice within the day, half within the week and a quarter after that. So a folder used every day
//! for a month outranks one opened twice this morning, and both outrank one opened forty times last
//! year. When the ranks add up to more than [`AGE_AT`], every one of them is scaled down by a tenth
//! and whatever falls below one is forgotten, which is what keeps the file a bounded size without a
//! rule about which entries to throw away.
//!
//! **A visit is a folder that opened.** It is recorded when the listing a navigation asked for
//! reaches the tab without an error, from a worker or from the cache — see
//! [`crate::pane::Tab::take_visit`] — so a mistyped path, a share that is down or a folder since
//! deleted never gets in. A tab restored at startup is not a visit; nobody went there, it was where
//! they were.
//!
//! **Kept beside the settings, in `history.tsv`, and merged rather than overwritten.** Each window
//! has the file's contents in memory and the changes it has made since, and saving reads the file
//! again, replays those changes onto what it finds and writes the result. Two windows open all day
//! therefore add up, instead of the second one to close throwing away the first one's afternoon —
//! which is how an unconditional save once lost a bookmark. See [`History::save`].

use std::path::{Path, PathBuf};

/// When the ranks add up to more than this, they are all scaled down. `zoxide`'s figure.
const AGE_AT: f64 = 10_000.0;

/// What a bookmark adds to a folder's score when the field ranks its offers. About what a handful of
/// visits this week is worth, so a bookmark comes before a folder opened once yesterday and after one
/// you are in every hour.
const BOOKMARK: f64 = 8.0;

/// One folder, as the file records it.
#[derive(Clone, Debug, PartialEq)]
struct Visit {
    path: PathBuf,
    /// How many times, less whatever aging has taken off.
    rank: f64,
    /// The last visit, in seconds since 1970.
    last: u64,
}

/// What this window has done to the history since it last saved, to be replayed onto the file.
#[derive(Clone, Debug, PartialEq)]
enum Change {
    Visit { path: PathBuf, at: u64 },
    Forget(PathBuf),
}

/// One thing the path field offers out of the history or the bookmarks.
#[derive(Clone, Debug, PartialEq)]
pub struct Recalled {
    pub path: PathBuf,
    /// A bookmark, which is drawn with its star — whether or not it has been visited too.
    pub bookmarked: bool,
}

#[derive(Default)]
pub struct History {
    visits: Vec<Visit>,
    pending: Vec<Change>,
    /// Bumped on every change, so the path field can tell its offers are stale without comparing
    /// lists. See [`crate::ui::breadcrumb::PathComplete`].
    generation: u64,
    /// `From history` unticked in the path field's menu: nothing is recorded and nothing is offered.
    ///
    /// **What is already there is kept**, neither offered nor added to, so ticking it again picks up
    /// where it left off; `Shift+Delete` and the file itself are the ways to take things out of it.
    /// Off rather than on in the field so that a history made by `Default` — every test's — records.
    paused: bool,
}

impl History {
    /// What is on disk. Nothing from a test, which builds its own.
    pub fn load() -> Self {
        let mut history = Self::default();
        if !cfg!(test) {
            if let Some(text) = file().and_then(|path| std::fs::read_to_string(path).ok()) {
                history.visits = parse(&text);
            }
        }
        history
    }

    /// Stop recording and offering, or start again. See [`Self::paused`].
    pub fn set_paused(&mut self, paused: bool) {
        if self.paused != paused {
            self.paused = paused;
            self.generation += 1;
        }
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// A pane has opened `path`.
    pub fn visit(&mut self, path: &Path, at: u64) {
        if self.paused || !worth_keeping(path) {
            return;
        }
        visit(&mut self.visits, path, at);
        self.pending.push(Change::Visit {
            path: path.to_path_buf(),
            at,
        });
        self.generation += 1;
    }

    /// Take `path` out of the history: `Shift+Delete` on an offer, as in a browser.
    pub fn forget(&mut self, path: &Path) {
        self.visits.retain(|visit| !same(&visit.path, path));
        self.pending.push(Change::Forget(path.to_path_buf()));
        self.generation += 1;
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether there is anything to save.
    pub fn dirty(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Write the history, merged with whatever another window has written since this one read it.
    ///
    /// On the UI thread, as the settings are, and for the same reason it is affordable: the file is
    /// a few hundred lines at most, and [`crate::app`] asks at most every few seconds and once on
    /// the way out.
    pub fn save(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending);
        if cfg!(test) {
            return;
        }
        let Some(path) = file() else { return };
        let on_disk = std::fs::read_to_string(&path)
            .map(|text| parse(&text))
            .unwrap_or_default();
        self.visits = replay(on_disk, &pending);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Staged and renamed over, as the settings are, so a crash mid-write leaves the old file.
        let text = to_text(&self.visits);
        let staged = path.with_extension("tsv.new");
        if std::fs::write(&staged, &text).is_ok() && std::fs::rename(&staged, &path).is_ok() {
            return;
        }
        let _ = std::fs::remove_file(&staged);
        let _ = std::fs::write(path, text);
    }

    /// What the path field offers for `typed`, best first: visited folders and bookmarks, at most
    /// `limit` of them, never `here` — the folder the pane is already in.
    ///
    /// **Every word typed has to appear somewhere in the path**, in any case and in any order,
    /// which is how a browser's bar matches: `api work` finds `C:\work\api`, and so does `C:\wo`. A
    /// folder whose own name starts with the last word counts double, because the name is what
    /// people type.
    ///
    /// No disk is touched. A folder offered here may have gone since; opening it says so, and
    /// `Shift+Delete` takes it off the list.
    pub fn recall<'a>(
        &self,
        typed: &str,
        bookmarks: impl Iterator<Item = &'a PathBuf>,
        here: &Path,
        now: u64,
        limit: usize,
    ) -> Vec<Recalled> {
        let typed = fold(typed);
        let words: Vec<&str> = typed.split_whitespace().collect();
        let fits = |path: &Path| {
            let folded = fold(&path.to_string_lossy());
            words.iter().all(|word| folded.contains(word))
        };
        let named = |path: &Path| {
            let (Some(last), Some(name)) = (words.last(), path.file_name()) else {
                return 1.0;
            };
            if fold(&name.to_string_lossy()).starts_with(last) {
                2.0
            } else {
                1.0
            }
        };

        let mut found: Vec<(f64, Recalled)> = Vec::new();
        let visits = if self.paused { &[][..] } else { &self.visits[..] };
        for visit in visits {
            if !same(&visit.path, here) && fits(&visit.path) {
                let score = frecency(visit, now) * named(&visit.path);
                found.push((score, Recalled { path: visit.path.clone(), bookmarked: false }));
            }
        }
        for mark in bookmarks {
            if same(mark, here) || !fits(mark) {
                continue;
            }
            let bonus = BOOKMARK * named(mark);
            match found.iter_mut().find(|(_, offer)| same(&offer.path, mark)) {
                Some((score, offer)) => {
                    *score += bonus;
                    offer.bookmarked = true;
                }
                None => found.push((bonus, Recalled { path: mark.clone(), bookmarked: true })),
            }
        }
        found.sort_by(|a, b| b.0.total_cmp(&a.0));
        found.truncate(limit);
        found.into_iter().map(|(_, offer)| offer).collect()
    }
}

/// Seconds since 1970, which is what the file records.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// Whether a folder is somewhere to offer back.
///
/// A real folder and not a root: not This PC, which is the empty path; not the Recycle Bin, which is
/// `shell:RecycleBinFolder`, or anything in it; and not a drive's root, which the field already
/// offers by its letter.
fn worth_keeping(path: &Path) -> bool {
    path.is_absolute()
        && path.parent().is_some()
        && !path
            .components()
            .any(|part| part.as_os_str().eq_ignore_ascii_case("$Recycle.Bin"))
}

fn visit(visits: &mut Vec<Visit>, path: &Path, at: u64) {
    match visits.iter_mut().find(|visit| same(&visit.path, path)) {
        Some(visit) => {
            visit.rank += 1.0;
            visit.last = visit.last.max(at);
        }
        None => visits.push(Visit {
            path: path.to_path_buf(),
            rank: 1.0,
            last: at,
        }),
    }
    let total: f64 = visits.iter().map(|visit| visit.rank).sum();
    if total > AGE_AT {
        for visit in visits.iter_mut() {
            visit.rank *= 0.9;
        }
        visits.retain(|visit| visit.rank >= 1.0);
    }
}

/// What the file says, with this window's changes made to it in order.
fn replay(mut visits: Vec<Visit>, changes: &[Change]) -> Vec<Visit> {
    for change in changes {
        match change {
            Change::Visit { path, at } => visit(&mut visits, path, *at),
            Change::Forget(path) => visits.retain(|visit| !same(&visit.path, path)),
        }
    }
    visits
}

fn frecency(visit: &Visit, now: u64) -> f64 {
    const HOUR: u64 = 3600;
    let age = now.saturating_sub(visit.last);
    let weight = match age {
        age if age < HOUR => 4.0,
        age if age < 24 * HOUR => 2.0,
        age if age < 7 * 24 * HOUR => 0.5,
        _ => 0.25,
    };
    visit.rank * weight
}

/// Two spellings of one folder: case-insensitively, as Windows compares them.
fn same(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

/// Text to compare paths by: lower case, with either slash as a backslash — a path typed into the
/// field with forward slashes is the same path.
fn fold(text: &str) -> String {
    text.to_lowercase().replace('/', "\\")
}

/// `rank<TAB>last<TAB>path` a line. A line that does not read is skipped, never fatal.
fn parse(text: &str) -> Vec<Visit> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, '\t');
            let rank: f64 = fields.next()?.parse().ok()?;
            let last: u64 = fields.next()?.parse().ok()?;
            let path = fields.next()?;
            (rank.is_finite() && rank > 0.0 && !path.is_empty()).then(|| Visit {
                path: PathBuf::from(path),
                rank,
                last,
            })
        })
        .collect()
}

fn to_text(visits: &[Visit]) -> String {
    let mut text = String::new();
    for visit in visits {
        text.push_str(&format!(
            "{:.3}\t{}\t{}\n",
            visit.rank,
            visit.last,
            visit.path.display()
        ));
    }
    text
}

fn file() -> Option<PathBuf> {
    crate::config::profile_file("history.tsv")
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;
    const HOUR: u64 = 3600;

    fn paths(offers: &[Recalled]) -> Vec<&str> {
        offers.iter().map(|o| o.path.to_str().unwrap()).collect()
    }

    #[test]
    fn often_and_lately_outranks_either_alone() {
        let mut history = History::default();
        // Every day for a month, last yesterday.
        for day in 1..=30 {
            history.visit(Path::new(r"C:\work\api"), NOW - day * 24 * HOUR);
        }
        // Twice this morning.
        history.visit(Path::new(r"C:\work\web"), NOW - 2 * HOUR);
        history.visit(Path::new(r"C:\work\web"), NOW - HOUR);
        // Forty times, a year ago.
        for _ in 0..40 {
            history.visit(Path::new(r"C:\old\thing"), NOW - 365 * 24 * HOUR);
        }
        let offers = history.recall("", std::iter::empty(), Path::new(""), NOW, 10);
        assert_eq!(paths(&offers), [r"C:\work\api", r"C:\old\thing", r"C:\work\web"]);
    }

    #[test]
    fn every_word_has_to_be_in_the_path_in_any_order_and_case() {
        let mut history = History::default();
        history.visit(Path::new(r"C:\work\api"), NOW);
        history.visit(Path::new(r"C:\work\web"), NOW);
        history.visit(Path::new(r"D:\photos"), NOW);
        let recall = |typed: &str| {
            let offers = history.recall(typed, std::iter::empty(), Path::new(""), NOW, 10);
            paths(&offers).into_iter().map(str::to_owned).collect::<Vec<_>>()
        };
        assert_eq!(recall("API work"), [r"C:\work\api"]);
        assert_eq!(recall("c:/work/a"), [r"C:\work\api"], "a forward slash is a separator");
        assert_eq!(recall("photo"), [r"D:\photos"]);
        assert!(recall("nowhere").is_empty());
    }

    #[test]
    fn a_bookmark_is_offered_starred_and_once() {
        let mut history = History::default();
        history.visit(Path::new(r"C:\work\api"), NOW);
        let marks = [PathBuf::from(r"C:\WORK\API"), PathBuf::from(r"C:\docs")];
        let offers = history.recall("", marks.iter(), Path::new(""), NOW, 10);
        assert_eq!(offers.len(), 2, "{offers:?}");
        assert!(offers.iter().all(|o| o.bookmarked));
        // Visited and bookmarked comes before bookmarked alone.
        assert_eq!(offers[0].path, PathBuf::from(r"C:\work\api"));
    }

    #[test]
    fn the_folder_you_are_in_and_the_places_nobody_types_are_not_offered() {
        let mut history = History::default();
        history.visit(Path::new(r"C:\work\api"), NOW);
        history.visit(Path::new(r"C:\"), NOW);
        history.visit(Path::new(""), NOW);
        history.visit(&crate::fs::recycle::location(), NOW);
        assert_eq!(history.visits.len(), 1, "{:?}", history.visits);
        let offers = history.recall("", std::iter::empty(), Path::new(r"c:\work\api"), NOW, 10);
        assert!(offers.is_empty(), "{offers:?}");
    }

    /// Two windows, each with its own copy: saving replays this window's changes onto the file
    /// rather than writing its copy over the other window's.
    #[test]
    fn saving_adds_to_what_another_window_wrote() {
        let on_disk = parse("3.000\t100\tC:\\theirs\n1.000\t100\tC:\\gone\n");
        let changes = [
            Change::Visit { path: PathBuf::from(r"C:\mine"), at: 200 },
            Change::Visit { path: PathBuf::from(r"C:\theirs"), at: 300 },
            Change::Forget(PathBuf::from(r"C:\gone")),
        ];
        let merged = replay(on_disk, &changes);
        let text = to_text(&merged);
        assert_eq!(text, "4.000\t300\tC:\\theirs\n1.000\t200\tC:\\mine\n");
        assert_eq!(parse(&text), merged, "the file reads back as it was written");
    }

    /// Paused, nothing is recorded and nothing is offered — and what was there comes back with the
    /// switch, because pausing is not forgetting.
    #[test]
    fn a_paused_history_records_nothing_and_offers_nothing() {
        let mut history = History::default();
        history.visit(Path::new(r"C:\work\api"), NOW);
        history.set_paused(true);
        history.visit(Path::new(r"C:\work\web"), NOW);
        assert!(history.recall("", std::iter::empty(), Path::new(""), NOW, 10).is_empty());
        assert_eq!(history.pending.len(), 1, "the visit made while paused was queued to save");
        history.set_paused(false);
        let offers = history.recall("", std::iter::empty(), Path::new(""), NOW, 10);
        assert_eq!(paths(&offers), [r"C:\work\api"]);
    }

    #[test]
    fn a_line_that_does_not_read_is_skipped() {
        let visits = parse("x\t1\tC:\\a\n2\t\tC:\\b\nNaN\t1\tC:\\c\n1\t5\t\n1.5\t5\tC:\\ok\n");
        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].path, PathBuf::from(r"C:\ok"));
    }

    #[test]
    fn ranks_age_once_they_add_up() {
        let mut visits = Vec::new();
        for i in 0..=AGE_AT as u64 {
            visit(&mut visits, Path::new(&format!(r"C:\f{}", i % 2)), i);
        }
        let total: f64 = visits.iter().map(|v| v.rank).sum();
        assert!(total <= AGE_AT, "{total}");
        assert_eq!(visits.len(), 2);
    }
}
