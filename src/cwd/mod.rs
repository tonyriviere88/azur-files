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
    let name = if cfg!(windows) { crate::brand::NAME } else { "azur-files" };
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
mod tests;
