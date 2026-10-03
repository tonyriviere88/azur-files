//! What a binary needs in order to run, followed down: the Dependency Walker question.
//!
//! A Windows executable names the DLLs it needs in its **import table**, by bare file name and
//! nothing else — `KERNEL32.dll`, not a path. Which file that turns out to be is the loader's
//! answer, not the binary's, and it is where the interesting failures live: the DLL that is
//! missing, the one that came off a different search directory than you thought, the 32-bit one
//! next to a 64-bit program. So this module does two separable things:
//!
//! 1. **Read the import table.** [`read`] opens a file, walks its headers and hands back the
//!    machine it was built for and the names it imports. Pure parsing, no policy.
//! 2. **Resolve those names and repeat.** [`walk`] follows the graph from one root binary,
//!    resolving each name against a search path, and comes back with every module reachable
//!    from it and where each one was found.
//!
//! # What is read, and what is not
//!
//! Only the headers and the import descriptors, never the whole file. A dependency walk of a
//! large application touches a couple of hundred binaries, and `Qt6Core.dll` alone is 6 MB —
//! reading them whole would be most of a gigabyte off the disk to find a few hundred strings.
//! Each module costs four small reads for its headers plus two per import, all of which land in
//! the page cache for the system DLLs that every walk visits.
//!
//! # What the search path is, and what it is not
//!
//! [`search_paths`], in this order: **the folder the root binary is in, then `System32`, then
//! `System`, then the Windows directory, and only then `PATH`.** Which is the loader's own order for
//! the parts of it that can be known from outside a running process.
//!
//! `PATH` being last is the part that matters. On an ordinary machine `System32` is *also* in
//! `PATH`, so a search that only knew about `PATH` still found `kernel32.dll` — but at whatever
//! position `System32` happened to occupy in that user's `PATH`, which is to say after anything
//! installed in front of it. The loader never does that, and a tool whose whole job is to say where
//! a DLL came from must not either.
//!
//! It is deliberately not the whole rule, and the differences are worth knowing before believing
//! a location this reports:
//!
//! - **`KnownDLLs` wins over everything.** `kernel32`, `ole32`, `user32` and about thirty others
//!   are resolved from a section the session manager opened at boot, so a copy of one sitting
//!   next to your program is ignored — where this would report the copy.
//! - **Side-by-side assemblies** (`WinSxS`) are resolved from a manifest, which is why two
//!   programs on one machine can load different `MSVCR90.dll`s.
//! - **API sets** are not files. See [`is_api_set`].
//! - **`SetDllDirectory`, `LOAD_WITH_ALTERED_SEARCH_PATH`, manifest `<file>` redirection** and a
//!   `.local` folder all move the goalposts at run time, and nothing on disk can predict them.
//! - **The current directory** sits between the Windows directory and `PATH` in the real order, and
//!   is left out: a file manager has no meaningful current directory to offer, and guessing one
//!   would put an answer in the list that the program being inspected would never see.
//! - The application directory is the **root's** folder for every level of the walk, not each
//!   DLL's own folder, which is what the loader does: the "application directory" belongs to the
//!   process, so a DLL in `C:\lib` loaded by a program in `C:\app` looks for its own imports in
//!   `C:\app`, not in `C:\lib`.
//!
//! # Why the walk is parallel
//!
//! For the same reason [`crate::fs::scan::scan_deep`] is: the work is a syscall waiting on the
//! file system, so a thread that is waiting is a module another thread could have been reading.
//! A level of the graph is resolved on up to [`hands`] threads at once, level by level, which is
//! what keeps a walk of a few hundred modules inside a tenth of a second warm.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// The graph
// ---------------------------------------------------------------------------

/// One binary in a dependency graph.
pub struct Module {
    /// The name it was imported by — which is a bare file name, and the only thing the
    /// importing binary actually said. The root's own file name, for the root.
    pub name: String,
    /// Where it was found, when it was. `None` for anything [`State`] says is not a file.
    pub path: Option<PathBuf>,
    /// The processor it was built for, as an `IMAGE_FILE_MACHINE_*` value. `0` when unknown,
    /// which is both "not read" and the real value of a processor-neutral image.
    pub machine: u16,
    pub state: State,
    /// What it imports, in import-table order.
    pub imports: Vec<Edge>,
}

/// One import: which module, and whether it is loaded up front or on first use.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Edge {
    pub to: usize,
    /// A **delay-load** import: the linker generated a stub, and the DLL is not opened until
    /// something calls into it. Worth telling apart from an ordinary import, because a missing
    /// delay-loaded DLL does not stop the program from starting — it stops one feature from
    /// working, later, somewhere else.
    pub delayed: bool,
}

/// How looking for a module turned out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Found on disk and read.
    Found,
    /// An API set: a name the loader resolves through a schema rather than a file. Not looked
    /// for, and not followed. See [`is_api_set`].
    ApiSet,
    /// Nothing in the search path is called this.
    Missing,
    /// There is a file, and it is not one this can read. The reason, in a few words.
    Unreadable(&'static str),
    /// Discovered, but the walk stopped before getting to it — see [`Graph::truncated`].
    Unvisited,
}

/// Everything reachable from one binary. The root is index 0.
pub struct Graph {
    pub modules: Vec<Module>,
    /// Where every name was looked for, in the order it was looked for in. Shown, because a
    /// location is only meaningful next to the list of places that were tried.
    pub search: Vec<PathBuf>,
    /// The walk stopped at [`BUDGET`] or [`PATIENCE`] rather than at the end of the graph, and
    /// what is here is therefore incomplete. Said out loud by the panel: a dependency list
    /// missing entries and not admitting it is worse than no list at all.
    pub truncated: bool,
    /// How long it took. The only honest way to claim this is fast.
    pub micros: u64,
}

impl Graph {
    /// The binary the walk started from, which is always module 0.
    pub fn root(&self) -> &Module {
        &self.modules[0]
    }

