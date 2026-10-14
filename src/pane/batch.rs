//! Renaming many files at once: the rule somebody is typing, and what it makes of each name.
//!
//! Everything here is a function of the rule and the names — no disk, no screen. The panel that
//! edits a rule is [`crate::ui::filelist`]'s; the rename itself is one
//! [`crate::shell::ops::Job::RenameMany`], so it is one entry on Ctrl+Z. What lives here is the part
//! that has to be *right* before anything is renamed, which is why it is all testable:
//!
//! - **What each name becomes** — [`plan`], for the three modes the panel offers.
//! - **What would go wrong** — [`Problem`], one per row, worked out against the listing the tab
//!   already holds rather than asked of the disk. A rename that would land on a file already
//!   there, two rows becoming one name, a name Windows refuses: each is caught here and the
//!   Rename button stays disabled until there are none, so the shell's conflict dialog is never
//!   the first anybody hears of it.
//!
//! # The extension is left alone
//!
//! Unless asked. Every mode works on the stem — `IMG_0412` of `IMG_0412.JPG` — and puts the
//! extension back on, because a find-and-replace that turned `.jpg` into `.jpeg` by accident, or a
//! pattern that dropped it, changes what the file *is* to Windows. Replace has a toggle to include
//! it; a pattern keeps it unless the pattern says where `{ext}` goes; Case has a separate toggle
//! for it. A folder has no extension: `my.photos` is a name with a dot in it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::fs::time::LocalZone;
use crate::fs::Dir;

/// Which of the three ways of making names the panel is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    /// Find and replace within each name — by far the commonest batch rename there is.
    #[default]
    Replace,
    /// A template the names are made from: `{name}_{n:03}.{ext}`.
    Pattern,
    /// Lower case, upper case or title case.
    Case,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Replace, Mode::Pattern, Mode::Case];

    pub fn label(self) -> &'static str {
        match self {
            Self::Replace => "Replace",
            Self::Pattern => "Pattern",
            Self::Case => "Case",
        }
    }

    /// The word the settings file writes it as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Pattern => "pattern",
            Self::Case => "case",
        }
    }

    /// Read back what [`Self::as_str`] wrote. `None` for anything else, which leaves the default.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str().eq_ignore_ascii_case(text))
    }
}

/// What Case mode does to the stem.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CaseTo {
    #[default]
    Lower,
    Upper,
    /// The first letter of every word upper, the rest lower. A word is a run of letters and
    /// digits, so `my_holiday-photo` is three of them.
    Title,
}

/// The tokens Pattern mode understands, for the panel's chips — inserted as written here.
pub const TOKENS: [(&str, &str); 7] = [
    ("{name}", "the current name, without its extension"),
    (
        "{ext}",
        "the extension, without its dot — leave it out and the extension is kept as it is",
    ),
    ("{n}", "a number counting up — {n:03} pads it to three digits"),
    ("{n:03}", "the number, padded to three digits"),
    ("{date}", "the date it was last modified, as 2026-10-12"),
    ("{folder}", "the name of the folder it is in"),
    ("{guid}", "a new, unique identifier for each item"),
];

/// One item being renamed, as it was when the panel opened.
#[derive(Clone, Debug)]
pub struct Item {
    /// Its name in the listing: the leaf, or a relative path in a flattened one. What it is found
    /// by again, so a listing re-read under the panel does not lose it.
    pub name: String,
    /// The name it has on disk.
    pub leaf: String,
    /// The folder it is in, relative to the listing — empty for the listing's own children. Two
    /// items only collide when they are in the same one.
    pub folder: String,
    /// The folder's own name, for `{folder}`.
    pub folder_name: String,
    pub is_dir: bool,
    /// Its modified time, for `{date}`.
    pub modified: u64,
    /// For `{guid}`: made when the panel opens, so the preview shows the names that will be used.
    pub guid: String,
    /// Where the file is — [`crate::fs::Dir::target`] — which is what the rename is asked of.
    pub target: PathBuf,
}

