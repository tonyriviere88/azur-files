//! Following the folder a terminal is in.
//!
//! A shell and a file manager side by side are two views of one place, and keeping them in step
//! by hand is the thing you do all day without noticing: `cd` somewhere, then find the same
//! folder again with the mouse. This makes the second half unnecessary — the shell says where it
//! is, and the focused pane goes there.
//!
//! # Why the shell publishes, rather than being asked
//!
//! The obvious design is to read the terminal's working directory. On Windows there is no
//! supported way to do it, and every unsupported way is wrong here:
//!
//! - **There is no handle to read.** [`crate::fs::shell::open_terminal`] goes through
//!   `ShellExecuteW`, which returns no process handle at all — and `wt.exe` is a stub that hands
//!   the request to an existing Windows Terminal and exits, so even `ShellExecuteEx` would hand
//!   back a process that is already gone. The shell is a grandchild of something this program
//!   never saw.
//! - **The PEB is not an API.** `NtQueryInformationProcess` into
//!   `RTL_USER_PROCESS_PARAMETERS.CurrentDirectory` does work, and needs
//!   `NtWow64ReadVirtualMemory64` when the bitness differs, and needs the right process picked
//!   out of a tree of them. Then it is *still* wrong for the shell most likely to be running:
//!   PowerShell's `$PWD` is a provider location, and the process working directory does not
//!   follow it.
//! - **OSC 7 is the correct signal and unreachable.** A shell already announces its directory
//!   with an escape sequence — it is how Windows Terminal implements "duplicate tab here". But an
//!   escape sequence is in the *output stream*, and reading that means owning the pseudoconsole,
//!   which means writing a terminal emulator: ConPTY, a VT parser, a grid, key encoding,
//!   selection, scrollback. That is the largest thing in this program, in order to compete with
//!   the terminal the user already has.
//!
//! So the shell writes it down. A prompt hook — [`hook`] prints one — puts the current directory
//! in `<profile>\Azur\cwd\<pid>.at`, and this watches that folder with the same
//! `ReadDirectoryChangesW` thread that keeps every listing fresh. See [`crate::watch`].
//!
//! # A prompt is the only safe moment, and a prompt hook only ever runs at one
//!
//! Going the other way — this program telling the shell to move — means a `cd` the user did not
//! type. Injected at the wrong moment it lands in the middle of a half-written command line and
//! destroys it. A hook cannot do that: it runs between commands, when there is no command line
//! to trample. So [`Link::send`] leaves the folder in `<pid>.to` and the shell picks it up at its
//! next prompt, which is late by exactly one `Enter` and cannot corrupt anything.
//!
//! # Following is automatic and leading is not
//!
//! The two directions are not equally welcome, so they are not equally eager. A terminal that
//! moves a pane costs nothing — a pane is a view, and the view was going to be changed by hand
//! anyway. A pane that moves a terminal rewrites somebody's session and their history. So the
//! first happens whenever the shell speaks and the second needs a keypress.
//!
//! There is no setting to turn following *on*, because installing the hook is the setting: with
//! no hook, nothing is ever published and none of this does anything. `terminal_link=0` in the
//! settings turns it off again without editing a shell profile.
//!
//! # Only a real move wakes the window
//!
//! The hooks compare against the directory they last published and write nothing when it has not
//! changed, which is what keeps this free: every `Enter` at a prompt would otherwise touch a file,
//! wake the watch thread and cost the window a frame, for a folder it is already showing. As
//! written, the window learns of a `cd` and of nothing else.
//!
//! # What is not read
//!
//! The newest `.at` file, and no other. "The terminal you last typed in" is the one you meant,
//! and a stale file cannot win because its timestamp does not move. A file whose contents are not
//! a folder that exists is ignored rather than reported — a shell can be sitting in a directory
//! that has since been deleted, or in one of its own that is not a Windows path at all.

use std::path::{Path, PathBuf};

/// Which shell a [`hook`] is written for.
///
/// Two, and they cover what a developer on Windows actually sits in. `cmd.exe` is absent because
/// it cannot do this: `PROMPT` interpolates variables and cannot run a command, so there is no
/// point in a `cmd` session at which anything could publish.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shell {
    /// Git Bash, MSYS2, and any other bash — including WSL, whose `/mnt/d/…` this understands.
    Bash,
    /// PowerShell, either edition.
    PowerShell,
}