    /// The graph in three numbers: files, API sets, and modules that are neither.
    ///
    /// Three rather than one because "937 modules" is a misleading total. Most of a modern
    /// graph is [`is_api_set`] names — 688 of those 937, walking this program's own binary —
    /// and they are not files, cannot be missing, and are not what somebody counting
    /// dependencies means. The number worth reading is the first one.
    pub fn tally(&self) -> (usize, usize, usize) {
        let mut files = 0;
        let mut api_sets = 0;
        let mut missing = 0;
        for module in &self.modules {
            match module.state {
                State::Found => files += 1,
                State::ApiSet => api_sets += 1,
                State::Missing | State::Unreadable(_) => missing += 1,
                State::Unvisited => {}
            }
        }
        (files, api_sets, missing)
    }

    /// Every module that would stop the root from starting: not there, and reached by imports
    /// that are all loaded up front.
    ///
    /// The distinction is the whole reason [`Edge::delayed`] is recorded. Walking this program's
    /// own binary finds five missing DLLs on this machine — `HvsiFileTrust`, `AzureAttestManager`
    /// and three more — every one of them delay-loaded from somewhere deep inside `shell32`, and
    /// every one of them harmless: Windows ships the stubs for features that are not installed.
    /// Treating those the same as a missing `MSVCP140.dll` beside the executable is what made
    /// dependency lists into something people learned to ignore.
    ///
    /// Returns, for each one, the chain of modules from the root down to it, so the panel can
    /// open exactly the branches worth looking at and nothing else.
    pub fn breaks_loading(&self) -> Vec<Vec<usize>> {
        // Breadth-first from the root along non-delayed imports only, remembering how each
        // module was first reached — which gives the shortest such chain, and a simple one.
        let mut parent = vec![usize::MAX; self.modules.len()];
        let mut reached = vec![false; self.modules.len()];
        let mut queue = std::collections::VecDeque::from([0usize]);
        reached[0] = true;
        while let Some(at) = queue.pop_front() {
            for edge in &self.modules[at].imports {
                if edge.delayed || reached[edge.to] {
                    continue;
                }
                reached[edge.to] = true;
                parent[edge.to] = at;
                queue.push_back(edge.to);
            }
        }

        let mut out = Vec::new();
        for (i, module) in self.modules.iter().enumerate() {
            if !reached[i] || !matches!(module.state, State::Missing | State::Unreadable(_)) {
                continue;
            }
            let mut chain = vec![i];
            let mut at = parent[i];
            while at != usize::MAX {
                chain.push(at);
                at = parent[at];
            }
            chain.reverse();
            out.push(chain);
        }
        out
    }

    /// Whether module `i` was built for a different processor than the root.
    ///
    /// Windows will not load it, which makes this the second most useful thing a dependency
    /// walk can tell you after "it is not there". A machine of `0` is not a mismatch — that is
    /// a processor-neutral image, which is a legitimate answer rather than a missing one.
    pub fn foreign(&self, i: usize) -> bool {
        let root = self.modules[0].machine;
        let mine = self.modules[i].machine;
        root != 0 && mine != 0 && root != mine
    }
}

/// The name of an `IMAGE_FILE_MACHINE_*` value, as people write it.
pub fn machine_name(machine: u16) -> &'static str {
    match machine {
        0x014c => "x86",
        0x8664 => "x64",
        0xaa64 => "arm64",
        // ARM64EC and ARM64X: x64 code in an arm64 image, and both in one image. They load
        // alongside arm64, so they are not a mismatch, and they are not the same thing either.
        0xa641 => "arm64ec",
        0xa64e => "arm64x",
        0x01c0 | 0x01c2 | 0x01c4 => "arm",
        0x0200 => "ia64",
        0x0000 => "neutral",
        _ => "?",
    }
}

/// Whether a file is worth offering a dependency walk for, from its extension alone.
///
/// Everything Windows loads as an executable image and a handful of things that are DLLs under
/// another name — a Python extension, a Node addon, a DirectShow filter. Cheap to ask, so it is
/// what decides whether the shortcut does anything, and a file that passes this and turns out
/// not to be a PE says so in the panel rather than being refused here.
pub fn is_image(ext: &str) -> bool {
    const IMAGES: [&str; 13] = [
        "exe", "dll", "ocx", "sys", "cpl", "drv", "scr", "efi", "node", "pyd", "ax", "mun",
        "mui",
    ];
    IMAGES.iter().any(|known| ext.eq_ignore_ascii_case(known))
}

/// Whether a name is an **API set** rather than a file.
///
/// `api-ms-win-core-file-l1-2-0.dll` and its several hundred siblings are not DLLs. They are
/// names in a schema the loader carries, mapped at load time onto whichever real DLL implements
/// that contract on this build of Windows — usually `kernelbase.dll` or `ntdll.dll`. Some builds
/// do ship stub files of the same name and some do not, which is exactly why looking on disk is
/// the wrong question to ask about them.
///
/// This is the single biggest difference between a useful dependency list and the wall of red
/// that made the original Dependency Walker useless on anything built after Windows 7 — it
/// reported every one of these as missing. They are marked for what they are, and not followed:
/// their own imports are `ntdll`, which every module in the graph already has.
pub fn is_api_set(name: &str) -> bool {
    let name = name.as_bytes();
    let starts = |prefix: &[u8]| {
        name.len() > prefix.len()
            && name[..prefix.len()].eq_ignore_ascii_case(prefix)
    };
    starts(b"api-ms-") || starts(b"ext-ms-")
}

// ---------------------------------------------------------------------------
// The walk
// ---------------------------------------------------------------------------

/// How many modules a walk may discover.
///
/// Far above anything real — a large Qt application reaches about two hundred — because the
/// bound that actually bites is [`PATIENCE`]. This one is here so that a file crafted to import
/// itself under a thousand names costs a bounded amount rather than the machine's memory.
pub const BUDGET: usize = 4096;

