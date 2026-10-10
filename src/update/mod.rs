//! Updating from the GitHub releases: whether a newer build has been published, and putting it in
//! place of the one that is running.
//!
//! # Where the answer comes from
//!
//! `api.github.com/repos/…/releases/latest`, which is the newest release that is neither a draft nor
//! marked pre-release — so a build can be published for trying out without every window offering
//! it. `.github/workflows/release.yml` attaches two files to each release, `azur-files.exe` and
//! `azur-files.exe.sha256`, and those two names are the whole of the contract with it.
//!
//! The request goes through WinHTTP (`src/windows/update.rs`) rather than an HTTP crate, which is
//! fewer dependencies but is not why: it takes the machine's own proxy configuration and its own
//! certificate store, which is what a domain-joined laptop behind a corporate proxy needs and what
//! a bundled TLS stack does not know about.
//!
//! # How often
//!
//! **As often as [`Every`] says — a day by default — across every window.** `Win+E` starts a
//! process each time, and GitHub allows an address sixty unauthenticated requests an hour — a busy
//! morning of folder windows would use them up, and then every check fails for the rest of the
//! hour. So the last answer is kept in `update.ini` beside `config.ini`, and a window opened within
//! the interval reads the badge off that without asking anybody. There is deliberately no "every
//! start", for exactly that reason.
//!
//! A check that fails — offline, a proxy refusing, GitHub down — says nothing and writes nothing,
//! so the next launch asks again. Being offline is not news.
//!
//! **Check now** is the exception on both counts: it asks whatever the day's answer says, and it
//! always answers — "up to date", or why it could not find out — because somebody clicked and is
//! waiting to hear. See [`Drained::told`].
//!
//! # Nothing installs itself
//!
//! A check only ever raises the badge in the title bar. Installing is a click, because it ends
//! with the window restarting, and a restart in the middle of a copy loses the copy. While one is
//! running the click is refused rather than queued.
//!
//! # Replacing a running executable
//!
//! Windows will not overwrite the image of a running process, but it will rename it — the image
//! stays mapped from wherever the file went. So the new build is downloaded beside the old one as
//! `.new`, the running one is renamed to `.old`, and the `.new` takes its name. The next start
//! deletes the `.old`.
//!
//! **Another window may still be running the `.old`**, and then it cannot be deleted — which is
//! why a leftover that will not go is left alone rather than reported, and the next update sets
//! itself aside under a numbered name instead. That window carries on as the old build until it is
//! closed, which is harmless; this is the same trick `cargo deploy` uses, for the same reason.
//!
//! # The download is checked
//!
//! Against the SHA-256 published beside it, before anything is renamed. A release without one is
//! refused rather than trusted: the checksum is what tells a truncated download from a complete
//! one, and a truncated executable in place of a working one is the one failure here that a user
//! could not recover from by clicking again.
//!
//! # Never from a test
//!
//! Under `cfg!(test)` nothing here touches the network, the settings folder or the executable —
//! [`Updater::check`] and [`Updater::install`] return before starting a thread, and nothing is read
//! from or written to `update.ini`. What can be checked without them is: the version order, the
//! reading of a release, the throttle's file and the swap, the last against files in
//! `target/sandbox`.
//!
//! `YAFE_UPDATE_AS=<version>` makes a real launch believe it is that version, which is how the
//! badge and the whole install can be driven against a release that is already published.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};

mod json;

#[cfg(windows)]
#[path = "../windows/update.rs"]
mod win;

/// This build's version, as `Cargo.toml` states it.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// Where the newest release is described.
const LATEST: &str = "https://api.github.com/repos/tonyriviere88/azur-files/releases/latest";

/// The asset a release is installed from. See `release.yml`.
const EXE: &str = "azur-files.exe";

/// How often the window asks GitHub. A setting — `update=` in `config.ini` — chosen in the
/// application menu's *Auto update* submenu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Every {
    Never,
    #[default]
    Day,
    Week,
}

impl Every {
    /// In the order the menu offers them.
    pub const ALL: [Every; 3] = [Every::Never, Every::Day, Every::Week];

    pub fn label(self) -> &'static str {
        match self {
            Every::Never => "Never",
            Every::Day => "Every day",
            Every::Week => "Every week",
        }
    }

    /// The word in the settings file.
    pub fn as_str(self) -> &'static str {
        match self {
            Every::Never => "never",
            Every::Day => "day",
            Every::Week => "week",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|every| every.as_str() == text)
    }

    /// How long an answer stands. `None` is never asking.
    fn seconds(self) -> Option<u64> {
        match self {
            Every::Never => None,
            Every::Day => Some(24 * 60 * 60),
            Every::Week => Some(7 * 24 * 60 * 60),
        }
    }
}