/// Everything the new names are made from. One struct so that "has the rule changed" is `==`, and
/// a field added here is one the cache cannot forget.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rule {
    pub mode: Mode,
    pub find: String,
    pub replace: String,
    pub match_case: bool,
    pub regex: bool,
    pub extension: bool,
    pub pattern: String,
    pub start: f64,
    pub step: f64,
    pub case_to: CaseTo,
    /// Case mode: lower-case the extension too.
    pub case_extension: bool,
}

/// The rule being typed, and the items it applies to. Lives on the tab for as long as the panel is
/// open. See [`crate::pane::Tab::batch`].
#[derive(Default)]
pub struct BatchRename {
    pub items: Vec<Item>,
    pub rule: Rule,
    /// The first frame of the panel, so the first field can take the keyboard.
    pub fresh: bool,
    /// The last plan, and the rule and listing it was planned from — so the preview is worked out
    /// when either changes and not on every frame, which over thousands of rows and a two-hundred-
    /// thousand-entry listing would be most of a frame. See [`Self::plan_cached`].
    cached: Option<(Rule, Arc<Dir>, Plan)>,
}

/// Why a row's new name cannot be used.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Problem {
    /// Nothing left of the name.
    Empty,
    /// A name Windows will not take — [`crate::fs::why_not_a_name`]'s sentence.
    Invalid(String),
    /// Another row in the same folder would get the same name.
    Duplicate,
    /// Something not being renamed already has that name, in that folder.
    Exists,
    /// Another row being renamed has that name *now*. Renaming them in one pass would ask the shell
    /// to put one on top of the other before it has moved; two passes would do, and this says so
    /// rather than guessing at an order.
    Swap,
    /// The file is no longer in the listing — renamed or removed while the panel was open.
    Gone,
}

impl Problem {
    pub fn sentence(&self) -> &str {
        match self {
            Self::Empty => "The name would be empty",
            Self::Invalid(why) => why,
            Self::Duplicate => "Another item would get the same name",
            Self::Exists => "Something with that name is already there",
            Self::Swap => "Another item being renamed has that name now — rename in two steps",
            Self::Gone => "This item is no longer here",
        }
    }
}

/// One row of the preview.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub new: String,
    /// Whether `new` is not the name the item has now.
    pub changes: bool,
    pub problem: Option<Problem>,
}

/// The whole preview: a row per item, and a sentence about the rule itself when it cannot be used
/// at all — a regex that does not compile, a token nobody has heard of.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Plan {
    pub rows: Vec<Planned>,
    pub error: Option<String>,
}

impl Plan {
    /// How many rows would actually be renamed.
    pub fn changing(&self) -> usize {
        self.rows.iter().filter(|row| row.changes).count()
    }

    /// How many rows have a problem.
    pub fn problems(&self) -> usize {
        self.rows.iter().filter(|row| row.problem.is_some()).count()
    }

    /// Whether the Rename button may be pressed: something changes, and nothing is wrong.
    pub fn ready(&self) -> bool {
        self.refusal().is_none() && self.changing() > 0
    }

    /// Why the rule cannot be applied: its own error, or the first row's problem.
    pub fn refusal(&self) -> Option<&str> {
        self.error.as_deref().or_else(|| {
            self.rows
                .iter()
                .find_map(|row| row.problem.as_ref().map(Problem::sentence))
        })
    }
}

impl BatchRename {
    /// A panel over `items`, numbering from 1.
    pub fn new(items: Vec<Item>) -> Self {
        Self {
            items,
            rule: Rule {
                pattern: "{name}_{n:03}".to_owned(),
                start: 1.0,
                step: 1.0,
                ..Default::default()
            },
            fresh: true,
            cached: None,
        }
    }

    /// [`Self::plan`] against a tab's listing: every name in it, by folder, for the collisions; and
    /// the items' own names, for the ones that have gone.
    pub fn plan_over(&self, dir: &Dir, zone: &LocalZone) -> Plan {
        let present: HashSet<&str> = (0..dir.len()).map(|i| dir.name(i)).collect();
        let listing = (0..dir.len()).map(|i| (dir.within(i), dir.leaf(i)));
        self.plan(listing, |name| present.contains(name), zone)
    }