/// How long a walk may take before it hands back what it has.
///
/// A warm walk of a real application is a few milliseconds; a cold one off a network share is
/// tens of milliseconds per module. Five seconds is long enough that nothing ordinary hits it
/// and short enough that a `PATH` entry pointing at a share that has gone away costs one wait
/// rather than a stuck panel.
pub const PATIENCE: Duration = Duration::from_secs(5);

/// Follow everything `root` needs, and everything those need.
///
/// The root is module 0 whatever happens to it — a file that is not a binary at all still comes
/// back as a graph of one module whose [`State`] says why, which is what lets the panel show the
/// answer in the same place it shows every other answer.
pub fn walk(root: &Path, budget: usize, patience: Duration) -> Graph {
    let started = Instant::now();
    let deadline = started + patience;
    let search = search_paths(root);
    let hands = hands();

    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());

    // The root is read here rather than through `probe`: it is named by a path and not by a
    // name to be resolved, which is the one module in the graph that is not a search result.
    let mut modules = Vec::new();
    let mut raw: Vec<Vec<(String, bool)>> = Vec::new();
    match read(root) {
        Ok(image) => {
            modules.push(Module {
                name,
                path: Some(root.to_path_buf()),
                machine: image.machine,
                state: State::Found,
                imports: Vec::new(),
            });
            raw.push(image.imports);
        }
        Err(why) => {
            modules.push(Module {
                name,
                path: Some(root.to_path_buf()),
                machine: 0,
                state: State::Unreadable(why),
                imports: Vec::new(),
            });
            raw.push(Vec::new());
        }
    }

    // Which module a name belongs to. Keyed by the *name*, lowercased, because resolution is a
    // function of the name alone: two references to `kernel32.dll` from two different modules
    // cannot come out as two different files, so one entry per name is exactly right — and it
    // is also what makes the graph finite when it has a cycle in it, which real ones do.
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut truncated = false;
    // The modules whose imports have been read but whose edges have not been built yet.
    let mut level = vec![0usize];

    while !level.is_empty() && !truncated {
        // Every name this level imports, with the modules that want it. New names get their
        // index here, before anything is read, so the edges can be attached in one pass.
        let mut fresh: Vec<usize> = Vec::new();
        for &from in &level {
            let wants = std::mem::take(&mut raw[from]);
            for (name, delayed) in wants {
                let key = name.to_ascii_lowercase();
                let to = match index.get(&key) {
                    Some(&known) => known,
                    None => {
                        if modules.len() >= budget {
                            truncated = true;
                            break;
                        }
                        let at = modules.len();
                        index.insert(key, at);
                        modules.push(Module {
                            name,
                            path: None,
                            machine: 0,
                            state: State::Unvisited,
                            imports: Vec::new(),
                        });
                        raw.push(Vec::new());
                        fresh.push(at);
                        at
                    }
                };
                modules[from].imports.push(Edge { to, delayed });
            }
            if truncated {
                break;
            }
        }

        if fresh.is_empty() {
            break;
        }
        let names: Vec<String> = fresh.iter().map(|&at| modules[at].name.clone()).collect();
        let (probed, out_of_time) = probe_all(&names, &search, deadline, hands);
        truncated |= out_of_time;
        for (&at, found) in fresh.iter().zip(probed) {
            modules[at].path = found.path;
            modules[at].machine = found.machine;
            modules[at].state = found.state;
            raw[at] = found.imports;
        }
        level = fresh;
    }

    Graph {
        modules,
        search,
        truncated,
        micros: started.elapsed().as_micros() as u64,
    }
}

/// Where a name is looked for, in order: the root binary's folder, then `PATH`.
///
/// Entries that are not directories are dropped **here, once**, rather than being tried for
/// every module. That is not tidiness: a `PATH` entry pointing at a share that has gone away
/// costs a connection timeout every time something is looked for in it, so a walk of two hundred
/// modules against a dead entry would be two hundred timeouts instead of the one this spends.
pub fn search_paths(root: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut add = |dir: PathBuf| {
        // Case-insensitively, because `C:\Windows\System32` and `c:\windows\system32` are one
        // directory and appear in `PATH` both ways round on a real machine.
        let key = dir.to_string_lossy().to_ascii_lowercase();
        if key.is_empty() || !seen.insert(key) {
            return;
        }
        if dir.is_dir() {
            out.push(dir);
        }
    };

    if let Some(folder) = root.parent() {
        add(folder.to_path_buf());
    }
    for dir in system_dirs() {
        add(dir);
    }
    if let Some(path) = std::env::var_os("PATH") {
        // `split_paths` rather than splitting on `;`, so a quoted entry containing a separator
        // survives and the same code is right on the platform where the separator is `:`.
        for dir in std::env::split_paths(&path) {
            add(dir);
        }
    }
    out
}

/// `System32`, `System` and the Windows directory, in the order the loader tries them.
///
/// **The three that have to come before `PATH`**, and getting that wrong is not a subtle error: on
/// an ordinary machine `System32` is in `PATH` as well, so a walk that only knew about `PATH` still
/// found `kernel32.dll` — but it found it at whatever position `System32` happened to occupy in
/// that user's `PATH`, which is to say *after* anything installed in front of it. The loader never
/// does that. `PATH` is the last resort and these three are not.
///
/// Asked of the platform rather than assembled from `%SystemRoot%`, because the environment is
/// writable and this is not: `GetSystemDirectoryW` is the authoritative answer, and it is also the
/// one that gets the *redirected* answer right — a 32-bit process is told `SysWOW64`, which is the
/// directory a 32-bit binary's imports really would come from. This program is 64-bit, so it is
/// told `System32`.
///
/// `System` is the 16-bit system directory and holds almost nothing on a modern install. It is here
/// because the loader looks there, in that position, and a search order that is *nearly* the
/// loader's is worse than one that says which parts it models.
fn system_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{
            GetSystemDirectoryW, GetWindowsDirectoryW,
        };

        // Both calls take a buffer and return the length written, or the length needed if it did
        // not fit. `MAX_PATH` is the documented bound for both, and neither has a long-path form.
        let ask = |call: unsafe extern "system" fn(*mut u16, u32) -> u32| -> Option<PathBuf> {
            let mut buffer = [0u16; 260];
            // SAFETY: the buffer is a local that outlives the call, and its length is passed as
            // the count of `u16`s, which is what both functions document.
            let len = unsafe { call(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
            (len > 0 && len < buffer.len())
                .then(|| PathBuf::from(String::from_utf16_lossy(&buffer[..len])))
        };

        let windows = ask(GetWindowsDirectoryW);
        let mut out = Vec::with_capacity(3);
        if let Some(system32) = ask(GetSystemDirectoryW) {
            out.push(system32);
        }
        if let Some(windows) = &windows {
            out.push(windows.join("System"));
        }
        out.extend(windows);
        out
    }
    #[cfg(not(windows))]
    Vec::new()
}

