//! `IContextMenu`: the real context menu, extensions included.
//!
//! The Windows half of [`crate::shell::menu`] — every COM call that builds, reads and invokes a
//! shell menu. The portable half next door owns the types this hands back, so what a menu *is*
//! stays platform-neutral and only the filling of it lives here.

use super::*;
use windows::core::{Interface, PCSTR, PCWSTR, PSTR, PWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    IContextMenu, IContextMenu2, IShellFolder, SHBindToObject, SHBindToParent,
    SHParseDisplayName, CMF_EXPLORE, CMF_NORMAL, CMINVOKECOMMANDINFOEX, GCS_VERBW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemInfoW, HMENU, MENUITEMINFOW,
    MFS_CHECKED, MFS_DEFAULT, MFS_DISABLED, MFS_GRAYED, MFT_SEPARATOR, MENU_ITEM_MASK,
    MIIM_BITMAP, MIIM_FTYPE,
    MIIM_ID, MIIM_STATE, MIIM_STRING, MIIM_SUBMENU, SW_SHOWNORMAL, WM_INITMENUPOPUP,
};

/// The shell's commands are numbered from here, so an id read back out of the menu
/// is turned into a command offset by subtracting it.
const FIRST: u32 = 0x1000;
const LAST: u32 = 0x7FFF;
/// Deep enough for anything real; a guard against a malformed extension.
const MAX_DEPTH: u32 = 4;

/// A PIDL that frees itself.
struct Pidl(*mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: allocated by `SHParseDisplayName`, freed exactly once.
            unsafe { windows::Win32::UI::Shell::ILFree(Some(self.0)) };
        }
    }
}

fn pidl_of(path: &Path) -> Option<Pidl> {
    let wide = crate::shell::wide(path);
    let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
    // SAFETY: `wide` is null-terminated and outlives the call.
    let ok =
        unsafe { SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut raw, 0, None).is_ok() };
    (ok && !raw.is_null()).then_some(Pidl(raw))
}

/// The `IContextMenu` for a selection, or for the folder's background when the selection
/// is empty — which is where New comes from.
///
/// # Why the empty case is a different call and not the same one with no items
///
/// A folder has *two* context menus and they are different objects. `GetUIObjectOf` on the
/// folder's own PIDL, taken from its parent, gives the menu Explorer shows for the folder
/// **as an item in a listing** — Open, Cut, Copy, Create shortcut, Delete, Rename,
/// Properties, and every extension registered under `Directory` and `Folder`.
/// `CreateViewObject` gives the menu Explorer shows for the folder's **background**, and
/// only that one has New in it.
///
/// This was the first of those, and New was simply absent. Not empty — absent, which is
/// why none of the `WM_INITMENUPOPUP` machinery in [`Live::fill`] had anything to do with
/// it: the handler is registered in exactly one place,
/// `HKCR\Directory\Background\shellex\ContextMenuHandlers\New`, so nothing asked for under
/// `Directory` or `Folder` can produce it at all. Measured against a project folder on
/// this machine, both menus read out with every submenu filled:
///
/// | | entries | New |
/// | --- | --- | --- |
/// | `GetUIObjectOf` on the folder item | 29 | absent |
/// | `CreateViewObject` | 12 | Folder, Shortcut and ten registered file types |
///
/// What the view object does *not* carry is Paste, Refresh, View and Sort by. Those belong
/// to Explorer's own view rather than to the shell folder — Explorer synthesises them
/// around this menu — so they are this program's to provide, and it does, on their
/// shortcuts.
///
/// And what it carries but will not *run* is Properties, which belongs to that same view and
/// answers `S_OK` without showing anything when it is asked from anywhere else. That one is
/// invoked against the folder-as-an-item menu above instead — see [`as_an_item`].
unsafe fn context_of(parent: &Path, items: &[PathBuf]) -> Option<IContextMenu> {
    if items.is_empty() {
        let pidl = pidl_of(parent)?;
        // Bound from the desktop, because what is wanted is the folder itself as an
        // `IShellFolder` — not the folder as an item inside its parent, which is what
        // `SHBindToParent` gives and what the selection path below wants.
        let folder: IShellFolder = SHBindToObject(None, pidl.0, None).ok()?;
        return folder.CreateViewObject(HWND::default()).ok();
    }

    // Every item has to be a child of one folder for one `IContextMenu`, which a
    // selection in this program always is.
    let pidls: Vec<Pidl> = items.iter().filter_map(|p| pidl_of(p)).collect();
    let first = pidls.first()?;
    let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
    let folder: IShellFolder = SHBindToParent(first.0, Some(&mut child)).ok()?;

    let mut children: Vec<*const ITEMIDLIST> = Vec::with_capacity(pidls.len());
    children.push(child as *const ITEMIDLIST);
    for pidl in pidls.iter().skip(1) {
        let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHBindToParent::<IShellFolder>(pidl.0, Some(&mut child)).is_ok() && !child.is_null() {
            children.push(child as *const ITEMIDLIST);
        }
    }
    folder.GetUIObjectOf(HWND::default(), &children, None).ok()
}

/// What to ask `QueryContextMenu` for. See [`Depth`] for the measurements behind this.
///
/// `CMF_EXPLORE` on the full one asks for the menu Explorer shows rather than the shorter
/// one a file dialog gets.
///
/// # `CMF_CANRENAME`, and why the menu had no Rename in it
///
/// **The shell does not offer `rename` unless it is asked to.** Measured by `probe_banding` on a
/// text file: without this flag the menu comes back with `cut`, `copy`, `link`, `delete` and
/// `properties` and no rename verb anywhere in it — which is why the Windows 11 tile row this
/// program draws was a row of four for as long as the flag was missing. The flag is the caller
/// saying "the thing showing this menu can rename an item", and the shell adds the entry when it
/// hears it. Explorer sets it whenever its view can rename; this program can, on `F2`, so it does.
///
/// The verb is then answered *here* rather than handed back — see
/// `crate::app::App::ours_rather_than_the_shell_s`. That is not a preference: the shell's own
/// `rename` starts an inline edit in the *view* that hosts the menu, and a menu built out of a bare
/// shell folder has no view, so `InvokeCommand("rename")` has nothing to open. It is the same shape
/// of problem as [`as_an_item`] documents for Properties on a background menu, and this program has
/// its own rename field to put the caret in.
///
/// Not on [`Depth::Fast`], which is the short menu asked for when a file on a share is taking
/// seconds to answer: `CMF_DEFAULTONLY` is a request for the default verb, and adding a rename to
/// that is asking for more work on the one path chosen for asking for less.
fn flags(depth: Depth) -> u32 {
    match depth {
        Depth::Full => CMF_NORMAL | CMF_EXPLORE | windows::Win32::UI::Shell::CMF_CANRENAME,
        Depth::Fast => windows::Win32::UI::Shell::CMF_DEFAULTONLY,
    }
}

/// One open menu's shell state, on the thread that made it.
///
/// `IContextMenu` is apartment-threaded and the `HMENU` belongs to whoever created it,
/// so this never leaves the thread it was made on — which is
/// [`super::Builder`]'s, not the UI's.
pub struct Live {
    context: IContextMenu,
    hmenu: HMENU,
    /// Every submenu found so far, under the id handed out with it.
    submenus: std::collections::HashMap<u32, Sub>,
    /// The next id. Only ever grows, so an id names one submenu for this menu's life.
    next: u32,
}

/// What is needed to fill one submenu.
#[derive(Clone)]
struct Sub {
    menu: HMENU,
    /// The item's position inside its *parent* `HMENU`, which is what
    /// `WM_INITMENUPOPUP` wants — and which is not the entry index, since separators are
    /// coalesced and unusable items dropped on the way out.
    position: u32,
    /// How deep the parent was, for the guard against a malformed extension.
    depth: u32,
    /// The positions of the submenus above this one, root first — so `trail + [position]`
    /// is the route from the top of the menu to this submenu's contents.
    ///
    /// Kept because it is the only way back to an entry once this `HMENU` is gone: see
    /// [`Command::Shell::path`].
    trail: Vec<u32>,
}

