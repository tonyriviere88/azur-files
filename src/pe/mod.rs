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

    /// **Which modules are opened before `main` runs**, by index.
    ///
    /// Reachable from the root along imports that are *all* loaded up front — which is a stronger
    /// statement than "something imports it without delay-loading it", and the difference is a real
    /// one: a DLL that `shell32` imports normally is not loaded at start-up if `shell32` itself is
    /// only ever delay-loaded. The weaker rule was written twice in the panel, once each way, and the
    /// two disagreed about the same module.
    ///
    /// A fact about the graph rather than about a panel, which is why it lives here beside
    /// [`foreign`](Self::foreign) and [`tally`](Self::tally) rather than in the two places that were
    /// each deriving it.
    ///
    /// **With [`missing`](Self::missing) this says which modules would stop the program from
    /// starting**, which is the distinction [`Edge::delayed`] is recorded for and the one that makes a
    /// dependency list worth reading: a module that is absent *and* opened up front is a program that
    /// will not run, and one that is absent and only ever delay-loaded is a feature that will not work,
    /// later, somewhere else. Walking this program's own binary finds five of the second kind and none
    /// of the first — Windows ships stubs for features that are not installed — and treating those the
    /// same as a missing `MSVCP140.dll` beside the executable is what made dependency lists into
    /// something people learned to ignore. Two composable facts rather than a third walk that answered
    /// only their intersection.
    pub fn upfront(&self) -> Vec<bool> {
        let mut reached = vec![false; self.modules.len()];
        let mut queue = std::collections::VecDeque::from([0usize]);
        reached[0] = true;
        while let Some(at) = queue.pop_front() {
            for edge in &self.modules[at].imports {
                if edge.delayed || reached[edge.to] {
                    continue;
                }
                reached[edge.to] = true;
                queue.push_back(edge.to);
            }
        }
        reached
    }

    /// **Every** module that is not there, delay-loaded or not, with the chain down to each.
    ///
    /// The list [`breaks_loading`](Self::breaks_loading) deliberately leaves out, and the reason it
    /// has to exist beside it: the panel's own title says "5 missing" — the count comes from
    /// [`tally`](Self::tally), which does not care how a module was reached — and on this machine all
    /// five of them are delay-loaded, so a tree that only opened the branches `breaks_loading` names
    /// opened none of them. A count of something the display then cannot show is the one thing a
    /// dependency list must not do; it is exactly the reason people stopped believing the original.
    ///
    /// So the count and this list are the same set, and the difference between the two kinds is
    /// carried by how each row is *drawn* rather than by which ones exist.
    pub fn missing(&self) -> Vec<Vec<usize>> {
        self.chains(is_absent)
    }

    /// The shortest chain from the root down to every module `want` says yes to.
    ///
    /// The same question [`missing`](Self::missing) asks, asked about anything: the panel's search box
    /// uses it to answer "where does this come in, and through what" — which is what a tree can say
    /// and a filter over the rows on show cannot.
    pub fn chains_to(&self, want: impl Fn(&Module) -> bool) -> Vec<Vec<usize>> {
        self.chains(want)
    }

    /// One breadth-first walk from the root, and the chain down to each module that is wanted.
    ///
    /// Every edge, delay-loaded or not — which is what makes the chain the *shortest* one and not the
    /// shortest that avoids a delay-load. Whether anything on it is opened up front is
    /// [`upfront`](Self::upfront)'s question, asked of the module rather than of the route.
    fn chains(&self, want: impl Fn(&Module) -> bool) -> Vec<Vec<usize>> {
        // Breadth-first from the root, remembering how each module was first reached — which gives
        // the shortest such chain, and a simple one.
        let mut parent = vec![usize::MAX; self.modules.len()];
        let mut reached = vec![false; self.modules.len()];
        let mut queue = std::collections::VecDeque::from([0usize]);
        reached[0] = true;
        while let Some(at) = queue.pop_front() {
            for edge in &self.modules[at].imports {
                if reached[edge.to] {
                    continue;
                }
                reached[edge.to] = true;
                parent[edge.to] = at;
                queue.push_back(edge.to);
            }
        }

        let mut out = Vec::new();
        for (i, module) in self.modules.iter().enumerate() {
            if !reached[i] || !want(module) {
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

/// One symbol a binary **exports**: what it is called, and what the loader binds to it.
///
/// Read on demand rather than during the walk — see [`exports`] — because a dependency graph of two
/// hundred modules is a couple of million symbols, and the question is only ever asked about one
/// module at a time.
pub struct Export {
    /// The name it is exported under. `None` for an export that has only an ordinal, which is legal
    /// and is how parts of a few Microsoft DLLs still ship.
    pub name: Option<String>,
    /// The number it sits at in the export table, which is what an importer that names no name asks
    /// for.
    pub ordinal: u16,
    /// What the loader binds to it: code here, or a name somewhere else.
    pub bound: Bound,
}

/// What an export resolves to.
///
/// One field rather than an address and an optional forwarding target, because it is one answer: an
/// export is code or a redirection, never both and never neither. Written as two `Option`s it was an
/// invariant held up by one expression sixty lines from its only reader, and the reader carried an arm
/// for the pair that cannot happen.
pub enum Bound {
    /// Its entry point, as an address in the image.
    Entry(u32),
    /// `NTDLL.RtlAllocateHeap` — an export that is really somewhere else. Most of what `kernel32.dll`
    /// still appears to export is this.
    Forward(String),
}

/// One symbol a binary **imports** from one particular DLL.
///
/// Which is the other half of the same question and not the same list: a module exports its whole
/// surface, and each of its importers uses a handful of it. See [`imported_from`].
pub struct Symbol {
    /// The name it was imported by. `None` when it was imported by ordinal, in which case
    /// [`Symbol::ordinal`] is the whole reference.
    pub name: Option<String>,
    /// Set only for an import by ordinal, because that is the only case where the number is what the
    /// binary actually said.
    pub ordinal: Option<u16>,
    /// The exporting module's name-table index, as this binary was built against it — so a hint
    /// that no longer matches the DLL on disk is how you find a binary built against another
    /// version of it.
    pub hint: Option<u16>,
    /// Whether the reference is delay-loaded. Per symbol and not per module, because a binary can
    /// import from one DLL both ways at once.
    pub delayed: bool,
}

/// Every symbol the binary at `path` exports, in ordinal order — which is the file's own order, so
/// no policy is applied here. Empty for a binary that exports nothing, which is most `.exe`s.
pub fn exports(path: &Path) -> Result<Vec<Export>, &'static str> {
    Ok(Image::open(path)?.exports())
}

/// Every symbol the binary at `path` imports from `dll`, in import-table order.
///
/// `dll` is matched against the name in the import descriptor case-insensitively and as written —
/// which is the only name there is, an import table naming bare file names. So this is asked with
/// the name off the graph's own edge and cannot fail to match for a reason to do with paths.
pub fn imported_from(path: &Path, dll: &str) -> Result<Vec<Symbol>, &'static str> {
    Ok(Image::open(path)?.imported_from(dll))
}

/// A C++ symbol as a person reads it: `rsh::App3D::Exec(struct rsh::Cmd &)` out of
/// `?Exec@App3D@rsh@@QEAAHAEAUCmd@rsh@@@Z`.
///
/// `None` for anything that is not a decorated name, which is every C export — `CreateFileW` is
/// already what it is called, and a demangler asked about it answers `CreateFileW` and costs a
/// syscall to do it. So the test is done here, cheaply: MSVC decorates with a leading `?`.
///
/// # What it is not
///
/// **MSVC's scheme only.** `UnDecorateSymbolName` is the linker's own demangler, which is exactly
/// what is wanted for a Windows DLL and knows nothing about Itanium mangling — so a MinGW or
/// Rust-built module's `_ZN…` names come back `None` and are shown as they are. Adding a second
/// demangler for those means a crate and a decision about which of the two schemes a name is in;
/// what is here answers the question the panel is actually asked, which is about MSVC C++ DLLs.
///
/// # Which flags: the name, not the declaration
///
/// The **qualified name and its parameters, without the decoration around them** — no calling
/// convention, no return type, no `public:`, no `__ptr64`. What comes back is what the symbol is
/// *called*, which is what a name is; the rest is what the compiler added to make it unique, and
/// `dbghelp` will reassemble a whole C++ declaration only because it can.
///
/// The caller that displays these has its own reasons to want it this short, and they are written
/// down where that decision is made — see `crate::ui::deps::symbols::readable`.
pub fn demangle(name: &str) -> Option<String> {
    if !name.starts_with('?') {
        return None;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Diagnostics::Debug::UnDecorateSymbolName;

        /// `UNDNAME_NO_MS_KEYWORDS | _NO_FUNCTION_RETURNS | _NO_ALLOCATION_MODEL |
        /// _NO_ALLOCATION_LANGUAGE | _NO_ACCESS_SPECIFIERS | _NO_THROW_SIGNATURES |
        /// _NO_MEMBER_TYPE`. See the note above; the names are `dbghelp.h`'s.
        const FLAGS: u32 = 0x0002 | 0x0004 | 0x0008 | 0x0010 | 0x0080 | 0x0100 | 0x0200;
        /// Long enough for anything MSVC will emit — a decorated name is capped at 4096, and an
        /// undecorated one is longer than its decoration only for deeply nested templates.
        const ROOM: usize = 8192;

        // **The two buffers are kept, not allocated per name.** A picked module hands this every name
        // in its export table — seventeen hundred for a system DLL, tens of thousands for a large C++
        // one — and a fresh 8 KB zeroed for each of those is fourteen megabytes of memset on one
        // click. Thread-local rather than a parameter so that the function stays one call.
        thread_local! {
            static SCRATCH: std::cell::RefCell<(Vec<u8>, Vec<u8>)> =
                const { std::cell::RefCell::new((Vec::new(), Vec::new())) };
        }
        SCRATCH.with_borrow_mut(|(decorated, out)| {
            // NUL-terminated, because the API takes a C string and a symbol name is ASCII.
            decorated.clear();
            decorated.extend_from_slice(name.as_bytes());
            decorated.push(0);
            out.clear();
            out.resize(ROOM, 0);
            // SAFETY: both buffers outlive the call. `decorated` is NUL-terminated, and `out`'s
            // length is passed as the count of bytes the function may write, which is what it
            // documents.
            let written = unsafe {
                UnDecorateSymbolName(decorated.as_ptr(), out.as_mut_ptr(), ROOM as u32, FLAGS)
            } as usize;
            // Zero is failure, and so is a name that came back as itself: `dbghelp` hands back the
            // decorated name unchanged for something it does not recognise, and showing that as a
            // demangling would be a lie about having understood it.
            let text = String::from_utf8_lossy(&out[..written.min(ROOM)]).into_owned();
            (written > 0 && text != name && !text.is_empty()).then_some(text)
        })
    }
    #[cfg(not(windows))]
    None
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

/// The three data directories this reads.
const DIR_EXPORT: usize = 0;
const DIR_IMPORT: usize = 1;
const DIR_DELAY: usize = 13;

/// A cap on the descriptor arrays, so a damaged or hostile file cannot loop.
const MAX_DESCRIPTORS: u64 = 4096;

/// How many symbols one table will report.
///
/// A large C++ DLL exports tens of thousands of mangled names, and each one costs a name to read.
/// Sixteen thousand is more than anyone reads down a panel and a few milliseconds of warm reads.
///
/// **Public because a list this long has been cut off**, and the panel showing it says so — a listing
/// missing rows that does not admit it is the rule this program holds itself to everywhere else. A
/// caller compares the length it got against this.
pub const MAX_SYMBOLS: usize = 16_384;

/// How much of the file a name lookup pulls in, and the longest name it will believe.
///
/// **A table of names costs one read and not one per name**, which is the whole reason the window
/// exists: the names an export or import table points at are packed together in one region and
/// walked in order, so 16 KB of them is one syscall rather than four thousand. Reading them one at a
/// time was measured at about 3 µs each — forty milliseconds for a big DLL's export table, which is
/// a dropped frame on the click that asked for it.
///
/// 4096 is MSVC's own limit on a decorated name, and it has to be well under the window: a name is
/// only read out of what the window already holds, so a window that could end in the middle of one
/// would cut it in half.
const WINDOW: usize = 16 * 1024;
const LONGEST: usize = 4096;

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
    /// How wide one entry of a thunk array is: four bytes in a PE32 image and eight in a PE32+ one,
    /// which is also where its "this is an ordinal" bit lives.
    thunk: usize,
    sections: Vec<Section>,
    dirs: Vec<(u32, u32)>,
    /// The last block read for a name, and where in the file it came from. See [`WINDOW`].
    window: Option<(u64, Vec<u8>)>,
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
        let (base, dirs_at, thunk) = match magic {
            0x10b => (u32_at(&optional, 28).ok_or("damaged headers")? as u64, 96, 4),
            0x20b => (u64_at(&optional, 24).ok_or("damaged headers")?, 112, 8),
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
            thunk,
            sections,
            dirs,
            window: None,
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

    /// Up to `len` bytes at an address, cut to what is actually behind it.
    ///
    /// For the tables whose length is *declared* by the file: an export table claiming forty
    /// thousand entries in a 30 KB DLL is a damaged or hostile file, and asking for the whole
    /// declared length as one exact read would come back with nothing at all rather than with the
    /// part that is there.
    fn upto_rva(&mut self, rva: u64, len: usize) -> Option<Vec<u8>> {
        let offset = self.offset(u32::try_from(rva).ok()?)?;
        let len = self.len.saturating_sub(offset).min(len as u64) as usize;
        at(&mut self.file, offset, len)
    }

    /// The file from `offset` on, out of one block kept between calls. See [`WINDOW`].
    ///
    /// A miss re-reads from exactly `offset`, so whatever is at the front of what comes back has a
    /// whole window behind it — which is what makes it safe for [`name_at`] to believe a name it
    /// finds there and to give up on one it does not.
    fn window(&mut self, offset: u64) -> Option<&[u8]> {
        let hit = match &self.window {
            Some((from, bytes)) if offset >= *from => {
                let into = (offset - *from) as usize;
                let held = bytes.len().saturating_sub(into);
                // Room for the longest name, or the end of the file — past which there is nothing
                // more to read and the window already holds all of it.
                (held > 0 && (held >= LONGEST || *from + bytes.len() as u64 >= self.len))
                    .then_some(into)
            }
            _ => None,
        };
        let into = match hit {
            Some(into) => into,
            None => {
                let len = self.len.saturating_sub(offset).min(WINDOW as u64) as usize;
                self.window = Some((offset, at(&mut self.file, offset, len)?));
                0
            }
        };
        self.window.as_ref().map(|(_, bytes)| &bytes[into..])
    }

    /// The file from an *address* on, out of the window. Where both readers below start.
    fn block_at(&mut self, rva: u64) -> Option<&[u8]> {
        let offset = self.offset(u32::try_from(rva).ok()?)?;
        self.window(offset)
    }

    /// The NUL-terminated name at an address. DLL and symbol names are ASCII by specification.
    fn name_at(&mut self, rva: u64) -> Option<String> {
        sane_name(self.block_at(rva)?)
    }

    /// An `IMAGE_IMPORT_BY_NAME`, which is the hint and the name in that order — out of one window,
    /// because the two are adjacent and the pair is read once per imported symbol.
    fn by_name(&mut self, rva: u64) -> (Option<u16>, Option<String>) {
        let Some(block) = self.block_at(rva) else {
            return (None, None);
        };
        (u16_at(block, 0), block.get(2..).and_then(sane_name))
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
        for entry in self.descriptors() {
            note(&mut out, &mut seen, entry.dll, entry.delayed);
        }
        out
    }

    /// Every entry of **both** import tables: which DLL, whether the reference is delay-loaded, and
    /// where the names it imports are listed.
    ///
    /// One walk for the two callers — the names alone for [`imports`](Self::imports), the thunk table
    /// as well for [`imported_from`](Self::imported_from) — because the delay-load wrinkle below is
    /// the one piece of arcana in this file that must not exist twice. It was written out in both, and
    /// a rule whose own comment says that getting it wrong "produces a plausible-looking name that is
    /// not there" is a rule you fix in one place and leave wrong in the other.
    fn descriptors(&mut self) -> Vec<Descriptor> {
        let mut out = Vec::new();

        // `IMAGE_IMPORT_DESCRIPTOR`: five words — the first is the name table, the fourth the DLL's
        // own name, and an all-zero one ends the array.
        //
        // **`OriginalFirstThunk` when there is one.** The two thunk arrays start out identical and the
        // loader overwrites the second with addresses, so a bound image still has its names — in the
        // first array only. `FirstThunk` is the fallback for images old enough not to have both.
        if let Some((rva, _)) = self.dir(DIR_IMPORT) {
            for step in 0..MAX_DESCRIPTORS {
                let Some(entry) = self.at_rva(rva as u64 + step * 20, 20) else {
                    break;
                };
                let (Some(lookup), Some(name_rva), Some(bound)) =
                    (u32_at(&entry, 0), u32_at(&entry, 12), u32_at(&entry, 16))
                else {
                    break;
                };
                if name_rva == 0 {
                    break;
                }
                if let Some(dll) = self.name_at(name_rva as u64) {
                    out.push(Descriptor {
                        dll,
                        delayed: false,
                        names: Some(if lookup != 0 { lookup as u64 } else { bound as u64 }),
                    });
                }
            }
        }

        // `IMAGE_DELAYLOAD_DESCRIPTOR`: eight words, the second of which is the name and the fifth
        // its name table.
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
                let (Some(attributes), Some(field), Some(names)) =
                    (u32_at(&entry, 0), u32_at(&entry, 4), u32_at(&entry, 16))
                else {
                    break;
                };
                if field == 0 {
                    break;
                }
                let base = self.base;
                let rebase = move |value: u32| -> Option<u64> {
                    if attributes & 1 != 0 {
                        Some(value as u64)
                    } else {
                        (value as u64).checked_sub(base)
                    }
                };
                let Some(name_rva) = rebase(field) else {
                    continue;
                };
                if let Some(dll) = self.name_at(name_rva) {
                    out.push(Descriptor {
                        dll,
                        delayed: true,
                        names: rebase(names),
                    });
                }
            }
        }

        out
    }

    /// Everything the export directory says, in ordinal order.
    ///
    /// The one piece of it worth knowing before reading the code: **the table is two lists, not
    /// one**. There is an array of addresses indexed by ordinal, and beside it a *sorted* array of
    /// names with a parallel array saying which ordinal each name belongs to — so a name cannot be
    /// paired with its address by position, and an export can have an address and no name at all.
    fn exports(&mut self) -> Vec<Export> {
        let Some((dir, size)) = self.dir(DIR_EXPORT) else {
            return Vec::new();
        };
        // `IMAGE_EXPORT_DIRECTORY`: ten words, of which six matter here.
        let Some(head) = self.at_rva(dir as u64, 40) else {
            return Vec::new();
        };
        // The ordinal the first slot of the address array is exported at. Almost always 1, and a
        // file is free to say otherwise.
        let base = u32_at(&head, 16).unwrap_or(1);
        let slots = u32_at(&head, 20).unwrap_or(0) as usize;
        let named = u32_at(&head, 24).unwrap_or(0) as usize;
        let (Some(at_slots), Some(at_names), Some(at_ordinals)) =
            (u32_at(&head, 28), u32_at(&head, 32), u32_at(&head, 36))
        else {
            return Vec::new();
        };
        let slots = slots.min(MAX_SYMBOLS);
        let named = named.min(MAX_SYMBOLS);

        // The three arrays whole, since each is one contiguous run of fixed-width entries — and
        // `upto_rva`, because their lengths are the file's claim about itself rather than a fact.
        let addresses = self.upto_rva(at_slots as u64, slots * 4).unwrap_or_default();
        let name_rvas = self.upto_rva(at_names as u64, named * 4).unwrap_or_default();
        let ordinals = self.upto_rva(at_ordinals as u64, named * 2).unwrap_or_default();

        // Which name belongs to which slot, built the only way round it can be. The names are read
        // here rather than in the loop below so that they come off the file in table order, which
        // is what keeps them inside one [`WINDOW`].
        let mut names: Vec<Option<String>> = vec![None; slots];
        for i in 0..named {
            let (Some(rva), Some(slot)) = (u32_at(&name_rvas, i * 4), u16_at(&ordinals, i * 2))
            else {
                break;
            };
            let Some(name) = self.name_at(rva as u64) else {
                continue;
            };
            if let Some(slot) = names.get_mut(slot as usize) {
                *slot = Some(name);
            }
        }

        let end = dir as u64 + size as u64;
        let mut out = Vec::with_capacity(slots);
        for slot in 0..slots {
            let Some(address) = u32_at(&addresses, slot * 4) else {
                break;
            };
            // A hole in the ordinal range: exported at no address and under no name. A DLL that has
            // had exports removed over the years is full of these, and they are not symbols.
            if address == 0 {
                continue;
            }
            // **An address inside the export directory is not code.** It is the `DLL.Function`
            // string this export is really a redirection to — which is what most of `kernel32`'s
            // surface has been since Windows 7, and reading it as an entry point would report an
            // address in the middle of a table of strings.
            let forwarder = (address as u64) >= dir as u64 && (address as u64) < end;
            out.push(Export {
                name: names.get_mut(slot).and_then(Option::take),
                ordinal: u16::try_from(slot as u64 + base as u64).unwrap_or(u16::MAX),
                bound: match forwarder.then(|| self.name_at(address as u64)).flatten() {
                    Some(to) => Bound::Forward(to),
                    None => Bound::Entry(address),
                },
            });
        }
        out
    }

    /// Every symbol imported from one named DLL, out of both import tables.
    ///
    /// Every matching descriptor and not the first: a linker may write two for one DLL, and stopping
    /// at one would report half of what is imported from it.
    fn imported_from(&mut self, dll: &str) -> Vec<Symbol> {
        let mut out = Vec::new();
        let wanted: Vec<Descriptor> = self
            .descriptors()
            .into_iter()
            .filter(|entry| entry.dll.eq_ignore_ascii_case(dll))
            .collect();
        for entry in wanted {
            if let Some(names) = entry.names {
                self.thunks(names, entry.delayed, &mut out);
            }
        }
        out
    }

    /// One thunk array read out into symbols. Ends at the first all-zero entry, which is what
    /// terminates it.
    ///
    /// **A block at a time rather than the whole array.** Its length is not declared anywhere — only
    /// the terminator says where it ends — so asking for [`MAX_SYMBOLS`] entries up front was a 128 KB
    /// read and a 128 KB allocation to find a zero forty entries in, which is the usual case. A block
    /// covers a typical table whole and a table of thousands still costs a handful of reads.
    fn thunks(&mut self, table: u64, delayed: bool, out: &mut Vec<Symbol>) {
        /// Entries per read. 512 of them is 4 KB in a 64-bit image, one page-ish, and more than any
        /// ordinary binary imports from one DLL.
        const BLOCK: usize = 512;

        let step = self.thunk;
        // The top bit of an entry says "this is an ordinal, in the low sixteen", and which bit that
        // is depends on how wide the entry is.
        let ordinal = 1u64 << (step * 8 - 1);
        for block in 0..MAX_SYMBOLS / BLOCK {
            let at = table + (block * BLOCK * step) as u64;
            let Some(entries) = self.upto_rva(at, BLOCK * step) else {
                return;
            };
            let mut read = 0;
            for slot in entries.chunks_exact(step) {
                let value = match step {
                    8 => u64_at(slot, 0),
                    _ => u32_at(slot, 0).map(u64::from),
                };
                // The terminator, or the end of what could be read: either way this table is done.
                let Some(value) = value.filter(|&value| value != 0) else {
                    return;
                };
                read += 1;
                if value & ordinal != 0 {
                    out.push(Symbol {
                        name: None,
                        ordinal: Some(value as u16),
                        hint: None,
                        delayed,
                    });
                    continue;
                }
                let (hint, name) = self.by_name(value);
                out.push(Symbol {
                    name,
                    ordinal: None,
                    hint,
                    delayed,
                });
            }
            // Short of a full block means the file ran out before the terminator did.
            if read < BLOCK {
                return;
            }
        }
    }
}

/// One entry of an import table: which DLL, how it is loaded, and where its symbols are named.
struct Descriptor {
    dll: String,
    delayed: bool,
    /// The thunk array that lists the symbols imported from it, as an address in the image. `None`
    /// for a delay-load descriptor whose addresses could not be rebased.
    names: Option<u64>,
}

/// Whether a module is one the loader would not find: the question both chain walks ask.
fn is_absent(module: &Module) -> bool {
    matches!(module.state, State::Missing | State::Unreadable(_))
}

/// The NUL-terminated name at the front of a block, if it is one.
///
/// A name with anything unprintable in it is a sign the address was wrong rather than a symbol with
/// an unusual character in it, and following it would put nonsense in the panel. So is a run with no
/// terminator inside a whole [`WINDOW`] — nothing in a PE file names anything that long.
fn sane_name(block: &[u8]) -> Option<String> {
    let end = block.iter().position(|&b| b == 0)?;
    let name = &block[..end];
    (!name.is_empty()
        && name.len() <= LONGEST
        && name.iter().all(|&b| b.is_ascii_graphic() || b == b' '))
    .then(|| String::from_utf8_lossy(name).into_owned())
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
mod tests;