/// One name, looked for and read.
struct Probe {
    path: Option<PathBuf>,
    machine: u16,
    state: State,
    imports: Vec<(String, bool)>,
}

/// Resolve and read a level of the graph, on up to `hands` threads.
///
/// In the order given, so the display order is the import order and not the order the threads
/// happened to finish in. Each worker takes the next name nobody has claimed rather than a fixed
/// share of them, because modules differ enormously in how long they take — one name off a
/// network share would otherwise leave every other thread idle behind it.
///
/// **A level no larger than the thread count goes inline.** Spawning a thread costs about as
/// much as reading a warm binary's headers, so a level of three does not pay for three threads;
/// the rule asks for two names each before it is worth it. The same rule, for the same measured
/// reason, as [`crate::fs::scan`]'s.
///
/// The second half of the answer is whether the deadline stopped it, because the caller cannot
/// tell that from a complete-looking list of modules that are all marked unvisited.
fn probe_all(
    names: &[String],
    search: &[PathBuf],
    until: Instant,
    hands: usize,
) -> (Vec<Probe>, bool) {
    let out_of_time = || Instant::now() >= until;

    if names.len() <= hands || hands <= 1 {
        let mut out = Vec::with_capacity(names.len());
        let mut gave_up = false;
        for name in names {
            if out_of_time() {
                gave_up = true;
            }
            out.push(if gave_up {
                unvisited()
            } else {
                probe(name, search)
            });
        }
        return (out, gave_up);
    }

    let next = AtomicUsize::new(0);
    let parts: Vec<Vec<(usize, Probe)>> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..hands.min(names.len()))
            .map(|_| {
                let next = &next;
                scope.spawn(move || {
                    // Per-thread, and these threads are new: a `PATH` entry on an empty card
                    // reader would otherwise raise "Please insert a disk" from inside the
                    // syscall, on a thread with no window to put it in front of.
                    crate::fs::scan::silence_device_dialogs();
                    let mut mine = Vec::new();
                    loop {
                        let at = next.fetch_add(1, Ordering::Relaxed);
                        let Some(name) = names.get(at) else {
                            return mine;
                        };
                        if out_of_time() {
                            return mine;
                        }
                        mine.push((at, probe(name, search)));
                    }
                })
            })
            .collect();
        // A worker that panicked contributes nothing rather than taking the window with it.
        workers.into_iter().filter_map(|w| w.join().ok()).collect()
    });

    let mut done: Vec<Option<Probe>> = (0..names.len()).map(|_| None).collect();
    let mut answered = 0;
    for (at, found) in parts.into_iter().flatten() {
        if let Some(slot) = done.get_mut(at) {
            if slot.is_none() {
                answered += 1;
            }
            *slot = Some(found);
        }
    }
    let gave_up = answered < names.len();
    (
        done.into_iter().map(|p| p.unwrap_or_else(unvisited)).collect(),
        gave_up,
    )
}

fn unvisited() -> Probe {
    Probe {
        path: None,
        machine: 0,
        state: State::Unvisited,
        imports: Vec::new(),
    }
}

/// Look one name up and read what it turns out to be.
fn probe(name: &str, search: &[PathBuf]) -> Probe {
    if is_api_set(name) {
        return Probe {
            path: None,
            machine: 0,
            state: State::ApiSet,
            imports: Vec::new(),
        };
    }
    let Some(path) = resolve(name, search) else {
        return Probe {
            path: None,
            machine: 0,
            state: State::Missing,
            imports: Vec::new(),
        };
    };
    match read(&path) {
        Ok(image) => Probe {
            path: Some(path),
            machine: image.machine,
            state: State::Found,
            imports: image.imports,
        },
        Err(why) => Probe {
            path: Some(path),
            machine: 0,
            state: State::Unreadable(why),
            imports: Vec::new(),
        },
    }
}

/// The first directory in `search` that has a file of this name.
fn resolve(name: &str, search: &[PathBuf]) -> Option<PathBuf> {
    // An import name is a bare file name. Anything with a separator in it is either a
    // hand-built binary or an attempt to reach out of the search path, and joining it would
    // do exactly that, so it is refused rather than followed.
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains(':') {
        return None;
    }
    search
        .iter()
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// How many modules are read at once.
///
/// The same shape and the same reason as [`crate::fs::scan`]'s: this waits on the file system
/// rather than on the processor, so past a handful more threads buy nothing and cost context
/// switches. Eight, because a walk is one burst of a few hundred reads rather than a trickle.
fn hands() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().clamp(2, 8))
        .unwrap_or(2)
}

// ---------------------------------------------------------------------------
// The file format
// ---------------------------------------------------------------------------

/// What a binary says about itself, which is all this needs from it.
pub struct Imports {
    pub machine: u16,
    /// The DLLs it names, in import-table order, each with whether the reference is
    /// delay-loaded. Deduplicated: a DLL named in both tables appears once, as an ordinary
    /// import, because that is what decides whether it is opened at start-up.
    pub imports: Vec<(String, bool)>,
}

