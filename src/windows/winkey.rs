//! Whether `Win+E` opens this program: one registry key, and one quirk that had to be measured.
//!
//! The Windows half of [`crate::shell::winkey`]. Registry reads and writes under
//! `HKEY_CURRENT_USER`, and nothing else — no COM, no elevation, no installer, and nothing left
//! behind that [`release`] cannot take back.
//!
//! Nothing is taken from the parent module, as in `windows/providers.rs` and for the same reason: a
//! key path is a string, so this needs neither `wide` nor a `Path` — only the key guard in
//! [`crate::shell::registry`] and [`crate::shell::nul_terminated`].
//!
//! # Which key, and why that one
//!
//! `Win+E` is not a hotkey any program can register. `RegisterHotKey` refuses the combination —
//! the shell holds it — so the keystroke cannot be taken by asking for it. What it *does* is
//! invoke the `opennewwindow` verb on the shell folder called File Explorer, which is a CLSID with
//! a registry key like any other. Rewriting that verb's command line is therefore the whole of the
//! mechanism, and it is the same one Directory Opus and XYplorer use for their own version of this
//! setting.
//!
//! Under `HKEY_CURRENT_USER\Software\Classes` rather than `HKEY_CLASSES_ROOT`: that is the
//! per-user half of the same merged view, it needs no administrator, and it changes nothing for
//! anybody else who uses the machine.
//!
//! # The quirk: `DelegateExecute` has to be emptied, not removed
//!
//! The stock key carries a `DelegateExecute` value naming a COM handler, and **while that value is
//! visible the shell builds the handler and never reads the command line at all**. Every guide to
//! this says "delete `DelegateExecute`", which is advice for editing `HKEY_CLASSES_ROOT` directly.
//! It cannot be followed from here, and the reason is the merge:
//!
//! `HKEY_CLASSES_ROOT` merges `HKLM\Software\Classes` and `HKCU\Software\Classes` **per value and
//! not per key**. Creating the key in `HKCU` does not shadow the machine's copy of it; the
//! `DelegateExecute` sitting in `HKLM` still shows through, and there is no way to delete an `HKLM`
//! value from `HKCU`. So the value has to be *overridden* with something the shell will not use.
//!
//! Three candidates, all three tried against a real `Win+E`:
//!
//! | what is written to `DelegateExecute` | what `Win+E` did |
//! | --- | --- |
//! | nothing — leave `HKLM`'s value showing through | Explorer opened; the command line was ignored |
//! | an unregistered CLSID | Explorer opened; the command line was ignored |
//! | **an empty string** | **the command line ran** |
//!
//! The middle row is the surprise, and it is why this writes an empty string rather than a
//! plausible-looking dud: a CLSID that fails to create still leaves the shell on the delegate path,
//! falling back to Explorer rather than to the command line. Only a value it cannot read as a CLSID
//! at all sends it to the command line. See [`claim`].

use crate::shell::nul_terminated;
use crate::shell::registry::Key;
use windows_sys::Win32::System::Registry::HKEY;

/// The shell folder `Win+E` opens, spelled as the registry spells it — **relative to a classes
/// root**, which is the distinction the two helpers below exist for.
///
/// Written out rather than built from a `GUID`, for the reason `windows/providers.rs` gives about
/// the same choice: this is a **key name**, and formatting a GUID back into it would be a
/// conversion in order to compare against the text it came from.
const FILE_EXPLORER: &str = r"CLSID\{52205FD8-5DFB-447D-801A-D0B52F2E83E1}";

/// Where the per-user classes live under `HKEY_CURRENT_USER`.
///
/// **`HKEY_CLASSES_ROOT` *is* this key**, merged with the machine's copy — it is not a hive that
/// contains it. So the same registration has two spellings depending on which root it is reached
/// through, and a path built for one root and passed to the other simply is not found. That is a
/// silent failure and it reads as good news: [`registered`] answered "Explorer opens `Win+E`", which
/// is the correct answer on an untouched machine and was, for a while, being given for the wrong
/// reason. Hence [`mine`] and [`merged`], and no bare `format!` anywhere below.
const CLASSES: &str = r"Software\Classes";

/// The verb's command key as reached through `HKEY_CLASSES_ROOT`.
fn merged() -> String {
    format!(r"{FILE_EXPLORER}\shell\opennewwindow\command")
}

/// A classes-relative path as reached through `HKEY_CURRENT_USER`.
fn mine(path: &str) -> String {
    format!(r"{CLASSES}\{path}")
}

