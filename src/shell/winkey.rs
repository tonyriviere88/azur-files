//! `Win+E`: whether Windows' folder key opens this program or Explorer.
//!
//! One setting, and the only one in this program that changes something outside the window. What it
//! does mechanically — one registry key under `HKEY_CURRENT_USER`, and the measured quirk that makes
//! it work at all — is `windows/winkey.rs`. What is decided here is the shape of it: what is
//! claimed, what is deliberately *not*, and where the truth about it lives.
//!
//! # `Win+E` and nothing else
//!
//! There are three separate ways Windows opens a folder window, and this takes over exactly one.
//! The distinction is worth stating because it is the difference between a setting and a
//! replacement:
//!
//! - **`Win+E`** — the verb this claims. A keystroke the user presses on purpose, meaning "open my
//!   file manager".
//! - **Opening a folder** — `ShellExecute` on a directory, a folder shortcut, a path typed into the
//!   Run box. These resolve through the `Folder` and `Directory` classes, and claiming *those*
//!   would put this program in front of every folder every program on the machine opens, including
//!   the virtual ones it cannot draw — This PC, Control Panel, the Recycle Bin, a search result.
//!   Not claimed.
//! - **"Show in folder"** from a browser's download list, an installer, an editor. Not claimable at
//!   all, and worth knowing rather than discovering: those callers either go through
//!   `SHOpenFolderAndSelectItems`, which builds an Explorer window inside `shell32` without
//!   consulting any registered verb, or they spawn `explorer.exe /select,…` by name. No registry
//!   key reaches either one. The only thing that would is a shim standing in for `explorer.exe`
//!   itself, which is a different kind of program from this one.
//!
//! So: the keystroke, because the keystroke is a request. Not the plumbing every other program on
//! the machine is using.
//!
//! # The registry is the setting
//!
//! Nothing here is written to `config.ini`, and that is the one design decision in this module worth
//! defending. The state genuinely lives in the registry: it is machine-wide-ish, it outlives the
//! process, and something else can change it — another file manager's own version of this switch,
//! an uninstaller, a fresh copy of this program in a different folder. A copy in the settings file
//! would be a second answer to a question that already has one, and the two would drift the first
//! time anything else touched the key. So [`state`] reads the registry, and the tick in the menu is
//! whatever it says.
//!
//! What that costs is two registry opens, which is microseconds — but it is still not free enough to
//! do every frame while a menu is open, so [`crate::app::App`] reads it once at startup and again
//! after each toggle. Nothing else can change it while this window has focus.
//!
//! # When the registered executable goes away
//!
//! A registration is a command line, so deleting or moving the executable leaves it naming nothing,
//! and `Win+E` puts up Explorer's own *Application not found*. **There is no fallback and no way to
//! ask for one**: emptying `DelegateExecute` — see `windows/winkey.rs`, where that is the whole
//! mechanism — is precisely the instruction "do not use your handler, use mine", so by then there
//! is no handler left to fall back to. No registry value means "mine, or yours if mine is missing".
//!
//! The dialog is the small half. The large half is that **the tick which would undo it lives in the
//! program that is gone**. Two things answer that, and it is worth being clear that neither
//! *prevents* the dialog — both recover from it afterwards:
//!
//! - [`heal`], the next time any copy of this program starts.
//! - Failing that — nothing of this program left to start — one command:
//!
//!   ```text
//!   reg delete "HKCU\Software\Classes\CLSID\{52205FD8-5DFB-447D-801A-D0B52F2E83E1}" /f
//!   ```
//!
//!   The stock registration is in the machine hive and nothing here ever touches it, so removing
//!   the per-user key is the whole of what it takes to have Explorer back.

use std::path::Path;

#[cfg(windows)]
#[path = "../windows/winkey.rs"]
mod win;

/// What `Win+E` opens right now.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum State {
    /// Explorer — either nothing has claimed the key, or what claimed it named Explorer.
    Explorer,
    /// This executable, at the path it is running from.
    Ours,
    /// A command line that is neither Explorer's nor this one's.
    ///
    /// Another file manager with a setting like this one, or **a different build of this program** —
    /// and the menu treats them alike, because claiming is the answer to both. The tick says "this
    /// build", so it is off either way, and clicking it takes the key over.
    ///
    /// Carries the command line so it can be said out loud rather than guessed at. What it cannot do
    /// is give a stranger's claim back, if they wrote it to the machine hive: see
    /// `windows/winkey.rs`' `release`.
    Other(String),
}

impl State {
    /// Whether the tick is on: this build, this path.
    pub fn ours(&self) -> bool {
        matches!(self, Self::Ours)
    }
}