impl Shell {
    /// What `--terminal-hook=` accepts. Bash first, and it is also what the bare flag means.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "" | "bash" | "sh" | "zsh" | "git" | "gitbash" | "git-bash" | "msys" => Some(Self::Bash),
            "pwsh" | "powershell" | "ps" | "ps1" => Some(Self::PowerShell),
            _ => None,
        }
    }
}

/// A terminal and the focused pane, kept in step.
pub struct Link {
    /// Where shells publish. `None` if there is no profile directory to put it in, which turns
    /// every method here into a no-op.
    dir: Option<PathBuf>,
    /// The folder last read out of a `.at` file.
    ///
    /// The comparison that matters, and it is against the folder rather than the timestamp: a
    /// shell republishing the folder it is already in must not drag a pane back from wherever the
    /// user has since browsed to. Only a `cd` moves anything.
    ///
    /// A resolved path and not the published text, because the same folder has two spellings in
    /// the two shells — `pwd -W` gives `D:/x` and `$PWD.Path` gives `D:\x`. Comparing the text
    /// meant that a folder [`Link::send`] had just handed over came back looking like a move, and
    /// the pane it came from navigated to where it already was.
    ///
    /// Seeded at startup from whatever is already there, so a shell that published yesterday does
    /// not navigate a window that has only just opened.
    seen: Option<PathBuf>,
}

impl Link {
    pub fn new() -> Self {
        let mut link = Self { dir: dir(), seen: None };
        if let Some(dir) = &link.dir {
            // Best effort, and the hook checks for the folder rather than creating it: a shell
            // profile that silently makes directories in a user's roaming profile is a shell
            // profile doing more than it was asked to.
            let _ = std::fs::create_dir_all(dir);
            prune(dir);
        }
        // The baseline. Everything already published happened before this window existed.
        link.seen = link.newest().and_then(|(_, at)| windows_path(&at));
        link
    }

    /// The folder to watch, for the caller to add to [`crate::watch::Watch::keep`].
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Somewhere a terminal has moved to since this was last asked.
    ///
    /// `None` on every other kind of change, which is most of them: a prompt that republished the
    /// same folder, a `.to` file being collected, a shell exiting.
    pub fn arrived(&mut self) -> Option<PathBuf> {
        let (_, at) = self.newest()?;
        let path = windows_path(&at)?;
        if Some(&path) == self.seen.as_ref() {
            return None;
        }
        self.seen = Some(path.clone());
        path.is_dir().then_some(path)
    }

    /// Ask the terminal to change to this folder. False if there is no terminal to ask.
    ///
    /// Whichever shell published most recently, which is the one the user last typed in. It moves
    /// at its next prompt — see the module header — so this returning true means the request was
    /// left where the shell will find it, not that the shell has moved.
    pub fn send(&mut self, to: &Path) -> bool {
        let Some((at, _)) = self.newest() else {
            return false;
        };
        let Some(pid) = at.file_stem().map(|stem| stem.to_owned()) else {
            return false;
        };
        // Forward slashes, because one string has to satisfy both shells: `cd "D:\x"` in bash
        // reads the backslash as an escape, and both `cd` and `Set-Location` take `D:/x`.
        let text = to.to_string_lossy().replace('\\', "/");
        let mut file = at.with_file_name(pid);
        file.set_extension("to");
        if std::fs::write(&file, text.as_bytes()).is_err() {
            return false;
        }
        // The shell will publish this folder the moment it gets there, and that must not read as
        // a `cd` to be followed — the pane it would move is the pane the folder came from.
        self.seen = Some(to.to_path_buf());
        true
    }

    /// The most recently published folder, and the file it came from.
    fn newest(&self) -> Option<(PathBuf, String)> {
        let dir = self.dir.as_ref()?;
        let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "at") {
                continue;
            }
            let Ok(at) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if best.as_ref().is_none_or(|(best, _)| at > *best) {
                best = Some((at, path));
            }
        }
        let (_, path) = best?;
        let text = std::fs::read_to_string(&path).ok()?;
        // A BOM is what PowerShell 5.1 writes with `-Encoding UTF8`, and a leading U+FEFF turns
        // the rest of the line into a path that does not exist.
        let text = text.trim_start_matches('\u{feff}').trim().to_owned();
        (!text.is_empty()).then_some((path, text))
    }

    /// A link against a stated folder, since [`dir`] refuses to answer in a test at all.
    ///
    /// Reachable from `app`'s tests as well as this module's: the decision the window makes when a
    /// terminal moves is worth pinning down, and it cannot be reached without one of these.
    #[cfg(test)]
    pub(crate) fn for_tests(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        let mut link = Self { dir: Some(dir), seen: None };
        link.seen = link.newest().and_then(|(_, at)| windows_path(&at));
        link
    }
}