/// The keys [`claim`] creates under `HKEY_CURRENT_USER`, innermost first — which is the order
/// [`release`] unwinds them in.
///
/// All four are ours to remove only because all four are ours to create: the stock registration
/// lives in `HKLM`, so on a machine where this setting has never been touched none of these exist
/// under `HKEY_CURRENT_USER` at all. [`release`] still checks each one is empty before deleting it,
/// because "never been touched" is an assumption and somebody else's per-user override is not this
/// program's to throw away.
fn chain() -> [String; 4] {
    [
        mine(&merged()),
        mine(&format!(r"{FILE_EXPLORER}\shell\opennewwindow")),
        mine(&format!(r"{FILE_EXPLORER}\shell")),
        mine(FILE_EXPLORER),
    ]
}

/// The value that decides whether the command line is read at all. See the module header.
const DELEGATE: &str = "DelegateExecute";

/// A string value under an open key: the key's own default when `name` is `None`, and a named one
/// otherwise.
///
/// `None` for a value that is not there and for one that is not text. **`Some("")` for one that is
/// there and empty**, which is the distinction the whole module turns on: an empty
/// [`DELEGATE`] is not a missing `DelegateExecute`, it is the override that makes the command line
/// live. `windows/providers.rs`' near-twin of this function folds the two together, because there
/// an empty ProgID and no ProgID mean the same thing. Here they could not mean less alike.
fn value(key: &Key, name: Option<&str>) -> Option<String> {
    use windows_sys::Win32::System::Registry::{REG_EXPAND_SZ, REG_SZ};

    // A command line, which `MAX_PATH` quoted plus a switch or two fits inside twice over. A value
    // longer than this is not one this program put there and not one it can act on.
    let (kind, text) = key.string::<1024>(name)?;
    // `REG_EXPAND_SZ` as well as `REG_SZ`: the stock command line for this very key is one, and a
    // value read here is only ever compared or shown — never run — so nothing needs expanding.
    if kind != REG_SZ && kind != REG_EXPAND_SZ {
        return None;
    }
    // An empty value is two bytes of terminator, or occasionally none at all, and both arrive here
    // as an empty string rather than as nothing.
    Some(text)
}

/// Open a key under one of the roots for reading. `None` when it is not there.
fn open_read(root: HKEY, path: &str) -> Option<Key> {
    use windows_sys::Win32::System::Registry::KEY_READ;

    Key::open(root, path, KEY_READ).ok()
}

/// The command line `Win+E` would actually run, or `None` when the shell's own handler has it.
///
/// Read from `HKEY_CLASSES_ROOT` rather than from the key [`claim`] writes, deliberately: the
/// question is what the keystroke *does*, and the answer is the merged view of both hives. That
/// also makes this honest about a registration this program did not make — a file manager that
/// claimed `Win+E` by editing the machine hive shows up here, where reading only `HKCU` would have
/// reported Explorer and quietly disagreed with the desktop.
///
/// `None` when [`DELEGATE`] is present and non-empty, because that is precisely the state in which
/// the command line is dead text; see the module header.
pub fn registered() -> Option<String> {
    use windows_sys::Win32::System::Registry::HKEY_CLASSES_ROOT;

    let key = open_read(HKEY_CLASSES_ROOT, &merged())?;
    let delegate = value(&key, Some(DELEGATE));
    let command = value(&key, None);
    // Both reads are done with.
    drop(key);

    if delegate.is_some_and(|d| !d.is_empty()) {
        return None;
    }
    command.filter(|c| !c.is_empty())
}