impl Drop for Live {
    fn drop(&mut self) {
        // Destroys the submenus with it, which is why they are not tracked for freeing.
        // SAFETY: created by `CreatePopupMenu` here, destroyed exactly once.
        unsafe {
            let _ = DestroyMenu(self.hmenu);
        }
    }
}

impl Live {
    /// Ask the shell, and read the top level. Submenus are left for [`Live::fill`].
    pub fn open(parent: &Path, items: &[PathBuf], depth: Depth) -> Option<(Self, Vec<Entry>)> {
        // SAFETY: the menu is destroyed by `Drop` on every path out, and the
        // interfaces are reference counted.
        unsafe {
            let context = context_of(parent, items)?;
            let hmenu = CreatePopupMenu().ok()?;
            let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags(depth));
            let mut live = Self {
                context,
                hmenu,
                submenus: std::collections::HashMap::new(),
                next: 0,
            };
            let entries = live.read(hmenu, 0, &[]);
            Some((live, entries))
        }
    }

    /// The contents of the submenu with this id, asking the extension to fill it first.
    pub fn fill(&mut self, id: u32) -> Vec<Entry> {
        let Some(sub) = self.submenus.get(&id).cloned() else {
            return Vec::new();
        };
        if sub.depth >= MAX_DEPTH {
            // Deep enough for anything real; a guard against a malformed extension.
            return Vec::new();
        }
        // SAFETY: both handles came out of this menu and are alive as long as it is.
        unsafe {
            // An extension fills its submenu when the menu is about to pop up. This
            // menu never pops up, so it is told to anyway — otherwise Send To and New
            // come back empty.
            init_popup(&self.context, sub.menu, sub.position);
            let mut trail = sub.trail.clone();
            trail.push(sub.position);
            self.read(sub.menu, sub.depth + 1, &trail)
        }
    }

    /// Read one `HMENU` the shell has filled into something drawable.
    ///
    /// `trail` is how this `HMENU` was reached from the top of the menu, and is what every
    /// command read out of it is stamped with. See [`Command::Shell::path`].
    unsafe fn read(&mut self, hmenu: HMENU, depth: u32, trail: &[u32]) -> Vec<Entry> {
        let count = GetMenuItemCount(Some(hmenu));
        if count <= 0 {
            return Vec::new();
        }
        let mut entries: Vec<Entry> = Vec::with_capacity(count as usize);

        for position in 0..count {
            let mask = MIIM_FTYPE | MIIM_STATE | MIIM_ID | MIIM_SUBMENU | MIIM_BITMAP;
            let Some((info, raw)) = item_at(hmenu, position as u32, mask) else {
                continue;
            };

            if info.fType.0 & MFT_SEPARATOR.0 != 0 {
                // Two separators running together, or one at either end, are what a
                // menu assembled from several extensions looks like before anyone
                // tidies it.
                if !matches!(entries.last(), None | Some(Entry { kind: Kind::Separator, .. })) {
                    entries.push(Entry::separator());
                }
                continue;
            }

            let (label, shortcut) = split_label(&raw);
            if label.is_empty() {
                continue;
            }

            let enabled = info.fState.0 & (MFS_DISABLED.0 | MFS_GRAYED.0) == 0;
            let checked = info.fState.0 & MFS_CHECKED.0 != 0;
            // `MFS_DEFAULT` — the entry a double click would have run. **Read, and never drawn.**
            //
            // It was drawn once, as the 2px accent bar a selected row gets, and that was wrong: the
            // default entry is nearly always the first one, so every context menu in the program
            // opened with a blue bar down the side of its top row, where it read as a selection
            // nobody had made rather than as a hint about double-clicking. So for a while it was not
            // read at all, there being nothing else to do with it.
            //
            // There is now. `crate::shell::menu::regroup` bands the menu, and the top band is "the
            // entry the shell considers default" — a question only the shell can answer, and this
            // is the answer. Still not drawn; see `crate::shell::menu::Entry::default`.
            let default = info.fState.0 & MFS_DEFAULT.0 != 0;
            let icon = menu_bitmap(info.hbmpItem);

            // The canonical name, asked for **before** the kinds are told apart — which is the fix
            // for a submenu row arriving anonymous. It used to be asked for only inside the command
            // branch below, so `Ouvrir avec` and `7-Zip` never got one: the popup branch is reached
            // first and returned without asking. They have one. Measured by
            // [`probe_submenu_verbs`]: `openas` on `wID` 4179 and `SevenZip` on 4214, both of which
            // were being read and thrown away. See `crate::shell::menu::Entry::verb`, and note that
            // `Envoyer vers` genuinely has none — a popup with no verb is a real answer, not this
            // bug wearing a different hat.
            let verb = (info.wID >= FIRST && info.wID <= LAST)
                .then(|| canonical_verb(&self.context, (info.wID - FIRST) as usize))
                .flatten();

            let kind = if !info.hSubMenu.is_invalid() && depth < MAX_DEPTH {
                // Noted rather than read. Whether there is anything in it is not known
                // until the extension is asked, and asking is the expensive part.
                let id = self.next;
                self.next += 1;
                self.submenus.insert(
                    id,
                    Sub {
                        menu: info.hSubMenu,
                        position: position as u32,
                        depth,
                        trail: trail.to_vec(),
                    },
                );
                Kind::unfilled(id)
            } else if info.wID >= FIRST && info.wID <= LAST {
                let offset = info.wID - FIRST;
                Kind::Command(Command::Shell {
                    verb: verb.clone(),
                    id: offset,
                    path: trail.to_vec(),
                    label: label.clone(),
                })
            } else {
                // An id outside the range this program handed out is not ours to
                // invoke.
                continue;
            };

            entries.push(Entry {
                label,
                shortcut,
                kind,
                enabled,
                checked,
                icon,
                default,
                verb,
            });
        }

        while matches!(entries.last(), Some(Entry { kind: Kind::Separator, .. })) {
            entries.pop();
        }
        entries
    }
}

/// Tell whoever owns a submenu that it is about to pop up, so that it fills it.
///
/// An extension populates its submenu lazily, on `WM_INITMENUPOPUP`, which a menu that is
/// never shown never gets. Without this Send To, Open With and New come back empty — and,
/// on the way to invoking one of their entries, the *ids* inside them are never assigned
/// either. `wParam` is the submenu's handle and `lParam` its position in its parent, which
/// is what the message carries when Windows sends it for real.
unsafe fn init_popup(context: &IContextMenu, menu: HMENU, position: u32) {
    if let Ok(two) = context.cast::<IContextMenu2>() {
        let _ = two.HandleMenuMsg(
            WM_INITMENUPOPUP,
            WPARAM(menu.0 as usize),
            LPARAM(position as isize),
        );
    }
}

/// One item of an `HMENU`, read whole: the fields `mask` asked for, and the text.
///
/// **Two calls, and the reason is the text.** `MENUITEMINFOW` does not carry a string, it carries a
/// pointer to a buffer the caller supplies — so the first call is asked with no buffer purely to
/// learn `cch`, and the second is given one that size. A single call with a fixed buffer truncates,
/// and what it truncates is `Open with <some application with a long name>`.
///
/// The text is returned raw, exactly as the shell wrote it: ampersands, tab, and all. Splitting it is
/// [`split_label`]'s job and not every caller wants it split.
///
/// Its own function because five places do this — the read, the id lookup, and three probes — and it
/// is a fiddly enough dance that five copies of it is five chances to forget the second call.
unsafe fn item_at(hmenu: HMENU, position: u32, mask: MENU_ITEM_MASK) -> Option<(MENUITEMINFOW, String)> {
    let mut info = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        // `MIIM_STRING` whether the caller asked or not: without it `cch` comes back as whatever it
        // was, and the buffer below would be sized from a number nobody set.
        fMask: mask | MIIM_STRING,
        ..Default::default()
    };
    GetMenuItemInfoW(hmenu, position, true, &mut info).ok()?;
    let mut text = vec![0u16; info.cch as usize + 1];
    info.dwTypeData = PWSTR(text.as_mut_ptr());
    info.cch = text.len() as u32;
    GetMenuItemInfoW(hmenu, position, true, &mut info).ok()?;
    Some((info, String::from_utf16_lossy(&text[..info.cch as usize])))
}