/// The import table of the binary at `path`.
///
/// `Err` with something short enough to put in a row for anything that is not a PE image this
/// can follow, which includes a 16-bit binary, a `.NET`-only assembly with a damaged header, a
/// text file somebody named `.exe`, and a file that cannot be opened at all.
pub fn read(path: &Path) -> Result<Imports, &'static str> {
    let mut image = Image::open(path)?;
    let imports = image.imports();
    Ok(Imports {
        machine: image.machine,
        imports,
    })
}

/// The two data directories this reads.
const DIR_IMPORT: usize = 1;
const DIR_DELAY: usize = 13;

/// A cap on the descriptor arrays, so a damaged or hostile file cannot loop.
const MAX_DESCRIPTORS: u64 = 4096;

/// One section's place in the file and in memory.
struct Section {
    va: u32,
    vsize: u32,
    raw: u32,
    rsize: u32,
}

/// An open binary, and enough of its headers to turn an address into a file offset.
struct Image {
    file: File,
    /// Its length, so a read near the end can be cut to what is there.
    len: u64,
    machine: u16,
    /// Where the image would be mapped, which is what an old-style delay-load descriptor's
    /// addresses are relative to.
    base: u64,
    /// How much of the front of the file is mapped one-to-one, for an address that lands in
    /// the headers rather than in a section.
    headers: u64,
    sections: Vec<Section>,
    dirs: Vec<(u32, u32)>,
}

impl Image {
    fn open(path: &Path) -> Result<Self, &'static str> {
        let mut file = File::open(path).map_err(|_| "cannot be opened")?;
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);

        // The DOS stub, which every PE file still carries, and the one field in it that
        // matters: where the real header is.
        let dos = at(&mut file, 0, 64).ok_or("not a binary")?;
        if &dos[..2] != b"MZ" {
            return Err("not a binary");
        }
        let start = u32_at(&dos, 0x3c).ok_or("not a binary")? as u64;
        // A 16-bit binary points at a `NE` header, and a file that is merely short points at
        // nothing at all. Both come out here.
        let coff = at(&mut file, start, 24).ok_or("not a Windows binary")?;
        if &coff[..4] != b"PE\0\0" {
            return Err("not a Windows binary");
        }
        let machine = u16_at(&coff, 4).ok_or("damaged headers")?;
        let section_count = u16_at(&coff, 6).ok_or("damaged headers")? as usize;
        let optional_size = u16_at(&coff, 20).ok_or("damaged headers")? as usize;

        // The optional header, which is only optional for an object file. Its length is
        // declared rather than fixed, because the number of data directories at the end of it
        // is a field of its own.
        let optional_at = start + 24;
        let optional = at(&mut file, optional_at, optional_size.min(4096))
            .ok_or("damaged headers")?;
        let magic = u16_at(&optional, 0).ok_or("damaged headers")?;
        // 0x10b is PE32 and 0x20b is PE32+, and the only difference that matters here is that
        // one of them has a 64-bit image base, which moves everything after it by four bytes.
        let (base, dirs_at) = match magic {
            0x10b => (u32_at(&optional, 28).ok_or("damaged headers")? as u64, 96),
            0x20b => (u64_at(&optional, 24).ok_or("damaged headers")?, 112),
            _ => return Err("not a Windows binary"),
        };
        let count = u32_at(&optional, dirs_at - 4).unwrap_or(0).min(16) as usize;
        let mut dirs = Vec::with_capacity(count);
        for i in 0..count {
            let at = dirs_at + i * 8;
            let (Some(rva), Some(size)) = (u32_at(&optional, at), u32_at(&optional, at + 4)) else {
                break;
            };
            dirs.push((rva, size));
        }

        // The section table, which is what makes an address in the file findable.
        let table_at = optional_at + optional_size as u64;
        let table = at(&mut file, table_at, section_count.min(96) * 40).unwrap_or_default();
        let mut sections = Vec::with_capacity(table.len() / 40);
        for chunk in table.chunks_exact(40) {
            let (Some(vsize), Some(va), Some(rsize), Some(raw)) = (
                u32_at(chunk, 8),
                u32_at(chunk, 12),
                u32_at(chunk, 16),
                u32_at(chunk, 20),
            ) else {
                break;
            };
            sections.push(Section {
                va,
                vsize,
                raw,
                rsize,
            });
        }

        Ok(Self {
            file,
            len,
            machine,
            base,
            headers: table_at + (sections.len() * 40) as u64,
            sections,
            dirs,
        })
    }

    /// Where an address lands in the file, if it lands in the file at all.
    ///
    /// A section is longer in memory than on disk whenever it has a zero-filled tail, and an
    /// address in that tail has no bytes behind it — which is a legitimate thing for a data
    /// directory to point at and has to come back as `None` rather than as a wild offset.
    fn offset(&self, rva: u32) -> Option<u64> {
        for section in &self.sections {
            let span = if section.vsize == 0 {
                section.rsize
            } else {
                section.vsize
            };
            let end = section.va as u64 + span as u64;
            if (rva as u64) >= section.va as u64 && (rva as u64) < end {
                let into = rva - section.va;
                if into >= section.rsize {
                    return None;
                }
                return Some(section.raw as u64 + into as u64);
            }
        }
        // Before the first section: the headers themselves are mapped one to one.
        ((rva as u64) < self.headers).then_some(rva as u64)
    }

    fn at_rva(&mut self, rva: u64, len: usize) -> Option<Vec<u8>> {
        let rva = u32::try_from(rva).ok()?;
        let offset = self.offset(rva)?;
        at(&mut self.file, offset, len)
    }

    /// The NUL-terminated name at an address. DLL names are ASCII by specification.
    fn name_at(&mut self, rva: u64) -> Option<String> {
        // A block rather than a byte at a time, which is what keeps a module's cost to a couple
        // of dozen small reads — but no further than the end of the file, or a name that
        // happens to sit in the last few bytes of it could not be read at all.
        let offset = self.offset(u32::try_from(rva).ok()?)?;
        let len = self.len.saturating_sub(offset).min(256) as usize;
        let block = at(&mut self.file, offset, len)?;
        let end = block.iter().position(|&b| b == 0).unwrap_or(block.len());
        let name = String::from_utf8_lossy(&block[..end]).into_owned();
        // A name with anything unprintable in it is a sign the address was wrong rather than a
        // DLL with an unusual name, and following it would put nonsense in the panel.
        let sane = !name.is_empty()
            && name.len() <= 255
            && name.bytes().all(|b| b.is_ascii_graphic() || b == b' ');
        sane.then_some(name)
    }

    fn dir(&self, which: usize) -> Option<(u32, u32)> {
        match self.dirs.get(which) {
            Some(&(rva, size)) if rva != 0 && size != 0 => Some((rva, size)),
            _ => None,
        }
    }

    /// Every DLL named in the two import tables.
    fn imports(&mut self) -> Vec<(String, bool)> {
        let mut out: Vec<(String, bool)> = Vec::new();
        let mut seen: HashMap<String, usize> = HashMap::new();

        // `IMAGE_IMPORT_DESCRIPTOR`: five words, the fourth of which is the name, and an
        // all-zero one ends the array.
        if let Some((rva, _)) = self.dir(DIR_IMPORT) {
            for step in 0..MAX_DESCRIPTORS {
                let Some(entry) = self.at_rva(rva as u64 + step * 20, 20) else {
                    break;
                };
                let Some(name_rva) = u32_at(&entry, 12) else {
                    break;
                };
                if name_rva == 0 {
                    break;
                }
                if let Some(name) = self.name_at(name_rva as u64) {
                    note(&mut out, &mut seen, name, false);
                }
            }
        }

        // `IMAGE_DELAYLOAD_DESCRIPTOR`: eight words, the second of which is the name.
        //
        // With a wrinkle worth writing down: the addresses in it are relative to the image
        // base only when the low bit of the attributes says so. Descriptors written by the
        // linkers of the late nineties hold absolute virtual addresses instead, and reading
        // one of those as an RVA lands somewhere arbitrary in the file — which is exactly the
        // kind of thing that produces a plausible-looking name that is not there.
        if let Some((rva, _)) = self.dir(DIR_DELAY) {
            for step in 0..MAX_DESCRIPTORS {
                let Some(entry) = self.at_rva(rva as u64 + step * 32, 32) else {
                    break;
                };
                let (Some(attributes), Some(field)) = (u32_at(&entry, 0), u32_at(&entry, 4))
                else {
                    break;
                };
                if field == 0 {
                    break;
                }
                let name_rva = if attributes & 1 != 0 {
                    Some(field as u64)
                } else {
                    (field as u64).checked_sub(self.base)
                };
                if let Some(name) = name_rva.and_then(|rva| self.name_at(rva)) {
                    note(&mut out, &mut seen, name, true);
                }
            }
        }

        out
    }
}