/// The program a registered command line runs, as written.
///
/// Quoted or not — the stock value for this key is a bare `C:\windows\Explorer.exe`, and what
/// [`claim`] writes is quoted, so both spellings turn up here. An unquoted path *with* spaces in it
/// is cut short, which is a limitation and not a bug worth code: it can only arrive from a third
/// party who wrote a command line Windows would itself run wrong.
fn program(command: &str) -> &str {
    let command = command.trim();
    match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next().unwrap_or(rest),
        None => command.split_whitespace().next().unwrap_or(command),
    }
}

/// Which [`State`] a registered command line amounts to, given the executable this is.
///
/// Split out from [`state`] so it can be tested: the registry half of this module is a fact about
/// the machine, and the classification is a decision. See the tests at the foot of the file, which
/// exercise every branch and touch no registry at all.
///
/// The comparison is a case-insensitive one on the text, not a canonicalisation, and that is enough
/// for the case it has to be right about: the string being compared is one [`claim`] wrote from
/// this same [`std::env::current_exe`], so if it is ours it matches exactly but for the drive
/// letter's case. Anything that does not match is [`State::Other`], which is the honest answer for
/// a path this program cannot prove is itself.
fn classify(command: Option<&str>, ours: &Path) -> State {
    let Some(command) = command else {
        return State::Explorer;
    };
    let exe = program(command);
    if exe.is_empty() {
        return State::Explorer;
    }
    if let Some(ours) = ours.to_str() {
        if exe.eq_ignore_ascii_case(ours) {
            return State::Ours;
        }
    }
    // A command line naming Explorer is Explorer, whoever wrote it and wherever they wrote it. This
    // is the stock value showing through with the delegate gone, and reporting it as somebody's
    // claim would put a "another program has this" note on a machine in its factory state.
    if Path::new(exe)
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("explorer.exe"))
    {
        return State::Explorer;
    }
    State::Other(command.trim().to_owned())
}

/// Whether a registered command line is this program's own wreckage: our executable's *name*, at a
/// path that is not there any more.
///
/// Both halves are load-bearing. **The name**, because a stranger's dangling registration — a file
/// manager somebody uninstalled while it still held the key — is not this program's to take.
/// **The missing file**, because another build that is still on the disk is a choice somebody made,
/// and [`State::Other`] is the honest answer to it.
///
/// `exists` is a parameter rather than a call so the decision is testable without a disk, the way
/// [`classify`] is testable without a registry.
fn dangling_ours(command: &str, ours: &Path, exists: impl Fn(&Path) -> bool) -> bool {
    let registered = Path::new(program(command));
    if exists(registered) {
        return false;
    }
    // `file_name` answers `None` for an empty path, so a command line with no program in it falls
    // out here rather than needing a guard of its own.
    let name = |path: &Path| path.file_name().map(|n| n.to_ascii_lowercase());
    let theirs = name(registered);
    theirs.is_some() && theirs == name(ours)
}

/// What `Win+E` opens, repairing this program's own dangling registration on the way.
///
/// Called instead of [`state`] at startup — see the module header for the failure this recovers
/// from. **It does not prevent that failure**: by the time anything here runs, the dialog has
/// already happened, because running at all is what was impossible.
///
/// The one place in this module that writes to the registry without having been clicked, so the
/// condition is deliberately narrow: already broken *and* already ours, per [`dangling_ours`]. Every
/// other machine gets [`state`]'s two reads and nothing else. It re-points rather than releases,
/// because what is wrong is the path and not the intent — the setting was on — and a refusal is
/// swallowed, since nobody asked for this.
pub fn heal() -> State {
    let current = state();
    #[cfg(windows)]
    {
        let wreckage = match &current {
            State::Other(command) => {
                let ours = std::env::current_exe().unwrap_or_default();
                dangling_ours(command, &ours, |path| path.exists())
            }
            _ => false,
        };
        if wreckage {
            // Through `set`, so the answer is what the registry says afterwards rather than what was
            // asked for — the same reason a click goes through it.
            return set(true).unwrap_or(current);
        }
    }
    current
}

/// What `Win+E` opens right now. Two registry reads; see the module header on why it is not cached
/// in the settings file.
pub fn state() -> State {
    #[cfg(windows)]
    {
        let ours = std::env::current_exe().unwrap_or_default();
        classify(win::registered().as_deref(), &ours)
    }
    #[cfg(not(windows))]
    State::Explorer
}