/// The submenu hanging off one position of an `HMENU`, if there is one.
unsafe fn submenu_at(hmenu: HMENU, position: u32) -> Option<HMENU> {
    let mut info = MENUITEMINFOW {
        cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_SUBMENU,
        ..Default::default()
    };
    GetMenuItemInfoW(hmenu, position, true, &mut info).ok()?;
    (!info.hSubMenu.is_invalid()).then_some(info.hSubMenu)
}

/// Every command in one level of a menu, as `(id, label)` — the id already offset from
/// [`FIRST`], the label as [`split_label`] would have given it.
///
/// Walked by position rather than asked for by id, and one `HMENU` rather than the whole
/// tree: `GetMenuItemInfoW` with `fByPosition = false` searches submenus too, and a match
/// found in a *different* submenu from the one the entry came out of would be exactly the
/// confusion [`resolve`] is here to catch.
unsafe fn commands_of(hmenu: HMENU) -> Vec<(u32, String)> {
    let count = GetMenuItemCount(Some(hmenu)).max(0);
    let mut out = Vec::with_capacity(count as usize);
    for position in 0..count {
        let Some((info, raw)) = item_at(hmenu, position as u32, MIIM_ID | MIIM_SUBMENU) else {
            continue;
        };
        // A popup's `wID` means nothing, and must not be allowed to match.
        if !info.hSubMenu.is_invalid() || info.wID < FIRST || info.wID > LAST {
            continue;
        }
        out.push((info.wID - FIRST, split_label(&raw).0));
    }
    out
}
/// Which id in the rebuilt menu is the entry the user clicked, if any.
///
/// The id first, when it still carries the same label. Otherwise the label, when exactly one
/// entry in this level has it. Otherwise nothing at all.
///
/// **The id really does move.** The first context menu of a process is short — measured on
/// this machine, 30 entries against the 35 every menu after it gets, because the modern
/// `IExplorerCommand` entries are not there yet; see the note at the top of this file. So a
/// command read off that first menu and rebuilt a moment later, once the shell is warm, is
/// numbered five entries out. Caught in the act: `Open Git Bash here`, id 86, came back as
/// `Open with Visual Studio` at id 86 in the rebuilt menu.
///
/// A verb is immune to all of this, which is why it is tried first. This is for the third of
/// a menu that has none.
///
/// The uniqueness requirement is not pedantry either: a Windows 11 menu carries the same
/// entry twice over — `Déplacer vers OneDrive` appears once as an `IExplorerCommand` and
/// once as the legacy handler behind it — so "the one with this label" is a question that
/// can have two answers, and two answers is no answer.
unsafe fn resolve(hmenu: HMENU, id: u32, label: &str) -> Option<u32> {
    let commands = commands_of(hmenu);
    if commands.iter().any(|(found, name)| *found == id && name == label) {
        return Some(id);
    }
    let mut matching = commands.iter().filter(|(_, name)| name == label);
    let first = matching.next()?;
    matching.next().is_none().then_some(first.0)
}

/// A menu label as the shell writes it, split into what to draw.
///
/// `&` marks the keyboard accelerator and `&&` is a literal ampersand; a tab
/// separates the label from its shortcut text.
pub(super) fn split_label(raw: &str) -> (String, String) {
    let (left, right) = match raw.split_once('\t') {
        Some((left, right)) => (left, right.trim().to_owned()),
        None => (raw, String::new()),
    };
    let mut label = String::with_capacity(left.len());
    let mut chars = left.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '&' {
            if chars.peek() == Some(&'&') {
                chars.next();
                label.push('&');
            }
            continue;
        }
        label.push(c);
    }
    (label.trim().to_owned(), right)
}

/// The canonical verb for a command, if it has one.
///
/// Stable across processes and threads, which is what lets the command be invoked
/// later against a freshly obtained `IContextMenu`.
unsafe fn canonical_verb(context: &IContextMenu, offset: usize) -> Option<String> {
    let mut buffer = [0u8; 260];
    // `GCS_VERBW` writes wide characters into the same buffer, so it is sized for
    // them and read back as such.
    context
        .GetCommandString(
            offset,
            GCS_VERBW,
            None,
            PSTR(buffer.as_mut_ptr()),
            (buffer.len() / 2) as u32,
        )
        .ok()?;
    let wide: Vec<u16> = buffer
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .take_while(|unit| *unit != 0)
        .collect();
    let verb = String::from_utf16_lossy(&wide);
    (!verb.is_empty()).then_some(verb)
}

// ---------------------------------------------------------------------------
// Who registered a verb
// ---------------------------------------------------------------------------

/// Every CLSID asked about so far, and what it resolved to.
///
/// Memoised for the life of the process because the answer cannot change while it is running — a
/// handler that is reinstalled under the same CLSID is the same product — and because a menu asks
/// about the same dozen CLSIDs every time it is built. `None` is a real answer and is cached too:
/// a CLSID with no `InprocServer32` is most often a packaged handler, whose implementation lives in
/// an appx and is not a DLL path at all, and re-establishing that on every menu would be four
/// registry reads for the same nothing.
static MEMO: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Option<Handler>>>> =
    std::sync::LazyLock::new(Default::default);

/// The product that registered this verb, where the verb is a CLSID and the CLSID is a DLL.
///
/// # Why a verb is enough to find one
///
/// `GetCommandString` answers with a *canonical name*, and for anything registered as an
/// `IExplorerCommand` — which is how everything written for Windows 11 registers — that name is the
/// handler's CLSID in braces: `{9F156763-7844-4DC4-B2B1-901F640F5155}` is Open in Terminal. So the
/// verb this program already reads off every entry is, for a third of a modern menu, a direct index
/// into `HKCR\CLSID`. Nothing else has to be asked of the shell.
///
/// A **static** registry verb — `Open with Zed`, from `HKCR\*\shell\Zed` — has no CLSID and gets
/// `None`. That is not a failure: `crate::shell::menu::name_of` reads a run with no owner as a run
/// nobody owns, which is exactly what a pile of unrelated `open with` verbs is.
pub(super) fn handler_of(verb: &str) -> Option<Handler> {
    // A CLSID and nothing else. `{` is the cheap test that keeps every ordinary verb — `open`,
    // `cut`, `properties` — out of the registry entirely.
    if !verb.starts_with('{') || !verb.ends_with('}') {
        return None;
    }
    if let Ok(memo) = MEMO.lock() {
        if let Some(found) = memo.get(verb) {
            return found.clone();
        }
    }
    let handler = resolve_handler(verb);
    if let Ok(mut memo) = MEMO.lock() {
        memo.insert(verb.to_owned(), handler.clone());
    }
    handler
}

/// The uncached half of [`handler_of`], which is **two lookups because there are two kinds of
/// handler** and the modern kind is not where thirty years of documentation says to look.
///
/// 1. `CLSID\{..}\InprocServer32` — classic COM. OneDrive's legacy menu, 7-Zip, TortoiseGit.
///    Gives a DLL path and, through it, a version-info name: `Microsoft OneDrive`, `7-Zip`.
/// 2. `PackagedCom\ClassIndex\{..}` — an MSIX-packaged handler, which has **no `CLSID` key at
///    all**. Its one subkey is the package full name.
///
/// The second is not an edge case: measured by `probe_banding` on this machine, *every* CLSID verb in
/// a file's menu — Copilot, Move to OneDrive, File Locksmith, Edit in Notepad, Open with Zed,
/// PowerRename — resolved to nothing until this was here, because PowerToys, OneDrive's modern
/// commands and everything from the Store now ship as sparse packages. Only the first lookup existed
/// to begin with, and the effect was a feature that appeared to work and named nothing.
fn resolve_handler(clsid: &str) -> Option<Handler> {
    if let Some(dll) = hkcr(&format!("CLSID\\{clsid}\\InprocServer32")) {
        let module = PathBuf::from(expand(&dll));
        // A bare file name means a DLL found on the loader's path — `shell32.dll` and friends —
        // which is still a usable identity even though no version info will read off it.
        let name = file_description(&module);
        return Some(Handler { module, name });
    }
    packaged_handler(clsid)
}