/// The most a text answer — the release, its checksum — may be. A release's JSON is a few
/// kilobytes; this is only so that a wrong answer costs a bounded amount of memory.
const TEXT_LIMIT: usize = 4 << 20;

/// A published release, as far as this program needs one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// `1.2.0` — the tag without its `v`.
    pub version: String,
    /// The release's page, for *What's new*.
    pub page: String,
    /// Where the executable is downloaded from.
    pub exe: String,
    /// And its checksum. `None` for a release published before there was one, which is offered
    /// but refused on install — see the module header.
    pub sha: Option<String>,
}

/// Whether `candidate` is a later version than `than`. Anything unreadable is not.
///
/// Numeric, part by part, so `1.10.0` is after `1.9.0`; trailing zeros do not count, so `1.2` and
/// `1.2.0` are the same. A suffix — `-beta`, `+build` — is ignored rather than ordered: GitHub
/// keeps pre-releases out of `latest` already, and ordering them properly is semver's whole
/// grammar for a case that does not reach here.
pub fn newer(candidate: &str, than: &str) -> bool {
    match (numbers(candidate), numbers(than)) {
        (Some(candidate), Some(than)) => candidate > than,
        _ => false,
    }
}

fn numbers(version: &str) -> Option<Vec<u64>> {
    let core = version.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next()?;
    let mut parts = core
        .split('.')
        .map(|part| part.parse().ok())
        .collect::<Option<Vec<u64>>>()?;
    while parts.len() > 1 && parts.last() == Some(&0) {
        parts.pop();
    }
    Some(parts)
}

/// Read GitHub's description of a release. `None` if it is not one, or has no executable.
pub fn release(text: &str) -> Option<Release> {
    let doc = json::parse(text)?;
    let version = doc
        .get("tag_name")?
        .as_str()?
        .trim()
        .trim_start_matches(['v', 'V'])
        .to_owned();
    numbers(&version)?;
    let page = doc.get("html_url")?.as_str()?.to_owned();
    let assets: Vec<(&str, &str)> = doc
        .get("assets")?
        .as_array()?
        .iter()
        .filter_map(|asset| {
            Some((
                asset.get("name")?.as_str()?,
                asset.get("browser_download_url")?.as_str()?,
            ))
        })
        .collect();
    // By name first, and then any executable at all — a release made by hand might have called it
    // something else, and it is still the program.
    let (name, exe) = assets
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(EXE))
        .or_else(|| {
            assets
                .iter()
                .find(|(name, _)| name.to_ascii_lowercase().ends_with(".exe"))
        })?;
    let checksum = format!("{name}.sha256");
    let sha = assets
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(&checksum))
        .map(|(_, url)| (*url).to_owned());
    Some(Release {
        version,
        page,
        exe: (*exe).to_owned(),
        sha,
    })
}

/// The hash out of a `.sha256` file: `sha256sum`'s `<hex>  <name>`, or the hex alone.
fn digest(text: &str) -> Option<String> {
    let token = text.trim_start_matches('\u{feff}').split_whitespace().next()?;
    (token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| token.to_ascii_lowercase())
}

/// `https://host[:port]/path` into its three parts. Anything but `https` is refused: an
/// executable is about to be fetched from the answer.
pub(crate) fn split_url(url: &str) -> Option<(&str, u16, &str)> {
    let rest = url.strip_prefix("https://")?;
    let rest = rest.split('#').next()?;
    let (authority, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (authority, 443),
    };
    let plain = !host.is_empty() && !host.contains(['@', '?']);
    plain.then_some((host, port, path))
}

// ---------------------------------------------------------------------------
// The day's answer, kept between launches
// ---------------------------------------------------------------------------

/// What the last check found and when — `update.ini`, beside `config.ini`. See the module header.
///
/// Its own file rather than keys in `config.ini`, because a window writes that one whole and only
/// when *it* has changed something, and this changes once a day in whichever window happened to
/// ask. Keeping the two apart means neither has to know about the other's writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cache {
    /// Seconds since 1970.
    pub(crate) checked: u64,
    /// The newest release then, whether or not it was newer than this build.
    pub(crate) release: Option<Release>,
}