impl Default for Link {
    fn default() -> Self {
        Self::new()
    }
}

/// Where shells publish, alongside the settings rather than in a temporary folder.
///
/// It has to be somewhere both halves can name without being told: this program derives it, and
/// the [`hook`] carries the same path as a literal, which is what makes a `YAFE_PROFILE` install
/// work rather than quietly pairing with somebody else's.
///
/// **Never in a test.** Same rule as [`crate::config::Config::save`], and for a stronger reason:
/// [`prune`] *deletes* files here, and a test that ran against a real profile would be deleting
/// out of one.
fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let name = if cfg!(windows) { crate::brand::NAME } else { "azur-file-explorer" };
    crate::config::base_dir().map(|base| base.join(name).join("cwd"))
}

/// Drop what dead shells left behind.
///
/// By whether the process is still alive, and deliberately not by age: the hooks publish only
/// when the folder changes, so a shell that has sat in one place for a fortnight has a fortnight-old
/// file and is still the terminal the user is typing in. Age would delete the pairing out from
/// under a live session.
///
/// A recycled process id can keep a dead shell's file alive, which costs nothing — a stale file's
/// timestamp never moves, so it can never be the newest, and a `.to` written for it is a hundred
/// bytes nobody reads.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "at" && ext != "to") {
            continue;
        }
        let alive = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| stem.parse::<u32>().ok())
            .is_some_and(running);
        if !alive {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether a process id belongs to something still running.
#[cfg(windows)]
fn running(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    // SAFETY: the handle is closed on the one path that produces one.
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            // Access denied means it exists and belongs to somebody else, which is still alive.
            Err(error) => {
                error.code() == windows::Win32::Foundation::ERROR_ACCESS_DENIED.to_hresult()
            }
        }
    }
}

/// Nothing to ask, so nothing is thrown away.
#[cfg(not(windows))]
fn running(_pid: u32) -> bool {
    true
}

/// Turn whatever a shell published into a path this program can navigate to.
///
/// Four spellings of the same folder reach here, because a shell on Windows may or may not be
/// living in a translated file system:
///
/// | published | means |
/// | --- | --- |
/// | `D:\Sources` or `D:/Sources` | itself — `pwd -W` in Git Bash, and `$PWD.Path` |
/// | `/d/Sources` | MSYS and Cygwin, when `pwd -W` was not available |
/// | `/mnt/d/Sources` | WSL |
/// | `//server/share` | a UNC path, either slash |
///
/// Anything else is `None`, and the two that matter are `/usr/bin` and `/tmp`: they are real
/// directories inside the shell's own root, whose location on this file system cannot be derived
/// from the path. Guessing would navigate somewhere that is not where the terminal is.
pub fn windows_path(text: &str) -> Option<PathBuf> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    // A forward slash cannot be part of a Windows file name, so rewriting them is safe wherever
    // this decides the text was already a Windows path. Same argument as [`crate::fs::normalize`].
    let windows = |text: &str| Some(PathBuf::from(text.replace('/', "\\")));

    if text.starts_with("//") || text.starts_with(r"\\") {
        return windows(text);
    }
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return windows(text);
    }

    // `/mnt/` first: `/mnt/d/x` is WSL's D drive, and stripping the bare slash would read it as a
    // drive called `m`.
    let rest = text.strip_prefix("/mnt/").or_else(|| text.strip_prefix('/'))?;
    let mut chars = rest.chars();
    let letter = chars.next()?.to_ascii_uppercase();
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    match chars.next() {
        // `/d` is the root of the drive.
        None => Some(PathBuf::from(format!("{letter}:\\"))),
        // The letter and the slash are both ASCII, so byte 2 is a character boundary.
        Some('/') => windows(&format!("{letter}:/{}", &rest[2..])),
        // `/usr`, `/tmp`, `/home` — inside the shell's root, and not derivable from here.
        Some(_) => None,
    }
}

/// Make the folder shells publish into, so a hook installed before the window has ever run works
/// from the next prompt rather than from the next launch.
///
/// The hooks check for this folder and never create it — a shell profile that makes directories in
/// somebody's roaming profile is a shell profile doing more than it was asked to — so somebody has
/// to, and printing the hook is the moment it is being asked for. Without this the first install
/// looks broken: `--terminal-hook` names a folder that is not there, the hook returns quietly at
/// every prompt, and nothing at all happens until the window has been opened once.
pub fn ensure_dir() {
    if let Some(dir) = dir() {
        let _ = std::fs::create_dir_all(dir);
    }
}