/// An MSIX-packaged handler, named after the package that registered it.
///
/// `PackagedCom\ClassIndex\{CLSID}` has exactly one subkey and it is the package full name —
/// `Microsoft.PowerToys.PowerRenameContextMenu_0.99.1.0_neutral__8wekyb3d8bbwe`. That is the
/// identity, version and all: two verbs from the same package are the same product, and a package
/// that updates is a different string for a different build, which costs a re-read and nothing else.
///
/// The name is the package name with the version and architecture cut off and the `ContextMenu`
/// suffix every one of them carries removed — `Microsoft.PowerToys.PowerRename`, then its last
/// component, `PowerRename`. Not the package's real `DisplayName`, which is an `ms-resource:`
/// indirection into an appx resource map: reachable, through `SHLoadIndirectString` and a manifest
/// read, and not worth a file parse and a string-loader for a submenu's caption.
///
/// The DLL is *not* used as the identity here even though the key beside this one has it: it is a
/// bare file name inside the package (`PowerToys.PowerRenameContextMenu.dll`) with no directory, so
/// two packages shipping the same file name would collide where the package name cannot.
fn packaged_handler(clsid: &str) -> Option<Handler> {
    let package = first_subkey(&format!("PackagedCom\\ClassIndex\\{clsid}"))?;
    // `Name_version_arch__publisher`. Everything from the first `_` is the identity of the *build*
    // rather than of the product, and it is the product a menu row should be named after.
    let stem = package.split('_').next().unwrap_or(&package);
    // The same tidy [`without_boilerplate`] does to a version-info name, and deliberately not the
    // same function: that one strips ` Shell Extension` from human text, this one strips
    // `ShellExtension` from a dotted identifier. One function covering both conventions would have to
    // guess which it was looking at.
    let stem = stem
        .strip_suffix("ContextMenu")
        .or_else(|| stem.strip_suffix("ShellExtension"))
        .unwrap_or(stem)
        .trim_end_matches('.');
    // The last dot-component: `Microsoft.PowerToys.PowerRename` reads as `PowerRename`, which is
    // what the entry it names is called. Falls back to the whole thing for a package with no dots.
    let name = stem.rsplit('.').next().unwrap_or(stem);
    Some(Handler {
        module: PathBuf::from(&package),
        name: (!name.is_empty()).then(|| name.to_owned()),
    })
}