impl Cache {
    pub(crate) fn to_text(&self) -> String {
        let mut text = format!("checked={}\n", self.checked);
        if let Some(release) = &self.release {
            text.push_str(&format!("version={}\n", release.version));
            text.push_str(&format!("page={}\n", release.page));
            text.push_str(&format!("exe={}\n", release.exe));
            if let Some(sha) = &release.sha {
                text.push_str(&format!("sha={sha}\n"));
            }
        }
        text
    }

    /// Anything missing or unreadable is a cache with no answer in it, which is asked again.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let mut checked = None;
        let (mut version, mut page, mut exe, mut sha) = (None, None, None, None);
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim().to_owned();
            match key.trim() {
                "checked" => checked = value.parse().ok(),
                "version" => version = Some(value),
                "page" => page = Some(value),
                "exe" => exe = Some(value),
                "sha" => sha = Some(value),
                _ => {}
            }
        }
        let release = match (version, page, exe) {
            (Some(version), Some(page), Some(exe)) => Some(Release {
                version,
                page,
                exe,
                sha,
            }),
            _ => None,
        };
        Some(Self {
            checked: checked?,
            release,
        })
    }

    /// Whether this was asked within the last `within` seconds. A clock that has gone backwards
    /// makes it stale rather than good for ever.
    pub(crate) fn fresh(&self, now: u64, within: u64) -> bool {
        self.checked <= now && now - self.checked < within
    }

    fn load() -> Option<Self> {
        if cfg!(test) {
            return None;
        }
        let text = std::fs::read_to_string(crate::config::profile_file("update.ini")?).ok()?;
        Self::parse(&text)
    }

    fn store(&self) {
        if cfg!(test) {
            return;
        }
        let Some(path) = crate::config::profile_file("update.ini") else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, self.to_text());
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

// ---------------------------------------------------------------------------
// The updater
// ---------------------------------------------------------------------------

/// What the title bar shows. `None` from [`Updater::badge`] is nothing at all, which is every
/// window that is up to date.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Badge {
    Available { version: String },
    Downloading { version: String, percent: u8 },
    Failed { version: String, why: String },
}

enum State {
    /// Up to date, or not asked yet.
    Quiet,
    Available(Release),
    Downloading(Release, u8),
    Failed(Release, String),
    /// Installed; the window is on its way out.
    Done,
}

enum Event {
    /// The newest release, or why it could not be found out.
    Checked(Result<Option<Release>, String>),
    Progress(u8),
    /// The executable that is now the new build.
    Installed(Result<PathBuf, String>),
}

/// What [`Updater::drain`] found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Drained {
    /// An install has finished: the executable to restart into.
    pub installed: Option<PathBuf>,
    /// The answer to a *Check now*, for the status line — "up to date", or why it could not find
    /// out. Nothing when it found a release, because then the badge is the answer.
    pub told: Option<String>,
}

/// The check and the install, each on a thread of its own, answering through a channel drained
/// once a frame — so nothing about either can hold a frame up.
pub struct Updater {
    ctx: egui::Context,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    state: State,
    /// A check is in flight.
    asking: bool,
    /// And somebody clicked *Check now* for it, so its answer is told whatever it is.
    asked: bool,
    /// The version this build compares against: [`CURRENT`], or `YAFE_UPDATE_AS`.
    current: String,
}

impl Updater {
    /// Clears away what the last update set aside — see the module header.
    pub fn new(ctx: &egui::Context) -> Self {
        if !cfg!(test) {
            if let Ok(exe) = std::env::current_exe() {
                tidy(&exe);
            }
        }
        let current = std::env::var("YAFE_UPDATE_AS")
            .ok()
            .filter(|version| numbers(version).is_some())
            .unwrap_or_else(|| CURRENT.to_owned());
        let (tx, rx) = channel();
        Self {
            ctx: ctx.clone(),
            tx,
            rx,
            state: State::Quiet,
            asking: false,
            asked: false,
            current,
        }
    }

    /// The version this build is, for the badge's menu.
    pub fn current(&self) -> &str {
        &self.current
    }

    /// The check a window makes as it opens: nothing for [`Every::Never`], the last answer if it is
    /// recent enough, and GitHub otherwise.
    pub fn check(&mut self, every: Every) {
        let Some(within) = every.seconds() else { return };
        if cfg!(test) {
            return;
        }
        if let Some(cache) = Cache::load().filter(|cache| cache.fresh(self::now(), within)) {
            self.found(cache.release);
            return;
        }
        self.ask();
    }

    /// *Check now*: ask GitHub whatever the last answer says, and tell the answer. See
    /// [`Drained::told`].
    pub fn check_now(&mut self) {
        if cfg!(test) {
            return;
        }
        self.asked = true;
        self.ask();
    }