    /// Work the plan out again if the rule or the listing has changed since the last time — a
    /// re-read under the panel is a new `Arc`, and a file that has gone or arrived since changes
    /// what collides. Read it with [`Self::preview`].
    pub fn replan(&mut self, dir: &Arc<Dir>, zone: &LocalZone) -> &Self {
        let stale = self
            .cached
            .as_ref()
            .is_none_or(|(rule, had, _)| *rule != self.rule || !Arc::ptr_eq(had, dir));
        if stale {
            let plan = self.plan_over(dir, zone);
            self.cached = Some((self.rule.clone(), dir.clone(), plan));
        }
        self
    }

    /// The plan [`Self::replan`] last made.
    pub fn preview(&self) -> &Plan {
        &self.cached.as_ref().expect("replan before preview").2
    }

    /// What Rename asks of the shell: each item that changes, and the name it gets.
    pub fn renames(&self, plan: &Plan) -> Vec<(PathBuf, String)> {
        self.items
            .iter()
            .zip(&plan.rows)
            .filter(|(_, row)| row.changes)
            .map(|(item, row)| (item.target.clone(), row.new.clone()))
            .collect()
    }

    /// Every item's name in the listing once it is renamed — what to select again afterwards.
    pub fn names_after(&self, plan: &Plan) -> Vec<String> {
        self.items
            .iter()
            .zip(&plan.rows)
            .map(|(item, row)| match item.folder.as_str() {
                "" => row.new.clone(),
                folder => format!("{folder}\\{}", row.new),
            })
            .collect()
    }