/// The name of the first subkey under `HKEY_CLASSES_ROOT\<path>`.
///
/// For `PackagedCom\ClassIndex\{CLSID}`, whose *only* content is one subkey named after the package
/// — the value is in the key's name, which is why this reads a name and not a value.
fn first_subkey(path: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, HKEY_CLASSES_ROOT, KEY_READ,
    };

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated and outlives the call; `key` is closed on every path out.
    if unsafe { RegOpenKeyExW(HKEY_CLASSES_ROOT, wide.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return None;
    }
    // A package full name. 256 is the documented maximum for a key name.
    let mut buffer = [0u16; 256];
    let mut units = buffer.len() as u32;
    // SAFETY: `buffer` and `units` describe the same allocation, and `units` is updated by the call
    // to the length written, not counting the terminator.
    let read = unsafe {
        RegEnumKeyExW(
            key,
            0,
            buffer.as_mut_ptr(),
            &mut units,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    // SAFETY: opened above, and not used again.
    unsafe { RegCloseKey(key) };
    if read != 0 {
        return None;
    }
    let name = String::from_utf16_lossy(&buffer[..(units as usize).min(buffer.len())]);
    (!name.is_empty()).then_some(name)
}

/// A string value's default under `HKEY_CLASSES_ROOT`, `REG_EXPAND_SZ` included.
///
/// `crate::shell::providers` has a near-twin of this that filters to `REG_SZ` — right for the
/// ProgIDs and perceived types it reads, wrong here: an `InprocServer32` is a path, and a path in
/// the registry is as likely to be written `%SystemRoot%\system32\…` as spelled out.
fn hkcr(path: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CLASSES_ROOT, KEY_READ,
        REG_EXPAND_SZ, REG_SZ,
    };

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut key: HKEY = std::ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated and outlives the call; `key` is only used once the call has
    // reported success, and is closed on every path out.
    if unsafe { RegOpenKeyExW(HKEY_CLASSES_ROOT, wide.as_ptr(), 0, KEY_READ, &mut key) } != 0 {
        return None;
    }
    // A path, so `MAX_PATH` doubled for the `\\?\` case. Anything longer is not one.
    let mut buffer = [0u16; 560];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    let mut kind = 0u32;
    // SAFETY: `buffer` and `bytes` describe the same allocation and `bytes` is updated to what was
    // written; the value name is null, which asks for the key's own default.
    let read = unsafe {
        RegQueryValueExW(
            key,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut kind,
            buffer.as_mut_ptr() as *mut u8,
            &mut bytes,
        )
    };
    // SAFETY: opened above, and not used again.
    unsafe { RegCloseKey(key) };
    if read != 0 || (kind != REG_SZ && kind != REG_EXPAND_SZ) {
        return None;
    }
    let units = (bytes as usize / 2).min(buffer.len());
    let text: String = String::from_utf16_lossy(&buffer[..units]);
    let text = text.trim_end_matches('\0').trim().to_owned();
    (!text.is_empty()).then_some(text)
}

/// `%SystemRoot%\system32\shell32.dll` as a path this process can open.
///
/// By hand rather than through `ExpandEnvironmentStringsW`, which would mean enabling
/// `Win32_System_Environment` for one call that `std::env` can already answer. Unknown names are
/// left as they were written, which keeps a malformed value recognisable instead of blanking it.
fn expand(text: &str) -> String {
    if !text.contains('%') {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('%') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('%') {
            Some(close) => {
                let name = &after[..close];
                match std::env::var(name) {
                    Ok(value) => out.push_str(&value),
                    // Not a variable this process has. `%` and the name go back verbatim.
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[close + 1..];
            }
            // An unpaired `%`, which is not an expansion at all.
            None => {
                out.push('%');
                out.push_str(after);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A DLL's `FileDescription` — `Microsoft OneDrive`, `PowerRename Shell Extension`.
///
/// The friendliest name a shell extension reliably carries. Its `ProductName` is often the suite
/// rather than the thing (`Microsoft Windows Operating System`), and its file name is not for
/// showing anybody, so this is the one worth the two calls.
///
/// The language block is asked for rather than assumed: `VarFileInfo\Translation` holds the
/// translations the file actually has, and a hardcoded `040904B0` misses every DLL that ships one
/// language and it is not US English — which on a French machine is a good number of them.
fn file_description(module: &Path) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW,
    };

    let wide = crate::shell::wide(module);
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let size = unsafe { GetFileVersionInfoSizeW(wide.as_ptr(), std::ptr::null_mut()) };
    if size == 0 {
        return None;
    }
    let mut block = vec![0u8; size as usize];
    // SAFETY: `block` is `size` bytes, which is what the call above asked for.
    if unsafe { GetFileVersionInfoW(wide.as_ptr(), 0, size, block.as_mut_ptr() as *mut _) } == 0 {
        return None;
    }

    // Whatever `\StringFileInfo\<lang><codepage>\FileDescription` the file has. Every translation
    // it declares, then the two spellings that are common enough to be worth guessing at when it
    // declares none.
    let mut blocks: Vec<String> = translations(&block)
        .into_iter()
        .map(|(lang, page)| format!("\\StringFileInfo\\{lang:04x}{page:04x}\\FileDescription"))
        .collect();
    blocks.push("\\StringFileInfo\\040904b0\\FileDescription".to_owned());
    blocks.push("\\StringFileInfo\\000004b0\\FileDescription".to_owned());

    for path in blocks {
        let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut value: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut units = 0u32;
        // SAFETY: `block` outlives the call and every pointer out of it; `value` points into
        // `block` and `units` is the count of `u16` there, both written by the call.
        let ok = unsafe {
            VerQueryValueW(
                block.as_ptr() as *const _,
                wide.as_ptr(),
                &mut value,
                &mut units,
            )
        };
        if ok == 0 || value.is_null() || units == 0 {
            continue;
        }
        // SAFETY: the call reports `units` UTF-16 units at `value`, inside `block`.
        let text = unsafe { std::slice::from_raw_parts(value as *const u16, units as usize) };
        let text = String::from_utf16_lossy(text);
        let text = text.trim_end_matches('\0').trim();
        if !text.is_empty() {
            return Some(without_boilerplate(text));
        }
    }
    None
}

/// `Microsoft OneDrive Shell Extension` → `Microsoft OneDrive`, `7-Zip Shell Extension` → `7-Zip`.
///
/// Every one of these files describes itself as a shell extension, so saying so on the menu row
/// names the mechanism rather than the product. [`packaged_handler`] does the same job for a package
/// name, under the other naming convention — and the row is narrow. Measured spellings, from the
/// files actually on this machine; anything unrecognised is left exactly as its author wrote it,
/// which is the right way round for a string being shown to somebody.
fn without_boilerplate(name: &str) -> String {
    const NOISE: [&str; 5] = [
        " Shell Extension",
        " Context Menu Handler",
        " Context Menu",
        " Shell Extensions",
        " Explorer Extension",
    ];
    for suffix in NOISE {
        if name.len() > suffix.len() && name.to_lowercase().ends_with(&suffix.to_lowercase()) {
            return name[..name.len() - suffix.len()].trim_end().to_owned();
        }
    }
    name.to_owned()
}

/// The `(language, code page)` pairs a version block declares, from `\VarFileInfo\Translation`.
fn translations(block: &[u8]) -> Vec<(u16, u16)> {
    use windows_sys::Win32::Storage::FileSystem::VerQueryValueW;

    let wide: Vec<u16> = "\\VarFileInfo\\Translation"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut value: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut bytes = 0u32;
    // SAFETY: as `file_description` above — `block` outlives the call, and both out-params are
    // written by it.
    let ok = unsafe {
        VerQueryValueW(
            block.as_ptr() as *const _,
            wide.as_ptr(),
            &mut value,
            &mut bytes,
        )
    };
    if ok == 0 || value.is_null() {
        return Vec::new();
    }
    // Pairs of `u16`: the language id then the code page.
    let count = bytes as usize / 4;
    // SAFETY: the call reports `bytes` bytes at `value`, inside `block`.
    let pairs = unsafe { std::slice::from_raw_parts(value as *const u16, count * 2) };
    pairs.chunks_exact(2).map(|pair| (pair[0], pair[1])).collect()
}

/// A menu item's bitmap as RGBA, when it is a real one.
///
/// `hbmpItem` doubles as a slot for the `HBMMENU_*` family — small integers standing in for a
/// bitmap, which have to be filtered out before the value is treated as a handle.
///
/// # The filter that ate half the icons
///
/// This was `(bitmap.0 as isize) <= 16`, on the reasoning that the magic values run from `-1` to `16`
/// and a real handle is a pointer, so anything at or below the top of that range is not one.
///
/// **A GDI handle is not a pointer.** It is a 32-bit value that Windows sign-extends into the
/// pointer-sized field, so any handle whose 32-bit form has its top bit set arrives as a large
/// *negative* `isize` — and `<= 16` threw every one of them away. Measured by [`probe_menu_icons`] on
/// a folder's background menu: `Open with Code` came in on `-1023068341`, `Open with Visual Studio`
/// on `-821751332`, `Open Git Bash here` on `-335203089`, all three of them perfectly good 16×16
/// 32bpp bitmaps, all three discarded. `Open Git GUI here` next to them arrived as a positive handle
/// and drew fine, which is what made the failure look intermittent rather than systematic: whether a
/// row had an icon came down to whether its handle's high bit happened to be set. Nine of the eleven
/// rows without an icon on that menu were this.
///
/// So the filter is the *set* the documentation actually defines — `HBMMENU_CALLBACK` is `-1` and the
/// rest are `1` to `11`, with room left over — and nothing else. It cannot reject a real handle,
/// because GDI does not hand out handles in single digits, and a value that gets past it and is not a
/// bitmap is caught anyway: `GetObjectW` in `crate::shell::icons::read_bgra` returns zero for a
/// handle that is not one, and the size bound there catches the rest.
///
/// `HBMMENU_CALLBACK` is the one that is a real answer rather than a mistake: it means the owner
/// intended to draw the icon itself during `WM_DRAWITEM`, which a menu that is never shown never
/// receives. Nothing to read, and nothing to be done about it here.
unsafe fn menu_bitmap(
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
) -> Option<egui::ColorImage> {
    if bitmap.is_invalid() || stands_in_for_a_bitmap(bitmap.0 as isize) {
        return None;
    }
    crate::shell::icons::bitmap_of(bitmap)
}

/// Whether an `hbmpItem` is one of the `HBMMENU_*` stand-ins rather than a handle.
///
/// Its own function so the set can be tested without a menu — see
/// `a_gdi_handle_with_its_high_bit_set_is_not_a_magic_value`, which is the fence around the bug
/// [`menu_bitmap`] describes.
pub(super) fn stands_in_for_a_bitmap(raw: isize) -> bool {
    /// Above `HBMMENU_POPUP_MINIMIZE`, which is 11, with slack for a value Windows has not
    /// documented yet. **Not** a floor on what counts as a handle; see [`menu_bitmap`].
    const LAST_MAGIC: isize = 16;
    // Null, `HBMMENU_CALLBACK`, and the positive family. Nothing else — a large negative value is a
    // sign-extended GDI handle and is exactly what this used to discard.
    raw == 0 || raw == -1 || (1..=LAST_MAGIC).contains(&raw)
}

/// What each `CMF_` flag combination costs, and what it leaves out.
///
/// The expensive work happens *inside* `QueryContextMenu`, before any entry exists, so
/// there is nothing to filter afterwards. The only lever is asking for less. This is what
/// asking for less actually buys.
#[cfg(test)]
pub(super) fn probe_flags(parent: &Path, items: &[PathBuf]) {
    use std::time::Instant;
    use windows::Win32::UI::Shell::{
        CMF_DEFAULTONLY, CMF_DONOTPICKDEFAULT, CMF_NOVERBS, CMF_OPTIMIZEFORINVOKE,
    };

    for (label, flags) in [
        ("NORMAL|EXPLORE (what it uses)", CMF_NORMAL | CMF_EXPLORE),
        ("OPTIMIZEFORINVOKE", CMF_OPTIMIZEFORINVOKE),
        (
            "NORMAL|EXPLORE|DONOTPICKDEFAULT",
            CMF_NORMAL | CMF_EXPLORE | CMF_DONOTPICKDEFAULT,
        ),
        ("DEFAULTONLY", CMF_DEFAULTONLY),
        ("NOVERBS", CMF_NOVERBS),
    ] {
        // SAFETY: the menu is destroyed before the next round, and the interfaces are
        // reference counted.
        unsafe {
            let Some(context) = context_of(parent, items) else {
                return;
            };
            let Ok(hmenu) = CreatePopupMenu() else { return };
            let at = Instant::now();
            let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags);
            let took = at.elapsed().as_secs_f32() * 1e3;
            let mut live = Live {
                context,
                hmenu,
                submenus: std::collections::HashMap::new(),
                next: 0,
            };
            let entries = live.read(hmenu, 0, &[]);
            eprintln!(
                "  {label:<32} {took:>9.1} ms  {} entries",
                entries.len()
            );
            eprintln!(
                "      {}",
                entries
                    .iter()
                    .filter(|e| !e.label.is_empty())
                    .map(|e| e.label.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
        }
    }
}

/// Whether a **submenu row** carries a canonical verb, which decides whether `Ouvrir avec` can be
/// anchored by name or only by position.
///
/// [`Live::read`] never asks: a submenu takes the `Kind::unfilled` branch before the id branch is
/// reached, and [`commands_of`] skips popups outright on the grounds that "a popup's `wID` means
/// nothing". This is the measurement behind that grounds — it prints `wID` and whatever
/// `GetCommandString` answers for every top-level row, popups included, so the claim is a reading
/// rather than an assumption. See `crate::shell::menu::regroup`, which has to place `Open with`.
#[cfg(test)]
pub(super) fn probe_submenu_verbs(parent: &Path, items: &[PathBuf]) {
    unsafe {
        let Some(context) = context_of(parent, items) else {
            return;
        };
        let Ok(hmenu) = CreatePopupMenu() else { return };
        let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags(Depth::Full));
        let count = GetMenuItemCount(Some(hmenu)).max(0);
        for position in 0..count {
            let mask = MIIM_ID | MIIM_SUBMENU | MIIM_FTYPE;
            let Some((info, raw)) = item_at(hmenu, position as u32, mask) else {
                continue;
            };
            if info.fType.0 & MFT_SEPARATOR.0 != 0 {
                continue;
            }
            let label = split_label(&raw).0;
            let popup = !info.hSubMenu.is_invalid();
            // Asked whatever the id looks like, which is the point: an out-of-range id with a real
            // verb behind it would mean the guard in `read` is what is losing the name.
            let verb = info
                .wID
                .checked_sub(FIRST)
                .and_then(|offset| canonical_verb(&context, offset as usize));
            eprintln!(
                "  {label:<44} wID {:<6} {} verb {verb:?}",
                info.wID,
                if popup { "POPUP" } else { "     " }
            );
        }
        let _ = DestroyMenu(hmenu);
    }
}

/// Why a row has no icon: what the shell put in `hbmpItem`, and what came of reading it.
///
/// Three different answers hide behind one blank gutter and they need different fixes, so this
/// prints them apart: **no handle at all** (the entry supplies its icon by some other route, and
/// `hbmpItem` was never going to have it), **a `HBMMENU_*` magic value** (a small integer, not a
/// bitmap — `HBMMENU_CALLBACK` means the owner expected to draw it during `WM_DRAWITEM`, which a menu
/// that is never shown never gets), or **a real handle that would not convert**, which is the only
/// one of the three that is this program's bug.
///
/// `MFT_OWNERDRAW` is printed too: an entry that draws itself has no label either, and one that
/// arrives here with an empty label is being dropped by [`Live::read`] rather than mis-drawn.
#[cfg(test)]
pub(super) fn probe_menu_icons(parent: &Path, items: &[PathBuf]) {
    use windows::Win32::Graphics::Gdi::{GetObjectW, BITMAP};
    use windows::Win32::UI::WindowsAndMessaging::MFT_OWNERDRAW;

    unsafe {
        let Some(context) = context_of(parent, items) else {
            return;
        };
        let Ok(hmenu) = CreatePopupMenu() else { return };
        let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags(Depth::Full));
        let count = GetMenuItemCount(Some(hmenu)).max(0);
        let (mut with, mut without) = (0, 0);
        // What converting them all costs, which is the question behind "should this be async": it
        // happens on the builder thread, so it is already off the frame, but a cost big enough to
        // delay the *menu* would want the icons filled in afterwards instead.
        let mut converting = std::time::Duration::ZERO;
        for position in 0..count {
            let mask = MIIM_ID | MIIM_SUBMENU | MIIM_FTYPE | MIIM_BITMAP;
            let Some((info, text)) = item_at(hmenu, position as u32, mask) else {
                continue;
            };
            if info.fType.0 & MFT_SEPARATOR.0 != 0 {
                continue;
            }
            let label = split_label(&text).0;
            let raw = info.hbmpItem.0 as isize;
            let verb = info
                .wID
                .checked_sub(FIRST)
                .and_then(|offset| canonical_verb(&context, offset as usize));

            // What the handle is, and — when it is a real one — what shape of bitmap.
            let what = if info.hbmpItem.is_invalid() || raw == 0 {
                "no handle".to_owned()
            } else if raw == -1 || (1..=16).contains(&raw) {
                // The `HBMMENU_*` family, and nothing wider — see [`menu_bitmap`], where a bound of
                // `<= 16` was throwing away every handle with its high bit set. `-1` is
                // `HBMMENU_CALLBACK`, which means "ask me to draw it" and is unanswerable for a menu
                // that never pops up.
                format!("magic {raw}")
            } else {
                let mut header = BITMAP::default();
                let read = GetObjectW(
                    info.hbmpItem.into(),
                    std::mem::size_of::<BITMAP>() as i32,
                    Some((&mut header) as *mut BITMAP as *mut std::ffi::c_void),
                );
                let at = std::time::Instant::now();
                let converted = menu_bitmap(info.hbmpItem).is_some();
                converting += at.elapsed();
                if read == 0 {
                    "handle, but GetObject refused it".to_owned()
                } else {
                    format!(
                        "{}x{} {}bpp -> {}",
                        header.bmWidth,
                        header.bmHeight,
                        header.bmBitsPixel,
                        if converted { "converted" } else { "**FAILED**" }
                    )
                }
            };
            if what.starts_with("no handle") || what.starts_with("magic") {
                without += 1;
            } else {
                with += 1;
            }
            eprintln!(
                "  {label:<44} {}{:<34} {}",
                if info.fType.0 & MFT_OWNERDRAW.0 != 0 { "OWNERDRAW " } else { "" },
                what,
                verb.unwrap_or_else(|| "-".to_owned())
            );
        }
        eprintln!(
            "  {with} rows offered a bitmap, {without} offered none — converting them all took              {:.2} ms",
            converting.as_secs_f32() * 1e3
        );
        let _ = DestroyMenu(hmenu);
    }
}

/// Time every shell call one menu costs, call by call.
///
/// Not a test of anything — a measurement, and the only way to find out which of these is
/// the slow one when the file is on a share. See `probe_menu_costs`.
#[cfg(test)]
pub(super) fn probe(parent: &Path, items: &[PathBuf]) {
    use std::time::Instant;
    let ms = |at: Instant| at.elapsed().as_secs_f32() * 1e3;

    unsafe {
        let at = Instant::now();
        let pidl = pidl_of(items.first().unwrap_or(&parent.to_owned()));
        eprintln!("  SHParseDisplayName      {:>9.1} ms", ms(at));
        drop(pidl);

        let at = Instant::now();
        let Some(context) = context_of(parent, items) else {
            eprintln!("  no IContextMenu");
            return;
        };
        eprintln!("  bind + GetUIObjectOf    {:>9.1} ms", ms(at));

        let at = Instant::now();
        let Ok(hmenu) = CreatePopupMenu() else { return };
        let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, CMF_NORMAL | CMF_EXPLORE);
        eprintln!("  QueryContextMenu        {:>9.1} ms", ms(at));

        // The read, split into the three things it does per item.
        let count = GetMenuItemCount(Some(hmenu));
        let mut reading = 0.0f32;
        let mut bitmaps = 0.0f32;
        let mut verbs: Vec<(String, f32, Option<String>)> = Vec::new();
        for position in 0..count {
            let at = Instant::now();
            let mut info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE
                    | MIIM_STATE
                    | MIIM_ID
                    | MIIM_SUBMENU
                    | MIIM_STRING
                    | MIIM_BITMAP,
                ..Default::default()
            };
            if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
                continue;
            }
            let mut text = vec![0u16; info.cch as usize + 1];
            info.dwTypeData = PWSTR(text.as_mut_ptr());
            info.cch = text.len() as u32;
            if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
                continue;
            }
            let raw = String::from_utf16_lossy(&text[..info.cch as usize]);
            reading += ms(at);

            let at = Instant::now();
            let _ = menu_bitmap(info.hbmpItem);
            bitmaps += ms(at);

            if info.hSubMenu.is_invalid() && info.wID >= FIRST && info.wID <= LAST {
                let at = Instant::now();
                let verb = canonical_verb(&context, (info.wID - FIRST) as usize);
                verbs.push((split_label(&raw).0, ms(at), verb));
            }
        }
        eprintln!("  GetMenuItemInfoW x{count:<3}   {reading:>9.1} ms");
        eprintln!("  item bitmaps            {bitmaps:>9.1} ms");
        let total: f32 = verbs.iter().map(|(_, ms, _)| ms).sum();
        eprintln!("  GetCommandString x{:<3}   {total:>9.1} ms", verbs.len());
        verbs.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (label, took, verb) in verbs.iter().take(8) {
            eprintln!("      {took:>9.1} ms  {label:<32} -> {verb:?}");
        }
        let _ = DestroyMenu(hmenu);
    }
}