/// Add a name unless it is already there, in which case an ordinary import wins over a
/// delay-loaded one — a DLL named in both tables is opened at start-up, which is the fact worth
/// showing.
fn note(
    out: &mut Vec<(String, bool)>,
    seen: &mut HashMap<String, usize>,
    name: String,
    delayed: bool,
) {
    let key = name.to_ascii_lowercase();
    match seen.get(&key) {
        Some(&at) => {
            if !delayed {
                out[at].1 = false;
            }
        }
        None => {
            seen.insert(key, out.len());
            out.push((name, delayed));
        }
    }
}

/// `len` bytes at `offset`, or `None` if the file is shorter than that.
fn at(file: &mut File, offset: u64, len: usize) -> Option<Vec<u8>> {
    if len == 0 {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut buffer = vec![0u8; len];
    file.read_exact(&mut buffer).ok()?;
    Some(buffer)
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test binary is itself a PE file, which makes it the one fixture that is real, is
    /// always there, and is built for whatever this machine is.
    fn me() -> PathBuf {
        std::env::current_exe().expect("a test process has an executable")
    }

    #[test]
    fn a_real_binary_names_what_it_needs() {
        let image = read(&me()).expect("the test binary is a PE file");
        assert_ne!(image.machine, 0, "a real image says what it was built for");
        assert!(
            !image.imports.is_empty(),
            "every Windows binary imports something"
        );
        // Whatever else a Rust program on Windows imports, it imports the C runtime's host or
        // the API sets that stand in for it. What is asserted here is the shape of the answer
        // rather than one name: every entry is a plausible module name.
        for (name, _) in &image.imports {
            assert!(
                name.len() > 3 && name.contains('.'),
                "{name:?} does not look like a DLL name"
            );
        }
        // The two tables are merged with no duplicates, which is what the panel counts on.
        let mut keys: Vec<String> = image
            .imports
            .iter()
            .map(|(name, _)| name.to_ascii_lowercase())
            .collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(before, keys.len(), "a DLL is named twice: {keys:?}");
    }

    #[test]
    fn something_that_is_not_a_binary_says_so_rather_than_guessing() {
        let text = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert_eq!(read(&text).err(), Some("not a binary"));
        assert_eq!(
            read(&PathBuf::from("no-such-file-anywhere.dll")).err(),
            Some("cannot be opened")
        );
    }

    /// The point of the whole module: a real graph, more than one level deep, with every
    /// module either accounted for or honestly marked.
    #[test]
    fn a_walk_finds_where_each_dependency_lives_and_keeps_going() {
        let graph = walk(&me(), BUDGET, PATIENCE);
        assert_eq!(graph.root().state, State::Found);
        assert!(
            graph.modules.len() > 2,
            "a walk of a real binary reaches more than its own imports: {}",
            graph.modules.len()
        );

        // Recursion: something in the graph is a dependency of a dependency. Without this the
        // test would pass on a walk that read the root's import table and stopped.
        let second_level = graph.modules[0]
            .imports
            .iter()
            .any(|edge| !graph.modules[edge.to].imports.is_empty());
        assert!(second_level, "nothing was followed past the first level");

        // Every module is one of the four things it can be, and a found one has a path that
        // exists and came off the search list.
        for (i, module) in graph.modules.iter().enumerate() {
            match module.state {
                State::Found => {
                    let path = module.path.as_ref().expect("a found module has a path");
                    assert!(path.is_file(), "{path:?} was reported and is not there");
                    assert!(
                        graph
                            .search
                            .iter()
                            .any(|dir| path.parent() == Some(dir.as_path())),
                        "{path:?} is not in any directory that was searched"
                    );
                    assert_ne!(module.machine, 0, "{:?} has no machine", module.name);
                }
                State::ApiSet => {
                    assert!(is_api_set(&module.name));
                    assert!(module.path.is_none(), "an API set is not a file");
                }
                State::Missing | State::Unreadable(_) => {}
                State::Unvisited => assert!(
                    graph.truncated,
                    "{:?} was never visited and the walk does not admit stopping",
                    module.name
                ),
            }
            // Nothing points outside the graph, and the root is nobody's index by accident.
            for edge in &module.imports {
                assert!(edge.to < graph.modules.len(), "module {i} has a wild edge");
            }
        }

        // The system DLLs are found through `PATH`, which is what the search order claims.
        assert!(
            graph
                .modules
                .iter()
                .any(|m| matches!(m.state, State::Found) && m.path.as_ref() != Some(&me())),
            "nothing but the root was resolved"
        );
    }

    /// A cycle does not become an endless walk, and it does not become a duplicated module
    /// either. Real graphs have them — `kernel32` and `kernelbase` refer to each other through
    /// forwarders — so this is checked against whatever this machine's really are.
    #[test]
    fn a_graph_holds_each_module_once_however_many_things_want_it() {
        let graph = walk(&me(), BUDGET, PATIENCE);
        let mut names: Vec<String> = graph
            .modules
            .iter()
            .map(|m| m.name.to_ascii_lowercase())
            .collect();
        let before = names.len();
        names.sort();
        names.dedup();
        assert_eq!(before, names.len(), "a module appears twice in the graph");
    }

    /// The root binary's own folder is searched before `PATH`.
    ///
    /// Checked by putting a copy of the test binary in a folder of its own under a name
    /// nothing else could resolve to, and walking *that*: its first search directory has to be
    /// the folder the copy is in.
    #[test]
    fn the_binarys_own_folder_is_looked_in_first() {
        let root = crate::sandbox::dir("pe");
        crate::sandbox::remove(&root);
        std::fs::create_dir_all(&root).expect("a directory in the temp folder");
        let copy = root.join("yafe-fixture.dll");
        std::fs::copy(me(), &copy).expect("a copy of the test binary");

        let search = search_paths(&copy);
        assert_eq!(
            search.first().map(|p| p.as_path()),
            Some(root.as_path()),
            "the binary's own folder is not the first place looked"
        );
        // And a name that only exists there resolves to it, which is the behaviour that
        // search order is *for*.
        assert_eq!(
            resolve("yafe-fixture.dll", &search).as_deref(),
            Some(copy.as_path())
        );
        // Nothing reaches out of the search path, however the name is written.
        assert_eq!(resolve(r"..\yafe-fixture.dll", &search), None);
        assert_eq!(resolve(&copy.to_string_lossy(), &search), None);

        let graph = walk(&copy, BUDGET, PATIENCE);
        assert_eq!(graph.root().state, State::Found);
        assert!(graph.modules.len() > 2);
        crate::sandbox::remove(&root);
    }

    /// What a walk of a real binary actually costs, and what it comes back with.
    ///
    /// Ignored, like `scan::flatten_speed`: it is a measurement rather than an assertion, it
    /// prints, and what it measures depends on the machine. `cargo test -- --ignored
    /// --nocapture walk_speed` — with `YAFE_WALK` set to walk something larger than the test
    /// binary, which is what a number worth having is measured on.
    #[test]
    #[ignore]
    fn walk_speed() {
        let target = std::env::var_os("YAFE_WALK")
            .map(PathBuf::from)
            .unwrap_or_else(me);
        let graph = walk(&target, BUDGET, PATIENCE);
        let (files, api_sets, missing) = graph.tally();
        println!(
            "{} on {} threads: {} modules ({files} files, {api_sets} api sets), \
             {missing} missing, {:.1} ms{}",
            target.display(),
            hands(),
            graph.modules.len(),
            graph.micros as f64 / 1000.0,
            if graph.truncated { " (truncated)" } else { "" }
        );
        for (i, module) in graph.modules.iter().enumerate() {
            let where_ = match (&module.path, module.state) {
                (Some(path), State::Found) => path
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                (_, State::ApiSet) => "(api set)".to_owned(),
                (_, State::Missing) => "NOT FOUND".to_owned(),
                (_, State::Unreadable(why)) => why.to_owned(),
                (_, State::Unvisited) => "(not visited)".to_owned(),
                (None, State::Found) => String::new(),
            };
            println!(
                "  {:>3} {:<34} {:<8} {:<3} {where_}",
                i,
                module.name,
                machine_name(module.machine),
                module.imports.len(),
            );
        }
    }

    /// **The system directories come before `PATH`, and the binary's own folder before them.**
    ///
    /// The order is the whole answer this module gives, so it is the thing worth pinning. What it
    /// catches is the mistake it was written to fix: `System32` is in `PATH` on every machine, so a
    /// search that only knew about `PATH` still *found* `kernel32.dll` and every test passed — it
    /// simply reported it as coming from wherever `System32` sat in that user's `PATH`, behind
    /// anything installed in front of it.
    #[cfg(windows)]
    #[test]
    fn the_system_directories_are_looked_in_before_path() {
        let me = me();
        let search = search_paths(&me);
        let at = |what: &str| {
            search
                .iter()
                .position(|dir| dir.to_string_lossy().eq_ignore_ascii_case(what))
        };

        let system = system_dirs();
        assert!(
            !system.is_empty(),
            "the platform would not say where Windows is"
        );
        let system32 = system[0].to_string_lossy().to_string();
        assert!(
            system32.to_lowercase().ends_with("system32"),
            "{system32} is not the system directory"
        );

        // The binary's own folder is first, and the system directory is right behind it.
        assert_eq!(
            search.first().map(|p| p.as_path()),
            me.parent(),
            "the binary's own folder is not the first place looked"
        );
        let system32_at = at(&system32).expect("the system directory is in the list");
        assert_eq!(system32_at, 1, "the system directory is not second: {search:?}");

        // And every one of the three comes before every `PATH` entry that is not itself one of
        // them. That is the assertion with teeth: `System32` is nearly always in `PATH` too, so
        // this is checking a *position* and not a presence.
        let from_path: Vec<String> = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|dir| dir.to_string_lossy().to_lowercase())
            .collect();
        // Anything that has a *reason* to be near the front, whatever `PATH` also says about it.
        // The binary's own folder is one of them and has to be excluded by name: cargo puts the
        // build directory on `PATH` when it runs a test, so under `cargo test` the first entry is
        // in both lists and the assertion below would fire on it.
        let mut excused: Vec<String> = system
            .iter()
            .map(|dir| dir.to_string_lossy().to_lowercase())
            .collect();
        excused.extend(me.parent().map(|dir| dir.to_string_lossy().to_lowercase()));
        let first_from_path = search
            .iter()
            .position(|dir| {
                let key = dir.to_string_lossy().to_lowercase();
                from_path.contains(&key) && !excused.contains(&key)
            })
            .expect("this machine's PATH has something in it that is not a system directory");
        for (which, dir) in system.iter().enumerate() {
            let Some(found) = at(&dir.to_string_lossy()) else {
                // `System` does not exist on every install, and a directory that is not there is
                // dropped up front — see `search_paths`.
                continue;
            };
            assert!(
                found < first_from_path,
                "system directory {which} ({}) is at {found}, behind a PATH entry at \
                 {first_from_path}",
                dir.display()
            );
        }

        // Nothing appears twice, however many of these lists it is in.
        let mut keys: Vec<String> = search
            .iter()
            .map(|dir| dir.to_string_lossy().to_lowercase())
            .collect();
        let before = keys.len();
        keys.sort();
        keys.dedup();
        assert_eq!(before, keys.len(), "a directory is searched twice");
    }

    #[test]
    fn an_api_set_is_told_apart_from_a_missing_dll() {
        assert!(is_api_set("api-ms-win-core-file-l1-2-0.dll"));
        assert!(is_api_set("API-MS-WIN-CRT-RUNTIME-L1-1-0.DLL"));
        assert!(is_api_set("ext-ms-win-ntuser-window-l1-1-0.dll"));
        assert!(!is_api_set("kernel32.dll"));
        // Not merely a prefix: the name has to have something after it.
        assert!(!is_api_set("api-ms-"));
        assert!(!is_api_set("apiset.dll"));
    }

    #[test]
    fn only_a_binary_is_offered_a_walk() {
        for yes in ["exe", "EXE", "dll", "Sys", "ocx", "node", "pyd"] {
            assert!(is_image(yes), "{yes} is a binary");
        }
        for no in ["", "txt", "rs", "zip", "exe2", "dl"] {
            assert!(!is_image(no), "{no} is not a binary");
        }
    }

    /// A walk with no patience hands back what it has rather than nothing at all.
    ///
    /// The root is read whatever the deadline says — it is read before the clock is consulted —
    /// so the honest answer to "no time at all" is one module and `truncated`.
    #[test]
    fn a_walk_out_of_patience_admits_it() {
        let graph = walk(&me(), BUDGET, Duration::ZERO);
        assert_eq!(graph.root().state, State::Found, "the root is always read");
        assert!(graph.truncated, "a walk that stopped early has to say so");
        assert!(graph
            .modules
            .iter()
            .skip(1)
            .all(|m| m.state == State::Unvisited));
    }

    /// And a walk with no budget stops at the budget, for the same reason.
    #[test]
    fn a_walk_stops_at_its_budget_and_admits_it() {
        let graph = walk(&me(), 2, PATIENCE);
        assert!(graph.modules.len() <= 2);
        assert!(graph.truncated);
    }

    /// Reading in parallel gives the same answer as reading one at a time.
    ///
    /// The thread count is what [`probe_all`] branches on — under it the level goes inline —
    /// so this drives both paths over the same real graph and compares them name for name.
    #[test]
    fn reading_in_parallel_does_not_change_the_answer() {
        let search = search_paths(&me());
        let image = read(&me()).expect("the test binary is a PE file");
        let names: Vec<String> = image.imports.into_iter().map(|(name, _)| name).collect();
        if names.len() < 2 {
            println!("this binary imports {} modules; nothing to compare", names.len());
            return;
        }
        let until = Instant::now() + PATIENCE;
        let (inline, _) = probe_all(&names, &search, until, 1);
        for hands in [2, 4, 8] {
            let (threaded, gave_up) = probe_all(&names, &search, until, hands);
            assert!(!gave_up, "a warm walk on {hands} threads ran out of time");
            assert_eq!(inline.len(), threaded.len());
            for (one, many) in inline.iter().zip(&threaded) {
                assert_eq!(one.path, many.path, "a different file was resolved");
                assert_eq!(one.state, many.state);
                assert_eq!(one.machine, many.machine);
                assert_eq!(one.imports, many.imports, "a different import list");
            }
        }
    }
}