    fn ask(&mut self) {
        if self.asking || matches!(self.state, State::Downloading(..) | State::Done) {
            return;
        }
        self.asking = true;
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let _ = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let answer = latest();
                if let Ok(release) = &answer {
                    Cache {
                        checked: self::now(),
                        release: release.clone(),
                    }
                    .store();
                }
                let _ = tx.send(Event::Checked(answer));
                ctx.request_repaint();
            });
    }

    /// Download the release the badge is offering and put it in place of this executable. The
    /// answer arrives through [`Self::drain`].
    pub fn install(&mut self) {
        if cfg!(test) {
            return;
        }
        let release = match &self.state {
            State::Available(release) | State::Failed(release, _) => release.clone(),
            _ => return,
        };
        self.state = State::Downloading(release.clone(), 0);
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let _ = std::thread::Builder::new()
            .name("update-download".into())
            .spawn(move || {
                let progress_tx = tx.clone();
                let progress_ctx = ctx.clone();
                let result = install(&release, &mut |percent| {
                    let _ = progress_tx.send(Event::Progress(percent));
                    progress_ctx.request_repaint();
                });
                let _ = tx.send(Event::Installed(result));
                ctx.request_repaint();
            });
    }

    /// Take in whatever the threads have said.
    pub fn drain(&mut self) -> Drained {
        let mut drained = Drained::default();
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Checked(answer) => {
                    self.asking = false;
                    let asked = std::mem::take(&mut self.asked);
                    // A failed check keeps whatever was already known — see the module header.
                    match answer {
                        Ok(release) => {
                            self.found(release);
                            if asked && !matches!(self.state, State::Available(_)) {
                                drained.told = Some(format!(
                                    "{} {} is up to date",
                                    crate::brand::NAME,
                                    self.current
                                ));
                            }
                        }
                        Err(why) if asked => {
                            drained.told = Some(format!("Could not check for updates: {why}"));
                        }
                        Err(_) => {}
                    }
                }
                Event::Progress(percent) => {
                    if let State::Downloading(_, at) = &mut self.state {
                        *at = percent;
                    }
                }
                Event::Installed(Ok(exe)) => {
                    self.state = State::Done;
                    drained.installed = Some(exe);
                }
                Event::Installed(Err(why)) => {
                    if let State::Downloading(release, _) =
                        std::mem::replace(&mut self.state, State::Quiet)
                    {
                        self.state = State::Failed(release, why);
                    }
                }
            }
        }
        drained
    }

    fn found(&mut self, release: Option<Release>) {
        if matches!(self.state, State::Downloading(..) | State::Done) {
            return;
        }
        self.state = match release {
            Some(release) if newer(&release.version, &self.current) => State::Available(release),
            _ => State::Quiet,
        };
    }

    /// The release on offer, if there is one — for *What's new* and *Skip this version*.
    pub fn release(&self) -> Option<&Release> {
        match &self.state {
            State::Available(release)
            | State::Downloading(release, _)
            | State::Failed(release, _) => Some(release),
            State::Quiet | State::Done => None,
        }
    }

    /// What the title bar should show. `skip` is the version the user said to skip, which hides an
    /// offer but never a download in progress or a failure.
    pub fn badge(&self, skip: &str) -> Option<Badge> {
        match &self.state {
            State::Available(release) if release.version != skip => Some(Badge::Available {
                version: release.version.clone(),
            }),
            State::Downloading(release, percent) => Some(Badge::Downloading {
                version: release.version.clone(),
                percent: *percent,
            }),
            State::Failed(release, why) => Some(Badge::Failed {
                version: release.version.clone(),
                why: why.clone(),
            }),
            _ => None,
        }
    }
}

/// Start the executable an install has just put in place. The caller closes this window after.
pub fn relaunch(exe: &Path) -> Result<(), String> {
    if cfg!(test) {
        return Err("not from a test".into());
    }
    std::process::Command::new(exe)
        .spawn()
        .map(drop)
        .map_err(|e| format!("The update is installed, but it could not be started: {e}"))
}

// ---------------------------------------------------------------------------
// The two threads' work
// ---------------------------------------------------------------------------

/// What a body is handed to, chunk by chunk, with the length the server announced.
type Sink<'a> = dyn FnMut(&[u8], Option<u64>) -> Result<(), String> + 'a;