/// The prompt hook to paste into a shell profile, for whichever shell.
///
/// Printed rather than installed. `--terminal-hook` sends it to standard output and this program
/// never opens `$PROFILE` or `.bashrc` — appending to somebody's shell profile without being
/// asked is a change to how every future session of theirs starts, and the one-line redirect that
/// does it is something the person running a terminal can write themselves.
pub fn hook(shell: Shell) -> Option<String> {
    dir().map(|dir| hook_in(&dir, shell))
}

/// [`hook`], against a stated folder — which is what makes it checkable without a profile
/// directory anywhere in it.
fn hook_in(dir: &Path, shell: Shell) -> String {
    let name = crate::brand::NAME;
    match shell {
        // Forward slashes throughout: a backslash inside a bash string is an escape.
        Shell::Bash => {
            let dir = dir.to_string_lossy().replace('\\', "/");
            format!(
                "\
# {name}: publish this shell's folder, and collect one sent from the window.
__azur_cwd() {{
    local d='{dir}'
    [ -d \"$d\" ] || return 0
    # A folder the window has asked for. Taken at a prompt, which is the only moment at which
    # changing directory cannot land in the middle of a half-typed command.
    if [ -f \"$d/$$.to\" ]; then
        local to
        to=$(cat \"$d/$$.to\" 2>/dev/null)
        rm -f \"$d/$$.to\"
        [ -n \"$to\" ] && [ -d \"$to\" ] && cd \"$to\"
    fi
    # `pwd -W` is a builtin in Git Bash and MSYS2 and gives the Windows spelling, so this costs
    # no process. Plain `pwd` is the fallback, and the window understands `/d/x` too.
    local now
    now=$( {{ pwd -W 2>/dev/null || pwd; }} )
    # Only on a real move. Writing every prompt would wake the window for a folder it is
    # already showing.
    if [ \"$now\" != \"$__azur_at\" ]; then
        __azur_at=$now
        printf '%s\\n' \"$now\" > \"$d/$$.at\"
    fi
}}
# Kept in front of whatever else already runs at the prompt, and added once however many times
# this file is sourced.
case \"$PROMPT_COMMAND\" in
    *__azur_cwd*) ;;
    *) PROMPT_COMMAND=\"__azur_cwd${{PROMPT_COMMAND:+; $PROMPT_COMMAND}}\" ;;
esac
"
            )
        }
        Shell::PowerShell => {
            let dir = dir.to_string_lossy();
            format!(
                "\
# {name}: publish this shell's folder, and collect one sent from the window.
$global:AzurCwd = '{dir}'
# Whatever prompt was already in force — oh-my-posh, starship, or the built-in one — is called
# through rather than replaced, so this goes *after* whatever sets it up. Captured once, or
# sourcing this file twice would leave the hook calling itself.
if (-not $global:AzurPrompt) {{ $global:AzurPrompt = $function:prompt }}
function global:prompt {{
    if (Test-Path -LiteralPath $global:AzurCwd) {{
        # A folder the window has asked for. Taken at a prompt, which is the only moment at
        # which changing directory cannot land in the middle of a half-typed command.
        $to = Join-Path $global:AzurCwd \"$PID.to\"
        if (Test-Path -LiteralPath $to) {{
            $dest = (Get-Content -LiteralPath $to -Raw -Encoding UTF8 -ErrorAction SilentlyContinue)
            Remove-Item -LiteralPath $to -Force -ErrorAction SilentlyContinue
            if ($dest) {{ $dest = $dest.Trim() }}
            if ($dest -and (Test-Path -LiteralPath $dest -PathType Container)) {{
                Set-Location -LiteralPath $dest
            }}
        }}
        # Only a file system location, and only on a real move: `$PWD` can be `HKLM:\\` or
        # `Cert:\\`, which are not folders, and writing every prompt would wake the window for a
        # folder it is already showing.
        if ($PWD.Provider.Name -eq 'FileSystem' -and $PWD.Path -ne $global:AzurAt) {{
            $global:AzurAt = $PWD.Path
            Set-Content -LiteralPath (Join-Path $global:AzurCwd \"$PID.at\") -Value $PWD.Path -Encoding UTF8
        }}
    }}
    & $global:AzurPrompt
}}
"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder of this test's own, under the scratch directory rather than a profile.
    ///
    /// [`dir`] refuses to answer in a test at all, so nothing here can reach a real one — this is
    /// only somewhere to put files that [`prune`] is allowed to delete.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("azur-cwd-tests").join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch folder");
        dir
    }

    fn publish(dir: &Path, pid: u32, path: &str) {
        // The mtime is what picks the newest, and two writes in the same tick can land on the
        // same one — so each publication is separated enough to be ordered.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join(format!("{pid}.at")), path).expect("published");
    }

    /// The whole of the follow direction: a `cd` moves the window, and nothing else does.
    ///
    /// The second half is the one worth holding down. Every prompt used to publish, so pressing
    /// Enter in the terminal dragged the pane back to the shell's folder from wherever the user
    /// had browsed to — which makes the feature actively hostile. Both halves defend against it:
    /// the hook writes nothing when the folder has not changed, and this compares contents rather
    /// than timestamps in case something else does.
    #[test]
    fn only_a_change_of_folder_is_followed() {
        let dir = scratch("followed");
        let here = std::env::current_dir().expect("a working directory");
        let parent = here.parent().expect("and a parent").to_path_buf();

        // Something published before the window opened is where the shell is, not a move.
        publish(&dir, 4242, &here.to_string_lossy());
        let mut link = Link::for_tests(dir.clone());
        assert_eq!(link.arrived(), None, "a window opening must not navigate itself");

        // A `cd`.
        publish(&dir, 4242, &parent.to_string_lossy());
        assert_eq!(link.arrived(), Some(parent.clone()));

        // The same folder again — a prompt, not a move.
        publish(&dir, 4242, &parent.to_string_lossy());
        assert_eq!(link.arrived(), None, "pressing Enter is not a `cd`");

        // And back.
        publish(&dir, 4242, &here.to_string_lossy());
        assert_eq!(link.arrived(), Some(here));
    }

    /// The newest publication wins, and a shell sitting still cannot outvote the one being used.
    #[test]
    fn the_terminal_last_typed_in_is_the_one_that_is_followed() {
        let dir = scratch("newest");
        let here = std::env::current_dir().expect("a working directory");
        let parent = here.parent().expect("and a parent").to_path_buf();

        publish(&dir, 1, &here.to_string_lossy());
        let mut link = Link::for_tests(dir.clone());

        // A second shell, moving.
        publish(&dir, 2, &parent.to_string_lossy());
        assert_eq!(link.arrived(), Some(parent.clone()));

        // The first shell is still sitting in its own folder and must not pull anything back.
        assert_eq!(link.arrived(), None);

        // And it is the second shell that a send goes to, since it published last.
        assert!(link.send(&here));
        assert!(dir.join("2.to").is_file(), "sent to the shell last typed in");
        assert!(!dir.join("1.to").exists());
    }

    /// What `send` leaves behind is what the hook expects, and the folder is not followed back.
    #[test]
    fn a_folder_sent_over_is_written_for_either_shell_and_not_followed_back() {
        let dir = scratch("sent");
        let here = std::env::current_dir().expect("a working directory");
        publish(&dir, 7, &here.to_string_lossy());
        let mut link = Link::for_tests(dir.clone());

        let target = here.join("src");
        assert!(link.send(&target));
        let written = std::fs::read_to_string(dir.join("7.to")).expect("a request");
        assert!(!written.contains('\\'), "a backslash in `cd \"...\"` is an escape: {written}");
        assert_eq!(windows_path(&written), Some(target.clone()));

        // The shell will arrive there and publish it, and that echo must not navigate the pane
        // the folder came from.
        publish(&dir, 7, &target.to_string_lossy());
        assert_eq!(link.arrived(), None, "the window followed its own request back");
    }

    /// Nothing is published, or what is published is not a folder.
    #[test]
    fn an_empty_folder_and_an_unusable_path_are_both_quiet() {
        let dir = scratch("quiet");
        let mut link = Link::for_tests(dir.clone());
        assert_eq!(link.arrived(), None);
        assert!(!link.send(Path::new(r"C:\")), "nowhere to send it");

        // A shell in a directory that has since been deleted, and one in a directory that is not
        // a Windows path at all.
        publish(&dir, 3, r"D:\gone-a4f1c9");
        assert_eq!(link.arrived(), None);
        publish(&dir, 3, "/usr/local/bin");
        assert_eq!(link.arrived(), None);
    }

    /// Dead shells are cleared out and live ones are left alone — including one that has not
    /// moved for a long time, which is why this is not done by age.
    #[test]
    fn pruning_keeps_the_shells_that_are_still_running() {
        let dir = scratch("prune");
        // This process is the live "shell"; a pid nothing can be using is the dead one.
        let mine = std::process::id();
        std::fs::write(dir.join(format!("{mine}.at")), r"C:\").expect("live");
        std::fs::write(dir.join("4294967294.at"), r"C:\").expect("dead");
        std::fs::write(dir.join("4294967294.to"), r"C:\").expect("dead request");
        // Not ours, and not a process id either.
        std::fs::write(dir.join("notes.txt"), "leave me").expect("unrelated");

        prune(&dir);
        assert!(dir.join(format!("{mine}.at")).is_file(), "a running shell was cleared out");
        assert!(!dir.join("4294967294.at").exists());
        assert!(!dir.join("4294967294.to").exists());
        assert!(dir.join("notes.txt").is_file(), "somebody else's file was deleted");
    }

    /// Every spelling of a folder that a shell on Windows can publish.
    #[test]
    fn a_published_folder_is_understood_in_every_spelling() {
        let cases = [
            (r"D:\Sources\x", Some(r"D:\Sources\x")),
            ("D:/Sources/x", Some(r"D:\Sources\x")),
            ("/d/Sources/x", Some(r"D:\Sources\x")),
            ("/mnt/d/Sources/x", Some(r"D:\Sources\x")),
            ("/d", Some(r"D:\")),
            ("/c/", Some(r"C:\")),
            ("//server/share/x", Some(r"\\server\share\x")),
            (r"\\server\share", Some(r"\\server\share")),
            // Inside the shell's own root: a real directory whose place on this file system
            // cannot be worked out from the path.
            ("/usr/bin", None),
            ("/tmp", None),
            ("/", None),
            ("", None),
            ("   ", None),
        ];
        for (text, want) in cases {
            assert_eq!(
                windows_path(text).as_deref(),
                want.map(Path::new),
                "{text:?}"
            );
        }
        // A trailing newline is what both shells write, and it is not part of the path.
        assert_eq!(windows_path("D:/x\n").as_deref(), Some(Path::new(r"D:\x")));
    }

    /// Both hooks carry the folder this program actually derived, in the spelling their own shell
    /// can read.
    ///
    /// The path is the part that has to be right: it is what pairs the two halves, so an install
    /// under `YAFE_PROFILE` publishes somewhere this program is looking rather than into the
    /// default profile beside it.
    #[test]
    fn a_hook_carries_the_folder_in_its_own_shells_spelling() {
        let dir = Path::new(r"C:\Users\x\AppData\Roaming\Azur\cwd");

        let bash = hook_in(dir, Shell::Bash);
        let quoted = bash.lines().find(|line| line.contains("d=")).expect("the folder");
        assert_eq!(quoted.trim(), "local d='C:/Users/x/AppData/Roaming/Azur/cwd'");
        assert!(
            !quoted.contains('\\'),
            "a backslash inside a bash string is an escape: {quoted}"
        );
        assert!(bash.contains("$$.at") && bash.contains("$$.to"));
        assert!(bash.contains("pwd -W"), "the builtin is what makes this free");
        assert!(bash.contains("*__azur_cwd*"), "sourcing twice must add it once");

        let pwsh = hook_in(dir, Shell::PowerShell);
        assert!(pwsh.contains(&format!("$global:AzurCwd = '{}'", dir.display())), "{pwsh}");
        assert!(pwsh.contains("$PID.at") && pwsh.contains("$PID.to"));
        assert!(pwsh.contains("FileSystem"), "`$PWD` can be a registry location");
        assert!(pwsh.contains("if (-not $global:AzurPrompt)"), "sourcing twice must not recurse");
        assert!(pwsh.contains("& $global:AzurPrompt"), "somebody's own prompt has to survive");
    }

    #[test]
    fn the_shells_are_named_the_way_people_would_type_them() {
        for text in ["bash", "Bash", " git-bash ", "zsh", ""] {
            assert_eq!(Shell::parse(text), Some(Shell::Bash), "{text:?}");
        }
        for text in ["pwsh", "PowerShell", "ps1"] {
            assert_eq!(Shell::parse(text), Some(Shell::PowerShell), "{text:?}");
        }
        assert_eq!(Shell::parse("cmd"), None, "cmd has no prompt hook to install");
        assert_eq!(Shell::parse("fish"), None);
    }
}