/// Make `Win+E` open this executable.
///
/// # No argument, deliberately
///
/// The command line is the quoted path and nothing after it. `Win+E` is not a request to open a
/// *place* — the verb is invoked on the File Explorer shell folder itself, so a `%1` in here would
/// expand to that folder's own parsing name, which is a CLSID and not a path this program could go
/// to. With no argument the window opens where it was left, which is what the keystroke means.
///
/// It also sidesteps the one thing that would have needed doing first. This program's command line
/// takes `--open=<path>` and has no bare positional argument at all, so a path arriving without a
/// flag would be dropped and the window would open somewhere else entirely — looking, from outside,
/// exactly like a setting that does not work. Claiming the folder-open verbs is what would need
/// that fixed; see the module header on why they are not claimed.
pub fn claim() -> Result<(), String> {
    #[cfg(windows)]
    {
        let exe = std::env::current_exe()
            .map_err(|e| format!("Cannot find this program's own path: {e}"))?;
        // Quoted, because the path very often has a space in it and an unquoted command line would
        // send Windows looking for `C:\Program.exe`.
        win::claim(&format!("\"{}\"", exe.display())).map_err(complaint)
    }
    #[cfg(not(windows))]
    Err("Win+E is a Windows setting".to_owned())
}

/// Give `Win+E` back to Explorer, removing what [`claim`] wrote.
pub fn release() -> Result<(), String> {
    #[cfg(windows)]
    {
        win::release().map_err(complaint)
    }
    #[cfg(not(windows))]
    Err("Win+E is a Windows setting".to_owned())
}

/// Claim or release, and answer with the state that resulted rather than the one that was asked
/// for.
///
/// The distinction matters at the call site: a write can be refused — by policy on a managed
/// machine, most plausibly — and a tick that went on because it was clicked, rather than because
/// the registry now says so, would be a switch that lies. So the answer is re-read.
pub fn set(on: bool) -> Result<State, String> {
    if on {
        claim()?;
    } else {
        release()?;
    }
    Ok(state())
}