/// Invoke `properties` against one of a folder's two menus, and say whether the shell claims to
/// have run it.
///
/// The instrument behind the table on [`as_an_item`], and the only way left to re-measure it:
/// [`invoke`] deliberately never asks a background menu for its Properties any more, so nothing
/// in the program reaches the case this reproduces. `true` means `InvokeCommand` returned
/// success, which on the background menu it does *without showing anything* — the whole point.
///
/// The menu is built and thrown away exactly as [`invoke`] builds it, because a verb has to be
/// invoked against a menu the shell has actually populated.
#[cfg(test)]
pub(super) fn probe_properties(parent: &Path, items: &[PathBuf]) -> bool {
    // SAFETY: the menu is destroyed before returning and the interfaces are reference counted.
    unsafe {
        let Some(context) = context_of(parent, items) else {
            return false;
        };
        let Ok(hmenu) = CreatePopupMenu() else {
            return false;
        };
        let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags(Depth::Full));
        let at = std::time::Instant::now();
        let ran = run(
            &context,
            parent,
            Named::Verb("properties"),
            crate::shell::Owner::default(),
        );
        eprintln!(
            "  InvokeCommand(properties) against {} -> {ran} in {:.1} ms",
            if items.is_empty() { "the background" } else { "the item" },
            at.elapsed().as_secs_f32() * 1e3
        );
        let _ = DestroyMenu(hmenu);
        ran
    }
}