/// GET `url`, handing each chunk of the body to `sink` with the length the server announced.
fn fetch(
    url: &str,
    accept: &str,
    sink: &mut Sink<'_>,
) -> Result<(), String> {
    #[cfg(windows)]
    {
        win::get(url, accept, sink)
    }
    #[cfg(not(windows))]
    {
        let _ = (url, accept, sink);
        Err("Updating is only implemented on Windows".into())
    }
}

fn fetch_text(url: &str, accept: &str) -> Result<String, String> {
    let mut body = Vec::new();
    fetch(url, accept, &mut |chunk, _| {
        if body.len() + chunk.len() > TEXT_LIMIT {
            return Err("The answer was far larger than expected".into());
        }
        body.extend_from_slice(chunk);
        Ok(())
    })?;
    String::from_utf8(body).map_err(|_| "The answer was not text".into())
}

/// The newest release. `Ok(None)` is an answer — there is no release — and is remembered like one.
fn latest() -> Result<Option<Release>, String> {
    let text = fetch_text(LATEST, "application/vnd.github+json")?;
    release(&text)
        .map(Some)
        .ok_or_else(|| "GitHub's answer did not describe a release".into())
}

fn install(release: &Release, progress: &mut dyn FnMut(u8)) -> Result<PathBuf, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("Could not find this program's own file: {e}"))?;
    let sha = release.sha.as_deref().ok_or_else(|| {
        format!(
            "Release {} has no checksum to check the download against",
            release.version
        )
    })?;
    let want = digest(&fetch_text(sha, "application/octet-stream")?)
        .ok_or("The release's checksum file could not be read")?;
    let staged = sibling(&exe, "new");
    let result = download(&release.exe, &staged, progress).and_then(|got| {
        if got == want {
            swap(&exe, &staged)
        } else {
            Err("The download does not match its checksum".into())
        }
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result.map(|()| exe)
}

/// Download `url` into `to`, and answer with its SHA-256 in lower-case hex.
fn download(url: &str, to: &Path, progress: &mut dyn FnMut(u8)) -> Result<String, String> {
    use sha2::Digest as _;
    use std::io::Write as _;

    let file = std::fs::File::create(to).map_err(|e| writing(to, &e))?;
    let mut out = std::io::BufWriter::new(file);
    let mut hash = sha2::Sha256::new();
    let mut got = 0u64;
    let mut shown = 0u8;
    fetch(url, "application/octet-stream", &mut |chunk, total| {
        out.write_all(chunk).map_err(|e| writing(to, &e))?;
        hash.update(chunk);
        got += chunk.len() as u64;
        if let Some(total) = total.filter(|&total| total > 0) {
            let percent = (got.saturating_mul(100) / total).min(100) as u8;
            if percent != shown {
                shown = percent;
                progress(percent);
            }
        }
        Ok(())
    })?;
    let file = out.into_inner().map_err(|e| writing(to, e.error()))?;
    file.sync_all().map_err(|e| writing(to, &e))?;
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn writing(path: &Path, e: &std::io::Error) -> String {
    let dir = path.parent().unwrap_or(path).display();
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        format!("No permission to write in {dir}")
    } else {
        format!("Could not write in {dir}: {e}")
    }
}

/// `azur-files.exe` → `azur-files.exe.<suffix>`, in the same folder.
fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    exe.with_file_name(name)
}

/// Put `staged` where `exe` is, setting the running one aside. See the module header.
///
/// If the second rename fails the first is undone, so a failure leaves the program where it was.
pub(crate) fn swap(exe: &Path, staged: &Path) -> Result<(), String> {
    let aside = set_aside(exe);
    std::fs::rename(exe, &aside).map_err(|e| writing(exe, &e))?;
    if let Err(e) = std::fs::rename(staged, exe) {
        let _ = std::fs::rename(&aside, exe);
        return Err(writing(exe, &e));
    }
    Ok(())
}

/// The first `.old` name that is free or can be made free — `.old`, then `.1.old`, `.2.old`. One
/// that will not delete is still mapped by a window running that build.
fn set_aside(exe: &Path) -> PathBuf {
    for n in 0..64 {
        let path = if n == 0 {
            sibling(exe, "old")
        } else {
            sibling(exe, &format!("{n}.old"))
        };
        if !path.exists() || std::fs::remove_file(&path).is_ok() {
            return path;
        }
    }
    sibling(exe, &format!("{}.old", now()))
}

/// Delete whatever earlier updates set aside beside `exe`. One still running is left.
pub(crate) fn tidy(exe: &Path) {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else { return };
    let prefix = format!("{}.", name.to_string_lossy().to_lowercase());
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().to_lowercase();
        if file.starts_with(&prefix) && file.ends_with(".old") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests;