    /// What every item becomes under the rule as it stands, and what is wrong with any of it.
    ///
    /// `listing` is every name the tab's listing holds, as `(folder, leaf)` — what a new name is
    /// checked against for [`Problem::Exists`], case-insensitively, because NTFS compares names
    /// that way. `present` says whether an item's own name is still among them, for
    /// [`Problem::Gone`].
    pub fn plan<'a>(
        &self,
        listing: impl IntoIterator<Item = (&'a str, &'a str)>,
        present: impl Fn(&str) -> bool,
        zone: &LocalZone,
    ) -> Plan {
        let (names, error) = self.names(zone);
        let mut rows: Vec<Planned> = names
            .into_iter()
            .zip(&self.items)
            .map(|(new, item)| Planned {
                changes: new != item.leaf,
                new,
                problem: None,
            })
            .collect();
        if error.is_some() {
            return Plan { rows, error };
        }

        // Folded once: `(folder, lower-case name)`. Only the folders an item is in can collide, so
        // the rest of a flattened listing is not folded at all.
        fn fold<'a>(folder: &'a str, leaf: &str) -> (&'a str, String) {
            (folder, leaf.to_lowercase())
        }
        let folders: HashSet<&str> = self.items.iter().map(|item| item.folder.as_str()).collect();
        let current: HashMap<(&str, String), usize> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| (fold(&item.folder, &item.leaf), i))
            .collect();
        let existing: HashSet<(&str, String)> = listing
            .into_iter()
            .filter(|(folder, _)| folders.contains(folder))
            .map(|(folder, leaf)| fold(folder, leaf))
            .collect();
        let mut wanted: HashMap<(&str, String), usize> = HashMap::new();
        for (row, item) in rows.iter().zip(&self.items) {
            *wanted.entry(fold(&item.folder, &row.new)).or_default() += 1;
        }

        for i in 0..rows.len() {
            let item = &self.items[i];
            let row = &rows[i];
            let problem = 'problem: {
                if !present(&item.name) {
                    break 'problem Some(Problem::Gone);
                }
                if !row.changes {
                    break 'problem None;
                }
                if row.new.trim().is_empty() {
                    break 'problem Some(Problem::Empty);
                }
                if let Some(why) = crate::fs::why_not_a_name(&row.new) {
                    break 'problem Some(Problem::Invalid(why));
                }
                let key = fold(&item.folder, &row.new);
                if wanted.get(&key).copied().unwrap_or(0) > 1 {
                    break 'problem Some(Problem::Duplicate);
                }
                // A change of case only: the same file under the same name to NTFS, and the shell
                // renames it in place. Nothing to collide with.
                if key == fold(&item.folder, &item.leaf) {
                    break 'problem None;
                }
                match current.get(&key) {
                    // Another row of the batch is called that now. If it keeps its name, this lands
                    // on a file that stays; if it changes, the one pass would collide on the way.
                    Some(&other) if other != i => Some(if rows[other].changes {
                        Problem::Swap
                    } else {
                        Problem::Exists
                    }),
                    _ if existing.contains(&key) => Some(Problem::Exists),
                    _ => None,
                }
            };
            rows[i].problem = problem;
        }
        Plan { rows, error: None }
    }

    /// Every item's new name, or the sentence about why the rule cannot be applied — in which case
    /// every name is its own, so the preview shows nothing changing rather than nonsense.
    fn names(&self, zone: &LocalZone) -> (Vec<String>, Option<String>) {
        // A regex is compiled once, not per name, and a bad one is the rule's problem, not a row's.
        // Capped like the find bar's, for the same reason: it is compiled on every keystroke.
        let regex = match (self.rule.mode, self.rule.regex && !self.rule.find.is_empty()) {
            (Mode::Replace, true) => match regex::RegexBuilder::new(&self.rule.find)
                .case_insensitive(!self.rule.match_case)
                .size_limit(crate::preview::search::PATTERN_LIMIT)
                .build()
            {
                Ok(regex) => Some(regex),
                Err(e) => return (self.unchanged(), Some(regex_sentence(&e))),
            },
            _ => None,
        };
        let mut names = Vec::with_capacity(self.items.len());
        for i in 0..self.items.len() {
            match self.name_with(i, zone, regex.as_ref()) {
                Ok(name) => names.push(name),
                Err(why) => return (self.unchanged(), Some(why)),
            }
        }
        (names, None)
    }

    fn unchanged(&self) -> Vec<String> {
        self.items.iter().map(|item| item.leaf.clone()).collect()
    }

    /// One item's new name.
    fn name_with(
        &self,
        index: usize,
        zone: &LocalZone,
        regex: Option<&regex::Regex>,
    ) -> Result<String, String> {
        let item = &self.items[index];
        let (stem, ext) = split(&item.leaf, item.is_dir);
        Ok(match self.rule.mode {
            Mode::Replace => {
                if self.rule.find.is_empty() {
                    return Ok(item.leaf.clone());
                }
                let replace = |text: &str| match regex {
                    Some(regex) => regex.replace_all(text, self.rule.replace.as_str()).into_owned(),
                    None => replace_literal(text, &self.rule.find, &self.rule.replace, self.rule.match_case),
                };
                if self.rule.extension {
                    replace(&item.leaf)
                } else {
                    join(&replace(stem), ext)
                }
            }
            // **Without `{ext}`, the pattern makes the stem and the extension stays**: `{name}_{n}`
            // over `a.jpg` is `a_1.jpg`. With it, the pattern is the whole name and says where the
            // extension goes — which is the only way a pattern can change or drop one.
            Mode::Pattern => {
                let number = self.rule.start + self.rule.step * index as f64;
                let made = expand(&self.rule.pattern, item, stem, ext, number, zone)?;
                if self.rule.pattern.contains("{ext}") {
                    made
                } else {
                    join(&made, ext)
                }
            }
            Mode::Case => {
                let stem = match self.rule.case_to {
                    CaseTo::Lower => stem.to_lowercase(),
                    CaseTo::Upper => stem.to_uppercase(),
                    CaseTo::Title => title(stem),
                };
                let ext = if self.rule.case_extension {
                    ext.to_lowercase()
                } else {
                    ext.to_owned()
                };
                join(&stem, &ext)
            }
        })
    }
}