/// Which entries survive into the menu `invoke` resolves against.
///
/// The instrument behind the table on [`super::invoke`]: it builds the menu twice, once as
/// shown and once with `CMF_OPTIMIZEFORINVOKE`, and names the commands the flag loses. Kept
/// because that flag is a standing temptation — it is documented to save exactly the work an
/// unshown menu does not need — and this is what says which half of the menu it breaks.
#[cfg(test)]
pub(super) fn probe_invokable(parent: &Path, items: &[PathBuf]) {
    use windows::Win32::UI::Shell::CMF_OPTIMIZEFORINVOKE;

    unsafe fn collect(
        live: &mut Live,
        entries: Vec<Entry>,
        prefix: &str,
        out: &mut Vec<(String, Option<String>, u32)>,
    ) {
        for entry in entries {
            if let Some(source) = entry.kind.unasked() {
                let children = live.fill(source);
                let deeper = format!("{prefix}{} > ", entry.label);
                collect(live, children, &deeper, out);
                continue;
            }
            if let Kind::Command(Command::Shell { verb, id, .. }) = entry.kind {
                out.push((format!("{prefix}{}", entry.label), verb, id));
            }
        }
    }

    let gather = |flags: u32| -> Vec<(String, Option<String>, u32)> {
        unsafe {
            let Some(context) = context_of(parent, items) else {
                return Vec::new();
            };
            let Ok(hmenu) = CreatePopupMenu() else {
                return Vec::new();
            };
            let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags);
            let mut live = Live {
                context,
                hmenu,
                submenus: std::collections::HashMap::new(),
                next: 0,
            };
            let entries = live.read(hmenu, 0, &[]);
            let mut out = Vec::new();
            collect(&mut live, entries, "", &mut out);
            out
        }
    };

    let shown = gather(CMF_NORMAL | CMF_EXPLORE);
    let optimised = gather(CMF_OPTIMIZEFORINVOKE);
    let have: std::collections::HashSet<String> = optimised
        .iter()
        .filter_map(|(_, verb, _)| verb.clone())
        .collect();
    eprintln!(
        "  as shown: {} commands;  rebuilt with OPTIMIZEFORINVOKE: {} commands",
        shown.len(),
        optimised.len()
    );
    for (label, verb, id) in &shown {
        match verb {
            Some(verb) if have.contains(verb) => eprintln!("    ok       {label:<46} {verb}"),
            Some(verb) => eprintln!("    MISSING  {label:<46} {verb}  (id {id})"),
            None => eprintln!("    no verb  {label:<46} (id {id})"),
        }
    }
}

/// `CMIC_MASK_UNICODE`, which the `windows` crate does not name. It is `SEE_MASK_UNICODE` —
/// the two families of flags share their numbering — and it is what makes the shell read
/// `lpVerbW`, `lpParametersW` and **`lpDirectoryW`** instead of the ANSI members. Without it
/// the wide half of `CMINVOKECOMMANDINFOEX` is filled in and ignored.
const CMIC_MASK_UNICODE: u32 = windows::Win32::UI::Shell::SEE_MASK_UNICODE;
/// `CMIC_MASK_FLAG_LOG_USAGE`, likewise unnamed: `SEE_MASK_FLAG_LOG_USAGE`. What Explorer
/// sets so that a verb the user chose counts towards the recent and frequent lists — an
/// "Open with Code" from here should teach the same things it teaches from Explorer.
const CMIC_MASK_FLAG_LOG_USAGE: u32 = windows::Win32::UI::Shell::SEE_MASK_FLAG_LOG_USAGE;

/// Run a shell command against a menu built afresh, by verb where there is one.
///
/// # Why the menu is built with the flags it was *shown* with
///
/// This used to ask for `CMF_OPTIMIZEFORINVOKE` whenever there was a verb — the flag whose
/// documented job is to skip the work only a displayed menu needs, and which was measured at
/// half a second saved on a file. It is also **the reason half the menu did nothing**.
///
/// Measured by `probe_invokable` on this machine, comparing the menu as shown against the
/// same menu rebuilt with that flag:
///
/// | menu | commands as shown | rebuilt | verbs that vanished |
/// | --- | --- | --- | --- |
/// | a folder | 57 | 40 | Open in Terminal, PowerRename, Open with Zed, Move to OneDrive, Unlock with File Locksmith, Pin to Start |
/// | a text file | 53 | 35 | those, plus Ask Copilot and Edit in Notepad |
/// | a folder's background | 18 | 17 | — |
///
/// Everything in that last column is an `IExplorerCommand` — which is how anything written
/// for Windows 11 registers — and their canonical "verb" is a CLSID in braces,
/// `{9F156763-7844-4DC4-B2B1-901F640F5155}` for Open in Terminal. `CMF_OPTIMIZEFORINVOKE`
/// skips that whole wrapper, so the rebuilt menu has no such verb, `InvokeCommand` matches
/// nothing, and the entry silently does nothing at all. Which is exactly what "Open in
/// Terminal does not work" was.
///
/// So: the same flags, from the same [`Depth`], as the menu the user actually clicked. It
/// costs what the first build cost — and rather less in practice, since the shell has just
/// been asked the same question and its caches are warm. It happens on
/// [`crate::shell::Modal`], where nothing is waiting for it.
///
/// # And why an id needs the submenu opened first
///
/// An entry with no canonical verb — every `Send to`, every `Open with`, `Include in
/// library` — can only be named by its numeric id, and that id is handed out by the
/// extension when it *populates* the submenu. A menu built afresh has populated none of
/// them, so the id names nothing. Hence [`Command::Shell::path`] and the walk below, which
/// sends each level on the way down the `WM_INITMENUPOPUP` that Windows would have sent.
///
/// The walk is not done for a verb, only as the fallback: it costs an extension's populate
/// per level — 3 ms for New, up to 116 ms for Open With — and a verb has no use for it.
///
/// # And why one command is invoked against a different menu entirely
///
/// `properties` on a folder's **background** menu answers `S_OK` and does nothing at all, so it
/// is run against the folder as an item instead. That is [`as_an_item`], and the measurements
/// are there.
pub fn invoke(
    parent: &Path,
    items: &[PathBuf],
    command: &super::Command,
    depth: Depth,
    owner: crate::shell::Owner,
) {
    let super::Command::Shell { verb, id, path, label } = command else {
        return;
    };
    // The one entry a background menu cannot run itself; see `as_an_item`. Everything below is
    // then an ordinary invoke against an ordinary selection — including the id fallback, which
    // `resolve` recovers by label if it is ever reached.
    let stand_in = as_an_item(parent, items, verb.as_deref());
    let items = stand_in.as_deref().unwrap_or(items);
    // SAFETY: the menu is destroyed before returning, and every string outlives the
    // call that reads it.
    unsafe {
        let Some(context) = context_of(parent, items) else {
            return;
        };
        let Ok(hmenu) = CreatePopupMenu() else {
            return;
        };
        let _ = context.QueryContextMenu(hmenu, 0, FIRST, LAST, flags(depth));

        // A verb first: it is a name rather than a position, so nothing about the shape of
        // this menu can make it mean the wrong thing.
        let mut ran = false;
        if let Some(verb) = verb {
            ran = run(&context, parent, Named::Verb(verb), owner);
        }
        // And the id, when there was no verb or the verb was refused. `InvokeCommand`
        // answering with a failure is the only signal there is that a verb did not resolve.
        if !ran {
            let mut level = hmenu;
            let mut reached = true;
            for position in path {
                match submenu_at(level, *position) {
                    Some(sub) => {
                        init_popup(&context, sub, *position);
                        level = sub;
                    }
                    None => {
                        reached = false;
                        break;
                    }
                }
            }
            // The id has to still name the entry the user clicked. It is a position in
            // somebody else's numbering, and a menu that came back a different shape would
            // otherwise run whatever now sits at that number — which in a context menu is
            // one slot away from Delete. See `resolve`, which is also what recovers the
            // right number when the shape did change.
            match reached.then(|| resolve(level, *id, label)).flatten() {
                Some(id) => {
                    run(&context, parent, Named::Id(id), owner);
                }
                None => {
                    // Only a debug build has a console to say it on, and this is the kind of
                    // thing that is unreadable in a log and priceless in a session.
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "shell menu: `{label}` (id {id}, verb {verb:?}, path {path:?}) is not \
                         in the rebuilt menu — {:?} — so nothing was invoked",
                        commands_of(level)
                    );
                }
            }
        }
        let _ = DestroyMenu(hmenu);
    }
}