/// A Win32 error code, as a sentence for the status line.
///
/// `ERROR_ACCESS_DENIED` is worth naming because it is the one with a cause somebody can act on —
/// or at least stop trying about — and everything else is a number that at least says which.
#[cfg(windows)]
fn complaint(code: u32) -> String {
    const ACCESS_DENIED: u32 = 5;
    if code == ACCESS_DENIED {
        return "Windows would not let this program change the Win+E setting — \
                a policy on this machine may forbid it."
            .to_owned();
    }
    format!("Windows would not change the Win+E setting (error {code}).")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Pulling the program out of a command line, in the spellings that actually turn up.
    #[test]
    fn the_program_a_command_line_runs() {
        assert_eq!(program(r#""C:\a b\azur.exe""#), r"C:\a b\azur.exe");
        assert_eq!(program(r"C:\windows\Explorer.exe"), r"C:\windows\Explorer.exe");
        assert_eq!(
            program(r#""C:\a b\azur.exe" --open=D:\"#),
            r"C:\a b\azur.exe",
            "an argument is not part of the program"
        );
        assert_eq!(
            program(r"C:\windows\Explorer.exe /select,C:\x"),
            r"C:\windows\Explorer.exe"
        );
        assert_eq!(program("   "), "", "nothing at all is not a program");
    }

    /// Every branch of the classification, and no registry.
    #[test]
    fn what_a_registration_amounts_to() {
        let ours = PathBuf::from(r"D:\Sources\azur\target\claude\debug\azur-files.exe");

        assert_eq!(
            classify(None, &ours),
            State::Explorer,
            "no command line means the shell's own handler has it"
        );

        assert_eq!(
            classify(Some(r#""D:\Sources\azur\target\claude\debug\azur-files.exe""#), &ours),
            State::Ours
        );
        assert_eq!(
            classify(Some(r#""d:\sources\azur\target\claude\debug\AZUR-FILES.EXE""#), &ours),
            State::Ours,
            "a drive letter's case is not a different program"
        );

        assert_eq!(
            classify(Some(r"C:\windows\Explorer.exe"), &ours),
            State::Explorer,
            "the stock value is Explorer however the delegate got out of the way"
        );
        assert_eq!(
            classify(Some(r"C:\WINDOWS\explorer.exe"), &ours),
            State::Explorer
        );

        // A different build of this same program: `Other`, because the tick means *this* build, and
        // clicking it has to be able to move the registration here.
        let elsewhere = r#""D:\Sources\azur\target\release\azur-files.exe""#;
        assert_eq!(
            classify(Some(elsewhere), &ours),
            State::Other(elsewhere.to_owned()),
            "another build of this program is not this build"
        );

        // Somebody else's file manager.
        let theirs = r#""C:\Program Files\GPSoftware\Directory Opus\dopus.exe""#;
        assert_eq!(classify(Some(theirs), &ours), State::Other(theirs.to_owned()));

        assert!(classify(Some(r#""D:\Sources\azur\target\claude\debug\azur-files.exe""#), &ours).ours());
        assert!(!classify(None, &ours).ours());
    }

    /// Which dangling registrations are this program's to repair, and which are not.
    ///
    /// No disk and no registry: `exists` is handed in, so each case states outright whether the
    /// registered path is supposed to be there.
    #[test]
    fn whose_wreckage_a_dangling_registration_is() {
        let ours = PathBuf::from(r"D:\Programs\Azur\azur-files.exe");
        let gone = |_: &Path| false;
        let there = |_: &Path| true;

        // The case this exists for: a build registered out of `target\`, then cleaned away.
        let stale = r#""D:\Sources\azur\target\claude\debug\azur-files.exe""#;
        assert!(
            dangling_ours(stale, &ours, gone),
            "our own executable name at a path that is gone is ours to re-point"
        );
        assert!(
            !dangling_ours(stale, &ours, there),
            "another build still on the disk is somebody's choice, not wreckage"
        );

        // Case matters in a file name here for the same reason it does in `classify`.
        assert!(dangling_ours(
            r#""D:\Sources\azur\target\debug\AZUR-FILES.EXE""#,
            &ours,
            gone
        ));

        // A stranger's, dangling. Not ours to take, however broken it is.
        assert!(
            !dangling_ours(
                r#""C:\Program Files\GPSoftware\Directory Opus\dopus.exe""#,
                &ours,
                gone
            ),
            "a file manager somebody uninstalled does not hand us the key"
        );

        // Explorer's own, and nothing at all.
        assert!(!dangling_ours(r"C:\windows\Explorer.exe", &ours, gone));
        assert!(!dangling_ours("", &ours, gone), "no path is not our path");
        assert!(!dangling_ours("   ", &ours, gone));
    }

    /// Claim `Win+E` and give it straight back, against the real registry.
    ///
    /// **`#[ignore]`, and for the reason every test in this suite stays under `target\sandbox`:**
    /// the registry is not under anything. This one writes to `HKEY_CURRENT_USER` — the developer's
    /// own desktop, the key their own `Win+E` goes through — so it is never part of a run that did
    /// not ask for it by name.
    ///
    /// Two things make that safe rather than merely small. It **refuses to start** unless `Win+E` is
    /// currently Explorer's, so it can never take a real registration off somebody and hand it back
    /// wrong. And the release happens before any assertion, so a failure still leaves the key the
    /// way it was found.
    ///
    /// What it proves is the half that cannot be unit-tested: that [`claim`] produces the exact
    /// registry state a real `Win+E` was measured to honour — a command line naming this executable,
    /// with `DelegateExecute` emptied. [`win::registered`] answers `None` unless *both* are true, so
    /// a command line coming back out is the whole condition. See `windows/winkey.rs`' header for
    /// the measurement, including the two neutralisations that do not work.
    ///
    /// ```text
    /// cargo test --bin azur-files claiming_win_e -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "writes to HKEY_CURRENT_USER; run explicitly"]
    fn claiming_win_e_and_giving_it_back() {
        let before = state();
        assert_eq!(
            before,
            State::Explorer,
            "Win+E is not Explorer's on this machine ({before:?}) — refusing to touch a \
             registration this test did not make"
        );
        let exe = std::env::current_exe().expect("this program's own path");

        // Claim, read, release, read — and only then assert. Nothing between the claim and the
        // release is allowed to panic, or the key would be left behind.
        let claimed = set(true);
        let seen = win::registered();
        let released = set(false);
        let after = state();

        println!("  claimed:  {claimed:?}");
        println!("  read as:  {seen:?}");
        println!("  released: {released:?}");

        assert_eq!(claimed.as_ref().map(State::ours), Ok(true), "the claim did not take");
        let seen = seen.expect(
            "no command line came back, so either the default value or the emptied \
             DelegateExecute did not land — see the table in windows/winkey.rs",
        );
        assert_eq!(
            program(&seen).to_ascii_lowercase(),
            exe.display().to_string().to_ascii_lowercase(),
            "the registered command line does not name this executable"
        );
        assert!(released.is_ok(), "the release failed: {released:?}");
        assert_eq!(after, State::Explorer, "Win+E was not given back");
    }

    /// What **this** machine says `Win+E` does, and what asking cost.
    ///
    /// A diagnostic rather than a test, for the reason `providers_on_this_machine` is one: the
    /// answer is a fact about the machine, so there is nothing here to assert. Read-only — it
    /// claims nothing and releases nothing, because a test that wrote to the registry would be
    /// changing the developer's own desktop.
    ///
    /// ```text
    /// cargo test --bin azur-files win_e_on_this_machine -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "diagnostic; run explicitly"]
    fn win_e_on_this_machine() {
        let started = std::time::Instant::now();
        let state = state();
        let micros = started.elapsed().as_micros();
        println!("  this executable: {:?}", std::env::current_exe());
        println!("  Win+E opens:     {state:?}");
        println!("  asked in:        {micros} µs");
    }
}