/// Point the verb at `command`, and empty [`DELEGATE`] so the shell reads it.
///
/// `command` is a full command line and arrives already quoted — see
/// [`crate::shell::winkey::claim`], which is also where the argument list is argued.
///
/// The two writes are in this order on purpose: the command line lands before the value that makes
/// it live, so a failure between them leaves `Win+E` opening Explorer rather than pointing at a
/// half-written setting.
pub fn claim(command: &str) -> Result<(), u32> {
    use windows_sys::Win32::System::Registry::{
        RegCreateKeyExW, RegSetValueExW, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE, REG_SZ,
    };

    let wide = nul_terminated(mine(&merged()));
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `wide` outlives the call; the class and security-attribute pointers are null, which
    // the API documents as "default"; `key` is only used when the call reports success. Every
    // missing key in the path is created, which is why there is no walk down to it here.
    let created = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };
    if created != 0 {
        return Err(created);
    }
    let key = Key(key);

    let write = |name: Option<&str>, text: &str| {
        let wide_name = name.map(nul_terminated);
        let data = nul_terminated(text);
        // SAFETY: `key` is open for writing, `data` is a NUL-terminated UTF-16 buffer that outlives
        // the call, and the byte count describes exactly it — terminator included, which is what
        // `REG_SZ` wants.
        let set = unsafe {
            RegSetValueExW(
                key.0,
                wide_name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
                0,
                REG_SZ,
                data.as_ptr().cast(),
                std::mem::size_of_val(&data[..]) as u32,
            )
        };
        (set == 0).then_some(()).ok_or(set)
    };

    let wrote = write(None, command).and_then(|()| write(Some(DELEGATE), ""));
    // Created above, and not used again.
    drop(key);
    wrote
}

/// Whether a key has neither subkeys nor values — the one question [`release`] asks before deleting
/// an ancestor it may not have created.
fn empty(path: &str) -> bool {
    use windows_sys::Win32::System::Registry::{RegQueryInfoKeyW, HKEY_CURRENT_USER};

    let Some(key) = open_read(HKEY_CURRENT_USER, path) else {
        // Not there at all, which for the caller's purposes is the same answer: nothing to keep.
        return true;
    };
    let mut subkeys = 0u32;
    let mut values = 0u32;
    // SAFETY: `key` is open for reading; every pointer is either null — the API's "not interested"
    // — or to a local that outlives the call.
    let asked = unsafe {
        RegQueryInfoKeyW(
            key.0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
            &mut subkeys,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut values,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    // Opened above, and not used again.
    drop(key);
    asked == 0 && subkeys == 0 && values == 0
}

/// Give `Win+E` back: remove the two values, then the keys that were only there to hold them.
///
/// The values go first and the keys are unwound innermost outwards, each deleted only if it is
/// [`empty`] — so a per-user override somebody else put under the same CLSID stops the unwind
/// instead of being swept up with it. A value or key that is already gone is not a failure: this
/// has to be safe to call on a machine where the setting was never on, because that is what the
/// menu does the first time somebody unticks something they never ticked.
///
/// What this cannot undo is a registration in the **machine** hive. If [`registered`] reports a
/// command line after a release, another program claimed `Win+E` through `HKLM` and giving it up is
/// that program's to do — see [`crate::shell::winkey::State::Other`].
pub fn release() -> Result<(), u32> {
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyW, RegDeleteValueW, HKEY_CURRENT_USER, KEY_SET_VALUE,
    };

    /// `ERROR_FILE_NOT_FOUND` — a key or value that is not there, which for everything below is the
    /// state this function is trying to reach rather than a failure to reach it.
    const NOT_FOUND: u32 = 2;

    // As `open_read`, with write access because the values are about to go.
    let key = match Key::open(HKEY_CURRENT_USER, &mine(&merged()), KEY_SET_VALUE) {
        Ok(key) => key,
        // Never claimed, or already given back. Which is what the menu does the first time somebody
        // unticks something they never ticked, so it cannot be an error.
        Err(NOT_FOUND) => return Ok(()),
        Err(opened) => return Err(opened),
    };

    let name = nul_terminated(DELEGATE);
    // SAFETY: `key` is open for writing; the name buffer outlives its call, and a null name is how
    // the API is told to delete the key's default value.
    let (default, delegate) = unsafe {
        let default = RegDeleteValueW(key.0, std::ptr::null());
        let delegate = RegDeleteValueW(key.0, name.as_ptr());
        drop(key);
        (default, delegate)
    };
    // **The two values are the whole of the setting**, and the walk below is only tidiness — so a
    // refusal here is what has to be reported, and a refusal there is not. Losing the command line
    // gives `Win+E` back to Explorer whether or not the empty keys that held it ever go.
    if let Some(code) = [default, delegate]
        .into_iter()
        .find(|&code| code != 0 && code != NOT_FOUND)
    {
        return Err(code);
    }

    for path in chain() {
        if !empty(&path) {
            break;
        }
        let wide = nul_terminated(&path);
        // SAFETY: `wide` outlives the call. The key is known to have no subkeys, which is the one
        // thing `RegDeleteKeyW` requires; a key already gone answers `ERROR_FILE_NOT_FOUND`, which
        // is the state this was trying to reach.
        unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, wide.as_ptr()) };
    }
    Ok(())
}