/// A name's stem and extension: `IMG_0412.JPG` is `IMG_0412` and `JPG`. A leading dot is not an
/// extension — `.gitignore` is all stem — and a folder has none.
pub fn split(leaf: &str, is_dir: bool) -> (&str, &str) {
    if is_dir {
        return (leaf, "");
    }
    match leaf.rfind('.') {
        Some(at) if at > 0 => (&leaf[..at], &leaf[at + 1..]),
        _ => (leaf, ""),
    }
}

fn join(stem: &str, ext: &str) -> String {
    if ext.is_empty() {
        stem.to_owned()
    } else {
        format!("{stem}.{ext}")
    }
}

/// Every occurrence of `find` in `text` replaced, case-insensitively unless asked otherwise.
///
/// ASCII folding for the case-insensitive search, because it keeps byte offsets where they are —
/// a fold beyond ASCII can change a string's length, and the replacement has to land at the
/// offsets of the *original*. A non-ASCII letter is then matched only as typed, which is the
/// cautious way round: it replaces less, never the wrong thing.
fn replace_literal(text: &str, find: &str, with: &str, match_case: bool) -> String {
    if match_case {
        return text.replace(find, with);
    }
    let hay = text.to_ascii_lowercase();
    let needle = find.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(found) = hay[at..].find(&needle) {
        out.push_str(&text[at..at + found]);
        out.push_str(with);
        at += found + needle.len();
    }
    out.push_str(&text[at..]);
    out
}

/// A pattern with its tokens filled in. An unknown token is the rule's error, not a literal.
///
/// **`.{ext}` on something with no extension leaves no dot behind**: `{name}.{ext}` over a file
/// called `README` is `README`, not `README.` — which Windows would refuse anyway, and which is not
/// what anybody typing that pattern meant.
fn expand(
    pattern: &str,
    item: &Item,
    stem: &str,
    ext: &str,
    number: f64,
    zone: &LocalZone,
) -> Result<String, String> {
    let mut out = String::with_capacity(pattern.len() + item.leaf.len());
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            return Err("A { has no } to close it".to_owned());
        };
        let token = &rest[open + 1..open + close];
        rest = &rest[open + close + 1..];
        let (name, format) = token.split_once(':').unwrap_or((token, ""));
        match name {
            "name" => out.push_str(stem),
            "ext" => {
                if ext.is_empty() {
                    if out.ends_with('.') {
                        out.pop();
                    }
                } else {
                    out.push_str(ext);
                }
            }
            "n" => {
                let width: usize = if format.is_empty() {
                    0
                } else {
                    format
                        .parse()
                        .map_err(|_| format!("{{n:{format}}} is not a width — try {{n:03}}"))?
                };
                // Whole numbers, which is what a counter is; a fractional step is rounded.
                let n = number.round() as i64;
                out.push_str(&format!("{n:0width$}"));
            }
            "date" => match zone.convert(item.modified) {
                Some(at) => out.push_str(&format!("{:04}-{:02}-{:02}", at.year, at.month, at.day)),
                None => out.push_str("0000-00-00"),
            },
            "folder" => out.push_str(&item.folder_name),
            "guid" => out.push_str(&item.guid),
            _ => return Err(format!("{{{token}}} is not a token this understands")),
        }
    }
    out.push_str(rest);
    Ok(out)
}

/// Title case: the first letter of each run of letters and digits upper, the rest lower.
fn title(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut start = true;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if start {
                out.extend(c.to_uppercase());
            } else {
                out.extend(c.to_lowercase());
            }
            start = false;
        } else {
            out.push(c);
            start = true;
        }
    }
    out
}

/// The regex crate's error, which is several lines with a caret under the pattern, as one line.
fn regex_sentence(e: &regex::Error) -> String {
    let text = e.to_string();
    let last = text
        .lines()
        .rev()
        .find(|line| line.trim_start().starts_with("error:"))
        .unwrap_or("error: not a valid pattern");
    format!("Not a valid regex — {}", last.trim_start().trim_start_matches("error: "))
}

#[cfg(test)]
mod tests;