/// The folder itself as a one-item selection, for the one command its **background** menu
/// will not run.
///
/// `properties` is in that menu — the shell puts it there, and this program draws it — and
/// invoking it does nothing whatsoever. Measured by
/// `properties_on_a_background_menu_goes_through_the_folder_as_an_item`, on a folder in the
/// sandbox, with the same verb against each of the two menus a folder has:
///
/// | the menu it is invoked against | `InvokeCommand("properties")` | what appeared |
/// | --- | --- | --- |
/// | `CreateViewObject` — the background | `S_OK`, in 0.3 ms | nothing, in four seconds of waiting |
/// | `GetUIObjectOf` — the folder as an item | `S_OK` | `Propriétés de : bg-props`, a second later |
///
/// **`S_OK` is the part that made it invisible.** The shell says it ran the command, so there is
/// no failure for [`invoke`] to fall back from: the id path is never reached, nothing is logged
/// even in a debug build, and the entry is simply inert. It is not the wrong verb, the wrong id
/// or a menu read incorrectly — every one of those was checked first. The sheet is not the return
/// value either; the shell puts it on a thread of its own and answers straight away, which is why
/// the difference between these two rows can only be seen by waiting for a window.
///
/// What the background menu's Properties actually is, is Explorer's *view's* command: the
/// handler shows the sheet through the site it expects to be hosted in, and a menu built here
/// out of a shell folder has no site. Providing one means implementing enough of
/// `IShellBrowser` and `IShellView` to satisfy somebody else's expectations, for one entry.
///
/// The swap is exact rather than approximate, which is what makes it the answer instead of a
/// workaround: Explorer's own background Properties shows the *folder's* sheet, and the folder's
/// sheet is what selecting that folder in its parent gives — the same one, from the same shell
/// folder, with the same property pages from the same extensions. See [`context_of`] for the two
/// menus a folder has.
///
/// Only `properties`, and only with nothing selected. Everything else in a background menu
/// belongs to whoever registered it — `Open Git Bash here`, `Open with Code`, New — and those
/// run perfectly well against the folder they were read from, which is where they belong.
pub(super) fn as_an_item(
    parent: &Path,
    items: &[PathBuf],
    verb: Option<&str>,
) -> Option<Vec<PathBuf>> {
    // By verb, never by label: `properties` is the shell's own name for it on every Windows,
    // and `Propriétés` is one localisation of many. Same rule as `super::properties_at`.
    let properties = verb.is_some_and(|verb| verb.eq_ignore_ascii_case("properties"));
    (items.is_empty() && properties).then(|| vec![parent.to_path_buf()])
}

/// How a command is being named to `InvokeCommand`.
enum Named<'a> {
    Verb(&'a str),
    /// An offset from the first id this program handed out.
    Id(u32),
}

/// One `InvokeCommand`. `true` when the shell says it ran.
///
/// `lpDirectory` is the folder, which is what `%V` and `%W` expand to in a registered
/// command line and what a launched process gets as its working directory. Explorer sets it;
/// this did not, and left every `Open <something> here` verb to guess.
unsafe fn run(
    context: &IContextMenu,
    parent: &Path,
    named: Named<'_>,
    owner: crate::shell::Owner,
) -> bool {
    let verb_bytes: Option<Vec<u8>> = match named {
        Named::Verb(verb) => {
            let mut bytes = verb.as_bytes().to_vec();
            bytes.push(0);
            Some(bytes)
        }
        Named::Id(_) => None,
    };
    let verb_wide: Option<Vec<u16>> = match named {
        Named::Verb(verb) => Some(verb.encode_utf16().chain(std::iter::once(0)).collect()),
        Named::Id(_) => None,
    };
    let id = match named {
        Named::Verb(_) => 0,
        Named::Id(id) => id,
    };

    // Wide, which is what `CMIC_MASK_UNICODE` selects — and the ANSI half as well, for an
    // extension that reads it regardless of the mask.
    //
    // Only when the path is ASCII, though: there is no cheap correct ANSI form of
    // `D:\Sources\Été`, and UTF-8 bytes are *not* one. A null there says "no directory",
    // which is what this passed for every path until now; the wrong directory would be new
    // and worse.
    let dir_wide = crate::shell::wide(parent);
    let dir_text = parent.to_string_lossy().replace('/', "\\");
    let dir_bytes: Option<Vec<u8>> = dir_text.is_ascii().then(|| {
        dir_text
            .clone()
            .into_bytes()
            .into_iter()
            .chain(std::iter::once(0))
            .collect()
    });

    let mut info = CMINVOKECOMMANDINFOEX {
        cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
        fMask: CMIC_MASK_UNICODE | CMIC_MASK_FLAG_LOG_USAGE,
        hwnd: if owner.0 != 0 {
            owner.hwnd()
        } else {
            HWND::default()
        },
        // A verb is a string; an id is a small integer pretending to be one,
        // which is how `IContextMenu` has taken numeric commands since it was
        // introduced. Both halves get it, since which one is read is the shell's choice.
        lpVerb: match &verb_bytes {
            Some(bytes) => PCSTR(bytes.as_ptr()),
            None => PCSTR(id as usize as *const u8),
        },
        lpVerbW: match &verb_wide {
            Some(wide) => PCWSTR(wide.as_ptr()),
            None => PCWSTR(id as usize as *const u16),
        },
        lpDirectory: match &dir_bytes {
            Some(bytes) => PCSTR(bytes.as_ptr()),
            None => PCSTR::null(),
        },
        lpDirectoryW: PCWSTR(dir_wide.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    let result = context.InvokeCommand(&mut info as *mut _ as *const _);
    #[cfg(debug_assertions)]
    if let Err(error) = &result {
        let named = match named {
            Named::Verb(verb) => verb.to_owned(),
            Named::Id(id) => format!("id {id}"),
        };
        eprintln!("shell menu: InvokeCommand({named}) refused: {error}");
    }
    result.is_ok()
}
