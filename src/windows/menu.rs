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
    MFS_CHECKED, MFS_DISABLED, MFS_GRAYED, MFT_SEPARATOR, MIIM_BITMAP, MIIM_FTYPE,
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
fn flags(depth: Depth) -> u32 {
    match depth {
        Depth::Full => CMF_NORMAL | CMF_EXPLORE,
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
            // Two calls: the first to learn the length, the second to get the text. A
            // fixed buffer would truncate a long "Open with <application>".
            if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
                continue;
            }
            let mut text = vec![0u16; info.cch as usize + 1];
            info.dwTypeData = PWSTR(text.as_mut_ptr());
            info.cch = text.len() as u32;
            if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
                continue;
            }

            if info.fType.0 & MFT_SEPARATOR.0 != 0 {
                // Two separators running together, or one at either end, are what a
                // menu assembled from several extensions looks like before anyone
                // tidies it.
                if !matches!(entries.last(), None | Some(Entry { kind: Kind::Separator, .. })) {
                    entries.push(Entry::separator());
                }
                continue;
            }

            let raw = String::from_utf16_lossy(&text[..info.cch as usize]);
            let (label, shortcut) = split_label(&raw);
            if label.is_empty() {
                continue;
            }

            let enabled = info.fState.0 & (MFS_DISABLED.0 | MFS_GRAYED.0) == 0;
            let checked = info.fState.0 & MFS_CHECKED.0 != 0;
            // `MFS_DEFAULT` — the entry a double click would have run — is deliberately not
            // read. It was, and it was drawn as the 2px accent bar a selected row gets, which
            // put a blue bar down the side of the top row of every context menu in the
            // program: the default entry is nearly always the first one, so what read as a
            // selection nobody had made was there every time the menu opened. There is
            // nothing else it would be used for, so it is not carried.
            let icon = menu_bitmap(info.hbmpItem);

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
                    verb: canonical_verb(&self.context, offset as usize),
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
        let mut info = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_ID | MIIM_STRING | MIIM_SUBMENU,
            ..Default::default()
        };
        if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
            continue;
        }
        // A popup's `wID` means nothing, and must not be allowed to match.
        if !info.hSubMenu.is_invalid() || info.wID < FIRST || info.wID > LAST {
            continue;
        }
        let id = info.wID - FIRST;
        let mut text = vec![0u16; info.cch as usize + 1];
        info.dwTypeData = PWSTR(text.as_mut_ptr());
        info.cch = text.len() as u32;
        if GetMenuItemInfoW(hmenu, position as u32, true, &mut info).is_err() {
            continue;
        }
        let raw = String::from_utf16_lossy(&text[..info.cch as usize]);
        out.push((id, split_label(&raw).0));
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

/// A menu item's bitmap as RGBA, when it is a real one.
///
/// `hbmpItem` doubles as a slot for a dozen `HBMMENU_*` magic values — small
/// integers, not handles — which is what the bound below is filtering out.
unsafe fn menu_bitmap(
    bitmap: windows::Win32::Graphics::Gdi::HBITMAP,
) -> Option<egui::ColorImage> {
    const LAST_MAGIC: isize = 16;
    if bitmap.is_invalid() || (bitmap.0 as isize) <= LAST_MAGIC {
        return None;
    }
    crate::shell::icons::bitmap_of(bitmap)
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
