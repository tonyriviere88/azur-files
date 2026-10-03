1//! What a binary needs in order to run, followed down: the Dependency Walker question.
2//!
3//! A Windows executable names the DLLs it needs in its **import table**, by bare file name and
4//! nothing else — `KERNEL32.dll`, not a path. Which file that turns out to be is the loader's
5//! answer, not the binary's, and it is where the interesting failures live: the DLL that is
6//! missing, the one that came off a different search directory than you thought, the 32-bit one
7//! next to a 64-bit program. So this module does two separable things:
8//!
9//! 1. **Read the import table.** [`read`] opens a file, walks its headers and hands back the
10//!    machine it was built for and the names it imports. Pure parsing, no policy.
11//! 2. **Resolve those names and repeat.** [`walk`] follows the graph from one root binary,
12//!    resolving each name against a search path, and comes back with every module reachable
13//!    from it and where each one was found.
14//!
15//! # What is read, and what is not
16//!
17//! Only the headers and the import descriptors, never the whole file. A dependency walk of a
18//! large application touches a couple of hundred binaries, and `Qt6Core.dll` alone is 6 MB —
19//! reading them whole would be most of a gigabyte off the disk to find a few hundred strings.
20//! Each module costs four small reads for its headers plus two per import, all of which land in
21//! the page cache for the system DLLs that every walk visits.
22//!
23//! # What the search path is, and what it is not
24//!
25//! [`search_paths`]: **the folder the root binary is in, and then `PATH`.** That is the part of
26//! the loader's rule that is worth having and cheap to be sure about, and on an ordinary machine
27//! it resolves everything, since `System32` is in `PATH`.
28//!
29//! It is deliberately not the whole rule, and the differences are worth knowing before believing
30//! a location this reports:
31//!
32//! - **`KnownDLLs` wins over everything.** `kernel32`, `ole32`, `user32` and about thirty others
33//!   are resolved from a section the session manager opened at boot, so a copy of one sitting
34//!   next to your program is ignored — where this would report the copy.
35//! - **Side-by-side assemblies** (`WinSxS`) are resolved from a manifest, which is why two
36//!   programs on one machine can load different `MSVCR90.dll`s.
37//! - **API sets** are not files. See [`is_api_set`].
38//! - **`SetDllDirectory`, `LOAD_WITH_ALTERED_SEARCH_PATH`, manifest `<file>` redirection** and a
39//!   `.local` folder all move the goalposts at run time, and nothing on disk can predict them.
40//! - The application directory is the **root's** folder for every level of the walk, not each
41//!   DLL's own folder, which is what the loader does: the "application directory" belongs to the
42//!   process, so a DLL in `C:\lib` loaded by a program in `C:\app` looks for its own imports in
43//!   `C:\app`, not in `C:\lib`.
44//!
45//! # Why the walk is parallel
46//!
47//! For the same reason [`crate::fs::scan::scan_deep`] is: the work is a syscall waiting on the
48//! file system, so a thread that is waiting is a module another thread could have been reading.
49//! A level of the graph is resolved on up to [`hands`] threads at once, level by level, which is
50//! what keeps a walk of a few hundred modules inside a tenth of a second warm.
51
52use std::collections::{HashMap, HashSet};
53use std::fs::File;
54use std::io::{Read, Seek, SeekFrom};
55use std::path::{Path, PathBuf};
56use std::sync::atomic::{AtomicUsize, Ordering};
57use std::time::{Duration, Instant};
58
59// ---------------------------------------------------------------------------
60// The graph
61// ---------------------------------------------------------------------------
62
63/// One binary in a dependency graph.
64pub struct Module {
65    /// The name it was imported by — which is a bare file name, and the only thing the
66    /// importing binary actually said. The root's own file name, for the root.
67    pub name: String,
68    /// Where it was found, when it was. `None` for anything [`State`] says is not a file.
69    pub path: Option<PathBuf>,
70    /// The processor it was built for, as an `IMAGE_FILE_MACHINE_*` value. `0` when unknown,
71    /// which is both "not read" and the real value of a processor-neutral image.
72    pub machine: u16,
73    pub state: State,
74    /// What it imports, in import-table order.
75    pub imports: Vec<Edge>,
76}
77
78/// One import: which module, and whether it is loaded up front or on first use.
79#[derive(Clone, Copy, PartialEq, Eq, Debug)]
80pub struct Edge {
81    pub to: usize,
82    /// A **delay-load** import: the linker generated a stub, and the DLL is not opened until
83    /// something calls into it. Worth telling apart from an ordinary import, because a missing
84    /// delay-loaded DLL does not stop the program from starting — it stops one feature from
85    /// working, later, somewhere else.
86    pub delayed: bool,
87}
88
89/// How looking for a module turned out.
90#[derive(Clone, Copy, PartialEq, Eq, Debug)]
91pub enum State {
92    /// Found on disk and read.
93    Found,
94    /// An API set: a name the loader resolves through a schema rather than a file. Not looked
95    /// for, and not followed. See [`is_api_set`].
96    ApiSet,
97    /// Nothing in the search path is called this.
98    Missing,
99    /// There is a file, and it is not one this can read. The reason, in a few words.
100    Unreadable(&'static str),
101    /// Discovered, but the walk stopped before getting to it — see [`Graph::truncated`].
102    Unvisited,
103}
104
105/// Everything reachable from one binary. The root is index 0.
106pub struct Graph {
107    pub modules: Vec<Module>,
108    /// Where every name was looked for, in the order it was looked for in. Shown, because a
109    /// location is only meaningful next to the list of places that were tried.
110    pub search: Vec<PathBuf>,
111    /// The walk stopped at [`BUDGET`] or [`PATIENCE`] rather than at the end of the graph, and
112    /// what is here is therefore incomplete. Said out loud by the panel: a dependency list
113    /// missing entries and not admitting it is worse than no list at all.
114    pub truncated: bool,
115    /// How long it took. The only honest way to claim this is fast.
116    pub micros: u64,
117}
118
119impl Graph {
120    /// The binary the walk started from, which is always module 0.
121    pub fn root(&self) -> &Module {
122        &self.modules[0]
123    }
124
125    /// The graph in three numbers: files, API sets, and modules that are neither.
126    ///
127    /// Three rather than one because "937 modules" is a misleading total. Most of a modern
128    /// graph is [`is_api_set`] names — 688 of those 937, walking this program's own binary —
129    /// and they are not files, cannot be missing, and are not what somebody counting
130    /// dependencies means. The number worth reading is the first one.
131    pub fn tally(&self) -> (usize, usize, usize) {
132        let mut files = 0;
133        let mut api_sets = 0;
134        let mut missing = 0;
135        for module in &self.modules {
136            match module.state {
137                State::Found => files += 1,
138                State::ApiSet => api_sets += 1,
139                State::Missing | State::Unreadable(_) => missing += 1,
140                State::Unvisited => {}
141            }
142        }
143        (files, api_sets, missing)
144    }
145
146    /// Every module that would stop the root from starting: not there, and reached by imports
147    /// that are all loaded up front.
148    ///
149    /// The distinction is the whole reason [`Edge::delayed`] is recorded. Walking this program's
150    /// own binary finds five missing DLLs on this machine — `HvsiFileTrust`, `AzureAttestManager`
151    /// and three more — every one of them delay-loaded from somewhere deep inside `shell32`, and
152    /// every one of them harmless: Windows ships the stubs for features that are not installed.
153    /// Treating those the same as a missing `MSVCP140.dll` beside the executable is what made
154    /// dependency lists into something people learned to ignore.
155    ///
156    /// Returns, for each one, the chain of modules from the root down to it, so the panel can
157    /// open exactly the branches worth looking at and nothing else.
158    pub fn breaks_loading(&self) -> Vec<Vec<usize>> {
159        // Breadth-first from the root along non-delayed imports only, remembering how each
160        // module was first reached — which gives the shortest such chain, and a simple one.
161        let mut parent = vec![usize::MAX; self.modules.len()];
162        let mut reached = vec![false; self.modules.len()];
163        let mut queue = std::collections::VecDeque::from([0usize]);
164        reached[0] = true;
165        while let Some(at) = queue.pop_front() {
166            for edge in &self.modules[at].imports {
167                if edge.delayed || reached[edge.to] {
168                    continue;
169                }
170                reached[edge.to] = true;
171                parent[edge.to] = at;
172                queue.push_back(edge.to);
173            }
174        }
175
176        let mut out = Vec::new();
177        for (i, module) in self.modules.iter().enumerate() {
178            if !reached[i] || !matches!(module.state, State::Missing | State::Unreadable(_)) {
179                continue;
180            }
181            let mut chain = vec![i];
182            let mut at = parent[i];
183            while at != usize::MAX {
184                chain.push(at);
185                at = parent[at];
186            }
187            chain.reverse();
188            out.push(chain);
189        }
190        out
191    }
192
193    /// Whether module `i` was built for a different processor than the root.
194    ///
195    /// Windows will not load it, which makes this the second most useful thing a dependency
196    /// walk can tell you after "it is not there". A machine of `0` is not a mismatch — that is
197    /// a processor-neutral image, which is a legitimate answer rather than a missing one.
198    pub fn foreign(&self, i: usize) -> bool {
199        let root = self.modules[0].machine;
200        let mine = self.modules[i].machine;
201        root != 0 && mine != 0 && root != mine
202    }
203}
204
205/// The name of an `IMAGE_FILE_MACHINE_*` value, as people write it.
206pub fn machine_name(machine: u16) -> &'static str {
207    match machine {
208        0x014c => "x86",
209        0x8664 => "x64",
210        0xaa64 => "arm64",
211        // ARM64EC and ARM64X: x64 code in an arm64 image, and both in one image. They load
212        // alongside arm64, so they are not a mismatch, and they are not the same thing either.
213        0xa641 => "arm64ec",
214        0xa64e => "arm64x",
215        0x01c0 | 0x01c2 | 0x01c4 => "arm",
216        0x0200 => "ia64",
217        0x0000 => "neutral",
218        _ => "?",
219    }
220}
221
222/// Whether a file is worth offering a dependency walk for, from its extension alone.
223///
224/// Everything Windows loads as an executable image and a handful of things that are DLLs under
225/// another name — a Python extension, a Node addon, a DirectShow filter. Cheap to ask, so it is
226/// what decides whether the shortcut does anything, and a file that passes this and turns out
227/// not to be a PE says so in the panel rather than being refused here.
228pub fn is_image(ext: &str) -> bool {
229    const IMAGES: [&str; 13] = [
230        "exe", "dll", "ocx", "sys", "cpl", "drv", "scr", "efi", "node", "pyd", "ax", "mun",
231        "mui",
232    ];
233    IMAGES.iter().any(|known| ext.eq_ignore_ascii_case(known))
234}
235
236/// Whether a name is an **API set** rather than a file.
237///
238/// `api-ms-win-core-file-l1-2-0.dll` and its several hundred siblings are not DLLs. They are
239/// names in a schema the loader carries, mapped at load time onto whichever real DLL implements
240/// that contract on this build of Windows — usually `kernelbase.dll` or `ntdll.dll`. Some builds
241/// do ship stub files of the same name and some do not, which is exactly why looking on disk is
242/// the wrong question to ask about them.
243///
244/// This is the single biggest difference between a useful dependency list and the wall of red
245/// that made the original Dependency Walker useless on anything built after Windows 7 — it
246/// reported every one of these as missing. They are marked for what they are, and not followed:
247/// their own imports are `ntdll`, which every module in the graph already has.
248pub fn is_api_set(name: &str) -> bool {
249    let name = name.as_bytes();
250    let starts = |prefix: &[u8]| {
251        name.len() > prefix.len()
252            && name[..prefix.len()].eq_ignore_ascii_case(prefix)
253    };
254    starts(b"api-ms-") || starts(b"ext-ms-")
255}
256
257// ---------------------------------------------------------------------------
258// The walk
259// ---------------------------------------------------------------------------
260
261/// How many modules a walk may discover.
262///
263/// Far above anything real — a large Qt application reaches about two hundred — because the
264/// bound that actually bites is [`PATIENCE`]. This one is here so that a file crafted to import
265/// itself under a thousand names costs a bounded amount rather than the machine's memory.
266pub const BUDGET: usize = 4096;
267
268/// How long a walk may take before it hands back what it has.
269///
270/// A warm walk of a real application is a few milliseconds; a cold one off a network share is
271/// tens of milliseconds per module. Five seconds is long enough that nothing ordinary hits it
272/// and short enough that a `PATH` entry pointing at a share that has gone away costs one wait
273/// rather than a stuck panel.
274pub const PATIENCE: Duration = Duration::from_secs(5);
275
276/// Follow everything `root` needs, and everything those need.
277///
278/// The root is module 0 whatever happens to it — a file that is not a binary at all still comes
279/// back as a graph of one module whose [`State`] says why, which is what lets the panel show the
280/// answer in the same place it shows every other answer.
281pub fn walk(root: &Path, budget: usize, patience: Duration) -> Graph {
282    let started = Instant::now();
283    let deadline = started + patience;
284    let search = search_paths(root);
285    let hands = hands();
286
287    let name = root
288        .file_name()
289        .map(|n| n.to_string_lossy().into_owned())
290        .unwrap_or_else(|| root.to_string_lossy().into_owned());
291
292    // The root is read here rather than through `probe`: it is named by a path and not by a
293    // name to be resolved, which is the one module in the graph that is not a search result.
294    let mut modules = Vec::new();
295    let mut raw: Vec<Vec<(String, bool)>> = Vec::new();
296    match read(root) {
297        Ok(image) => {
298            modules.push(Module {
299                name,
300                path: Some(root.to_path_buf()),
301                machine: image.machine,
302                state: State::Found,
303                imports: Vec::new(),
304            });
305            raw.push(image.imports);
306        }
307        Err(why) => {
308            modules.push(Module {
309                name,
310                path: Some(root.to_path_buf()),
311                machine: 0,
312                state: State::Unreadable(why),
313                imports: Vec::new(),
314            });
315            raw.push(Vec::new());
316        }
317    }
318
319    // Which module a name belongs to. Keyed by the *name*, lowercased, because resolution is a
320    // function of the name alone: two references to `kernel32.dll` from two different modules
321    // cannot come out as two different files, so one entry per name is exactly right — and it
322    // is also what makes the graph finite when it has a cycle in it, which real ones do.
323    let mut index: HashMap<String, usize> = HashMap::new();
324    let mut truncated = false;
325    // The modules whose imports have been read but whose edges have not been built yet.
326    let mut level = vec![0usize];
327
328    while !level.is_empty() && !truncated {
329        // Every name this level imports, with the modules that want it. New names get their
330        // index here, before anything is read, so the edges can be attached in one pass.
331        let mut fresh: Vec<usize> = Vec::new();
332        for &from in &level {
333            let wants = std::mem::take(&mut raw[from]);
334            for (name, delayed) in wants {
335                let key = name.to_ascii_lowercase();
336                let to = match index.get(&key) {
337                    Some(&known) => known,
338                    None => {
339                        if modules.len() >= budget {
340                            truncated = true;
341                            break;
342                        }
343                        let at = modules.len();
344                        index.insert(key, at);
345                        modules.push(Module {
346                            name,
347                            path: None,
348                            machine: 0,
349                            state: State::Unvisited,
350                            imports: Vec::new(),
351                        });
352                        raw.push(Vec::new());
353                        fresh.push(at);
354                        at
355                    }
356                };
357                modules[from].imports.push(Edge { to, delayed });
358            }
359            if truncated {
360                break;
361            }
362        }
363
364        if fresh.is_empty() {
365            break;
366        }
367        let names: Vec<String> = fresh.iter().map(|&at| modules[at].name.clone()).collect();
368        let (probed, out_of_time) = probe_all(&names, &search, deadline, hands);
369        truncated |= out_of_time;
370        for (&at, found) in fresh.iter().zip(probed) {
371            modules[at].path = found.path;
372            modules[at].machine = found.machine;
373            modules[at].state = found.state;
374            raw[at] = found.imports;
375        }
376        level = fresh;
377    }
378
379    Graph {
380        modules,
381        search,
382        truncated,
383        micros: started.elapsed().as_micros() as u64,
384    }
385}
386
387/// Where a name is looked for, in order: the root binary's folder, then `PATH`.
388///
389/// Entries that are not directories are dropped **here, once**, rather than being tried for
390/// every module. That is not tidiness: a `PATH` entry pointing at a share that has gone away
391/// costs a connection timeout every time something is looked for in it, so a walk of two hundred
392/// modules against a dead entry would be two hundred timeouts instead of the one this spends.
393pub fn search_paths(root: &Path) -> Vec<PathBuf> {
394    let mut out: Vec<PathBuf> = Vec::new();
395    let mut seen: HashSet<String> = HashSet::new();
396    let mut add = |dir: PathBuf| {
397        // Case-insensitively, because `C:\Windows\System32` and `c:\windows\system32` are one
398        // directory and appear in `PATH` both ways round on a real machine.
399        let key = dir.to_string_lossy().to_ascii_lowercase();
400        if key.is_empty() || !seen.insert(key) {
401            return;
402        }
403        if dir.is_dir() {
404            out.push(dir);
405        }
406    };
407
408    if let Some(folder) = root.parent() {
409        add(folder.to_path_buf());
410    }
411    if let Some(path) = std::env::var_os("PATH") {
412        // `split_paths` rather than splitting on `;`, so a quoted entry containing a separator
413        // survives and the same code is right on the platform where the separator is `:`.
414        for dir in std::env::split_paths(&path) {
415            add(dir);
416        }
417    }
418    out
419}
420
421/// One name, looked for and read.
422struct Probe {
423    path: Option<PathBuf>,
424    machine: u16,
425    state: State,
426    imports: Vec<(String, bool)>,
427}
428
429/// Resolve and read a level of the graph, on up to `hands` threads.
430///
431/// In the order given, so the display order is the import order and not the order the threads
432/// happened to finish in. Each worker takes the next name nobody has claimed rather than a fixed
433/// share of them, because modules differ enormously in how long they take — one name off a
434/// network share would otherwise leave every other thread idle behind it.
435///
436/// **A level no larger than the thread count goes inline.** Spawning a thread costs about as
437/// much as reading a warm binary's headers, so a level of three does not pay for three threads;
438/// the rule asks for two names each before it is worth it. The same rule, for the same measured
439/// reason, as [`crate::fs::scan`]'s.
440///
441/// The second half of the answer is whether the deadline stopped it, because the caller cannot
442/// tell that from a complete-looking list of modules that are all marked unvisited.
443fn probe_all(
444    names: &[String],
445    search: &[PathBuf],
446    until: Instant,
447    hands: usize,
448) -> (Vec<Probe>, bool) {
449    let out_of_time = || Instant::now() >= until;
450
451    if names.len() <= hands || hands <= 1 {
452        let mut out = Vec::with_capacity(names.len());
453        let mut gave_up = false;
454        for name in names {
455            if out_of_time() {
456                gave_up = true;
457            }
458            out.push(if gave_up {
459                unvisited()
460            } else {
461                probe(name, search)
462            });
463        }
464        return (out, gave_up);
465    }
466
467    let next = AtomicUsize::new(0);
468    let parts: Vec<Vec<(usize, Probe)>> = std::thread::scope(|scope| {
469        let workers: Vec<_> = (0..hands.min(names.len()))
470            .map(|_| {
471                let next = &next;
472                scope.spawn(move || {
473                    // Per-thread, and these threads are new: a `PATH` entry on an empty card
474                    // reader would otherwise raise "Please insert a disk" from inside the
475                    // syscall, on a thread with no window to put it in front of.
476                    crate::fs::scan::silence_device_dialogs();
477                    let mut mine = Vec::new();
478                    loop {
479                        let at = next.fetch_add(1, Ordering::Relaxed);
480                        let Some(name) = names.get(at) else {
481                            return mine;
482                        };
483                        if out_of_time() {
484                            return mine;
485                        }
486                        mine.push((at, probe(name, search)));
487                    }
488                })
489            })
490            .collect();
491        // A worker that panicked contributes nothing rather than taking the window with it.
492        workers.into_iter().filter_map(|w| w.join().ok()).collect()
493    });
494
495    let mut done: Vec<Option<Probe>> = (0..names.len()).map(|_| None).collect();
496    let mut answered = 0;
497    for (at, found) in parts.into_iter().flatten() {
498        if let Some(slot) = done.get_mut(at) {
499            if slot.is_none() {
500                answered += 1;
501            }
502            *slot = Some(found);
503        }
504    }
505    let gave_up = answered < names.len();
506    (
507        done.into_iter().map(|p| p.unwrap_or_else(unvisited)).collect(),
508        gave_up,
509    )
510}
511
512fn unvisited() -> Probe {
513    Probe {
514        path: None,
515        machine: 0,
516        state: State::Unvisited,
517        imports: Vec::new(),
518    }
519}
520
521/// Look one name up and read what it turns out to be.
522fn probe(name: &str, search: &[PathBuf]) -> Probe {
523    if is_api_set(name) {
524        return Probe {
525            path: None,
526            machine: 0,
527            state: State::ApiSet,
528            imports: Vec::new(),
529        };
530    }
531    let Some(path) = resolve(name, search) else {
532        return Probe {
533            path: None,
534            machine: 0,
535            state: State::Missing,
536            imports: Vec::new(),
537        };
538    };
539    match read(&path) {
540        Ok(image) => Probe {
541            path: Some(path),
542            machine: image.machine,
543            state: State::Found,
544            imports: image.imports,
545        },
546        Err(why) => Probe {
547            path: Some(path),
548            machine: 0,
549            state: State::Unreadable(why),
550            imports: Vec::new(),
551        },
552    }
553}
554
555/// The first directory in `search` that has a file of this name.
556fn resolve(name: &str, search: &[PathBuf]) -> Option<PathBuf> {
557    // An import name is a bare file name. Anything with a separator in it is either a
558    // hand-built binary or an attempt to reach out of the search path, and joining it would
559    // do exactly that, so it is refused rather than followed.
560    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains(':') {
561        return None;
562    }
563    search
564        .iter()
565        .map(|dir| dir.join(name))
566        .find(|candidate| candidate.is_file())
567}
568
569/// How many modules are read at once.
570///
571/// The same shape and the same reason as [`crate::fs::scan`]'s: this waits on the file system
572/// rather than on the processor, so past a handful more threads buy nothing and cost context
573/// switches. Eight, because a walk is one burst of a few hundred reads rather than a trickle.
574fn hands() -> usize {
575    std::thread::available_parallelism()
576        .map(|n| n.get().clamp(2, 8))
577        .unwrap_or(2)
578}
579
580// ---------------------------------------------------------------------------
581// The file format
582// ---------------------------------------------------------------------------
583
584/// What a binary says about itself, which is all this needs from it.
585pub struct Imports {
586    pub machine: u16,
587    /// The DLLs it names, in import-table order, each with whether the reference is
588    /// delay-loaded. Deduplicated: a DLL named in both tables appears once, as an ordinary
589    /// import, because that is what decides whether it is opened at start-up.
590    pub imports: Vec<(String, bool)>,
591}
592
593/// The import table of the binary at `path`.
594///
595/// `Err` with something short enough to put in a row for anything that is not a PE image this
596/// can follow, which includes a 16-bit binary, a `.NET`-only assembly with a damaged header, a
597/// text file somebody named `.exe`, and a file that cannot be opened at all.
598pub fn read(path: &Path) -> Result<Imports, &'static str> {
599    let mut image = Image::open(path)?;
600    let imports = image.imports();
601    Ok(Imports {
602        machine: image.machine,
603        imports,
604    })
605}
606
607/// The two data directories this reads.
608const DIR_IMPORT: usize = 1;
609const DIR_DELAY: usize = 13;
610
611/// A cap on the descriptor arrays, so a damaged or hostile file cannot loop.
612const MAX_DESCRIPTORS: u64 = 4096;
613
614/// One section's place in the file and in memory.
615struct Section {
616    va: u32,
617    vsize: u32,
618    raw: u32,
619    rsize: u32,
620}
621
622/// An open binary, and enough of its headers to turn an address into a file offset.
623struct Image {
624    file: File,
625    /// Its length, so a read near the end can be cut to what is there.
626    len: u64,
627    machine: u16,
628    /// Where the image would be mapped, which is what an old-style delay-load descriptor's
629    /// addresses are relative to.
630    base: u64,
631    /// How much of the front of the file is mapped one-to-one, for an address that lands in
632    /// the headers rather than in a section.
633    headers: u64,
634    sections: Vec<Section>,
635    dirs: Vec<(u32, u32)>,
636}
637
638impl Image {
639    fn open(path: &Path) -> Result<Self, &'static str> {
640        let mut file = File::open(path).map_err(|_| "cannot be opened")?;
641        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
642
643        // The DOS stub, which every PE file still carries, and the one field in it that
644        // matters: where the real header is.
645        let dos = at(&mut file, 0, 64).ok_or("not a binary")?;
646        if &dos[..2] != b"MZ" {
647            return Err("not a binary");
648        }
649        let start = u32_at(&dos, 0x3c).ok_or("not a binary")? as u64;
650        // A 16-bit binary points at a `NE` header, and a file that is merely short points at
651        // nothing at all. Both come out here.
652        let coff = at(&mut file, start, 24).ok_or("not a Windows binary")?;
653        if &coff[..4] != b"PE\0\0" {
654            return Err("not a Windows binary");
655        }
656        let machine = u16_at(&coff, 4).ok_or("damaged headers")?;
657        let section_count = u16_at(&coff, 6).ok_or("damaged headers")? as usize;
658        let optional_size = u16_at(&coff, 20).ok_or("damaged headers")? as usize;
659
660        // The optional header, which is only optional for an object file. Its length is
661        // declared rather than fixed, because the number of data directories at the end of it
662        // is a field of its own.
663        let optional_at = start + 24;
664        let optional = at(&mut file, optional_at, optional_size.min(4096))
665            .ok_or("damaged headers")?;
666        let magic = u16_at(&optional, 0).ok_or("damaged headers")?;
667        // 0x10b is PE32 and 0x20b is PE32+, and the only difference that matters here is that
668        // one of them has a 64-bit image base, which moves everything after it by four bytes.
669        let (base, dirs_at) = match magic {
670            0x10b => (u32_at(&optional, 28).ok_or("damaged headers")? as u64, 96),
671            0x20b => (u64_at(&optional, 24).ok_or("damaged headers")?, 112),
672            _ => return Err("not a Windows binary"),
673        };
674        let count = u32_at(&optional, dirs_at - 4).unwrap_or(0).min(16) as usize;
675        let mut dirs = Vec::with_capacity(count);
676        for i in 0..count {
677            let at = dirs_at + i * 8;
678            let (Some(rva), Some(size)) = (u32_at(&optional, at), u32_at(&optional, at + 4)) else {
679                break;
680            };
681            dirs.push((rva, size));
682        }
683
684        // The section table, which is what makes an address in the file findable.
685        let table_at = optional_at + optional_size as u64;
686        let table = at(&mut file, table_at, section_count.min(96) * 40).unwrap_or_default();
687        let mut sections = Vec::with_capacity(table.len() / 40);
688        for chunk in table.chunks_exact(40) {
689            let (Some(vsize), Some(va), Some(rsize), Some(raw)) = (
690                u32_at(chunk, 8),
691                u32_at(chunk, 12),
692                u32_at(chunk, 16),
693                u32_at(chunk, 20),
694            ) else {
695                break;
696            };
697            sections.push(Section {
698                va,
699                vsize,
700                raw,
701                rsize,
702            });
703        }
704
705        Ok(Self {
706            file,
707            len,
708            machine,
709            base,
710            headers: table_at + (sections.len() * 40) as u64,
711            sections,
712            dirs,
713        })
714    }
715
716    /// Where an address lands in the file, if it lands in the file at all.
717    ///
718    /// A section is longer in memory than on disk whenever it has a zero-filled tail, and an
719    /// address in that tail has no bytes behind it — which is a legitimate thing for a data
720    /// directory to point at and has to come back as `None` rather than as a wild offset.
721    fn offset(&self, rva: u32) -> Option<u64> {
722        for section in &self.sections {
723            let span = if section.vsize == 0 {
724                section.rsize
725            } else {
726                section.vsize
727            };
728            let end = section.va as u64 + span as u64;
729            if (rva as u64) >= section.va as u64 && (rva as u64) < end {
730                let into = rva - section.va;
731                if into >= section.rsize {
732                    return None;
733                }
734                return Some(section.raw as u64 + into as u64);
735            }
736        }
737        // Before the first section: the headers themselves are mapped one to one.
738        ((rva as u64) < self.headers).then_some(rva as u64)
739    }
740
741    fn at_rva(&mut self, rva: u64, len: usize) -> Option<Vec<u8>> {
742        let rva = u32::try_from(rva).ok()?;
743        let offset = self.offset(rva)?;
744        at(&mut self.file, offset, len)
745    }
746
747    /// The NUL-terminated name at an address. DLL names are ASCII by specification.
748    fn name_at(&mut self, rva: u64) -> Option<String> {
749        // A block rather than a byte at a time, which is what keeps a module's cost to a couple
750        // of dozen small reads — but no further than the end of the file, or a name that
751        // happens to sit in the last few bytes of it could not be read at all.
752        let offset = self.offset(u32::try_from(rva).ok()?)?;
753        let len = self.len.saturating_sub(offset).min(256) as usize;
754        let block = at(&mut self.file, offset, len)?;
755        let end = block.iter().position(|&b| b == 0).unwrap_or(block.len());
756        let name = String::from_utf8_lossy(&block[..end]).into_owned();
757        // A name with anything unprintable in it is a sign the address was wrong rather than a
758        // DLL with an unusual name, and following it would put nonsense in the panel.
759        let sane = !name.is_empty()
760            && name.len() <= 255
761            && name.bytes().all(|b| b.is_ascii_graphic() || b == b' ');
762        sane.then_some(name)
763    }
764
765    fn dir(&self, which: usize) -> Option<(u32, u32)> {
766        match self.dirs.get(which) {
767            Some(&(rva, size)) if rva != 0 && size != 0 => Some((rva, size)),
768            _ => None,
769        }
770    }
771
772    /// Every DLL named in the two import tables.
773    fn imports(&mut self) -> Vec<(String, bool)> {
774        let mut out: Vec<(String, bool)> = Vec::new();
775        let mut seen: HashMap<String, usize> = HashMap::new();
776
777        // `IMAGE_IMPORT_DESCRIPTOR`: five words, the fourth of which is the name, and an
778        // all-zero one ends the array.
779        if let Some((rva, _)) = self.dir(DIR_IMPORT) {
780            for step in 0..MAX_DESCRIPTORS {
781                let Some(entry) = self.at_rva(rva as u64 + step * 20, 20) else {
782                    break;
783                };
784                let Some(name_rva) = u32_at(&entry, 12) else {
785                    break;
786                };
787                if name_rva == 0 {
788                    break;
789                }
790                if let Some(name) = self.name_at(name_rva as u64) {
791                    note(&mut out, &mut seen, name, false);
792                }
793            }
794        }
795
796        // `IMAGE_DELAYLOAD_DESCRIPTOR`: eight words, the second of which is the name.
797        //
798        // With a wrinkle worth writing down: the addresses in it are relative to the image
799        // base only when the low bit of the attributes says so. Descriptors written by the
800        // linkers of the late nineties hold absolute virtual addresses instead, and reading
801        // one of those as an RVA lands somewhere arbitrary in the file — which is exactly the
802        // kind of thing that produces a plausible-looking name that is not there.
803        if let Some((rva, _)) = self.dir(DIR_DELAY) {
804            for step in 0..MAX_DESCRIPTORS {
805                let Some(entry) = self.at_rva(rva as u64 + step * 32, 32) else {
806                    break;
807                };
808                let (Some(attributes), Some(field)) = (u32_at(&entry, 0), u32_at(&entry, 4))
809                else {
810                    break;
811                };
812                if field == 0 {
813                    break;
814                }
815                let name_rva = if attributes & 1 != 0 {
816                    Some(field as u64)
817                } else {
818                    (field as u64).checked_sub(self.base)
819                };
820                if let Some(name) = name_rva.and_then(|rva| self.name_at(rva)) {
821                    note(&mut out, &mut seen, name, true);
822                }
823            }
824        }
825
826        out
827    }
828}
829
830/// Add a name unless it is already there, in which case an ordinary import wins over a
831/// delay-loaded one — a DLL named in both tables is opened at start-up, which is the fact worth
832/// showing.
833fn note(
834    out: &mut Vec<(String, bool)>,
835    seen: &mut HashMap<String, usize>,
836    name: String,
837    delayed: bool,
838) {
839    let key = name.to_ascii_lowercase();
840    match seen.get(&key) {
841        Some(&at) => {
842            if !delayed {
843                out[at].1 = false;
844            }
845        }
846        None => {
847            seen.insert(key, out.len());
848            out.push((name, delayed));
849        }
850    }
851}
852
853/// `len` bytes at `offset`, or `None` if the file is shorter than that.
854fn at(file: &mut File, offset: u64, len: usize) -> Option<Vec<u8>> {
855    if len == 0 {
856        return None;
857    }
858    file.seek(SeekFrom::Start(offset)).ok()?;
859    let mut buffer = vec![0u8; len];
860    file.read_exact(&mut buffer).ok()?;
861    Some(buffer)
862}
863
864fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
865    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
866}
867
868fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
869    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
870}
871
872fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
873    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
874}
875
876#[cfg(test)]
877mod tests {
878    use super::*;
879
880    /// The test binary is itself a PE file, which makes it the one fixture that is real, is
881    /// always there, and is built for whatever this machine is.
882    fn me() -> PathBuf {
883        std::env::current_exe().expect("a test process has an executable")
884    }
885
886    #[test]
887    fn a_real_binary_names_what_it_needs() {
888        let image = read(&me()).expect("the test binary is a PE file");
889        assert_ne!(image.machine, 0, "a real image says what it was built for");
890        assert!(
891            !image.imports.is_empty(),
892            "every Windows binary imports something"
893        );
894        // Whatever else a Rust program on Windows imports, it imports the C runtime's host or
895        // the API sets that stand in for it. What is asserted here is the shape of the answer
896        // rather than one name: every entry is a plausible module name.
897        for (name, _) in &image.imports {
898            assert!(
899                name.len() > 3 && name.contains('.'),
900                "{name:?} does not look like a DLL name"
901            );
902        }
903        // The two tables are merged with no duplicates, which is what the panel counts on.
904        let mut keys: Vec<String> = image
905            .imports
906            .iter()
907            .map(|(name, _)| name.to_ascii_lowercase())
908            .collect();
909        let before = keys.len();
910        keys.sort();
911        keys.dedup();
912        assert_eq!(before, keys.len(), "a DLL is named twice: {keys:?}");
913    }
914
915    #[test]
916    fn something_that_is_not_a_binary_says_so_rather_than_guessing() {
917        let text = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
918        assert_eq!(read(&text).err(), Some("not a binary"));
919        assert_eq!(
920            read(&PathBuf::from("no-such-file-anywhere.dll")).err(),
921            Some("cannot be opened")
922        );
923    }
924
925    /// The point of the whole module: a real graph, more than one level deep, with every
926    /// module either accounted for or honestly marked.
927    #[test]
928    fn a_walk_finds_where_each_dependency_lives_and_keeps_going() {
929        let graph = walk(&me(), BUDGET, PATIENCE);
930        assert_eq!(graph.root().state, State::Found);
931        assert!(
932            graph.modules.len() > 2,
933            "a walk of a real binary reaches more than its own imports: {}",
934            graph.modules.len()
935        );
936
937        // Recursion: something in the graph is a dependency of a dependency. Without this the
938        // test would pass on a walk that read the root's import table and stopped.
939        let second_level = graph.modules[0]
940            .imports
941            .iter()
942            .any(|edge| !graph.modules[edge.to].imports.is_empty());
943        assert!(second_level, "nothing was followed past the first level");
944
945        // Every module is one of the four things it can be, and a found one has a path that
946        // exists and came off the search list.
947        for (i, module) in graph.modules.iter().enumerate() {
948            match module.state {
949                State::Found => {
950                    let path = module.path.as_ref().expect("a found module has a path");
951                    assert!(path.is_file(), "{path:?} was reported and is not there");
952                    assert!(
953                        graph
954                            .search
955                            .iter()
956                            .any(|dir| path.parent() == Some(dir.as_path())),
957                        "{path:?} is not in any directory that was searched"
958                    );
959                    assert_ne!(module.machine, 0, "{:?} has no machine", module.name);
960                }
961                State::ApiSet => {
962                    assert!(is_api_set(&module.name));
963                    assert!(module.path.is_none(), "an API set is not a file");
964                }
965                State::Missing | State::Unreadable(_) => {}
966                State::Unvisited => assert!(
967                    graph.truncated,
968                    "{:?} was never visited and the walk does not admit stopping",
969                    module.name
970                ),
971            }
972            // Nothing points outside the graph, and the root is nobody's index by accident.
973            for edge in &module.imports {
974                assert!(edge.to < graph.modules.len(), "module {i} has a wild edge");
975            }
976        }
977
978        // The system DLLs are found through `PATH`, which is what the search order claims.
979        assert!(
980            graph
981                .modules
982                .iter()
983                .any(|m| matches!(m.state, State::Found) && m.path.as_ref() != Some(&me())),
984            "nothing but the root was resolved"
985        );
986    }
987
988    /// A cycle does not become an endless walk, and it does not become a duplicated module
989    /// either. Real graphs have them — `kernel32` and `kernelbase` refer to each other through
990    /// forwarders — so this is checked against whatever this machine's really are.
991    #[test]
992    fn a_graph_holds_each_module_once_however_many_things_want_it() {
993        let graph = walk(&me(), BUDGET, PATIENCE);
994        let mut names: Vec<String> = graph
995            .modules
996            .iter()
997            .map(|m| m.name.to_ascii_lowercase())
998            .collect();
999        let before = names.len();
1000        names.sort();
1001        names.dedup();
1002        assert_eq!(before, names.len(), "a module appears twice in the graph");
1003    }
1004
1005    /// The root binary's own folder is searched before `PATH`.
1006    ///
1007    /// Checked by putting a copy of the test binary in a folder of its own under a name
1008    /// nothing else could resolve to, and walking *that*: its first search directory has to be
1009    /// the folder the copy is in.
1010    #[test]
1011    fn the_binarys_own_folder_is_looked_in_first() {
1012        let root = std::env::temp_dir().join(format!("yafe-pe-{}", std::process::id()));
1013        let _ = std::fs::remove_dir_all(&root);
1014        std::fs::create_dir_all(&root).expect("a directory in the temp folder");
1015        let copy = root.join("yafe-fixture.dll");
1016        std::fs::copy(me(), &copy).expect("a copy of the test binary");
1017
1018        let search = search_paths(&copy);
1019        assert_eq!(
1020            search.first().map(|p| p.as_path()),
1021            Some(root.as_path()),
1022            "the binary's own folder is not the first place looked"
1023        );
1024        // And a name that only exists there resolves to it, which is the behaviour that
1025        // search order is *for*.
1026        assert_eq!(
1027            resolve("yafe-fixture.dll", &search).as_deref(),
1028            Some(copy.as_path())
1029        );
1030        // Nothing reaches out of the search path, however the name is written.
1031        assert_eq!(resolve(r"..\yafe-fixture.dll", &search), None);
1032        assert_eq!(resolve(&copy.to_string_lossy(), &search), None);
1033
1034        let graph = walk(&copy, BUDGET, PATIENCE);
1035        assert_eq!(graph.root().state, State::Found);
1036        assert!(graph.modules.len() > 2);
1037        let _ = std::fs::remove_dir_all(&root);
1038    }
1039
1040    /// What a walk of a real binary actually costs, and what it comes back with.
1041    ///
1042    /// Ignored, like `scan::flatten_speed`: it is a measurement rather than an assertion, it
1043    /// prints, and what it measures depends on the machine. `cargo test -- --ignored
1044    /// --nocapture walk_speed` — with `YAFE_WALK` set to walk something larger than the test
1045    /// binary, which is what a number worth having is measured on.
1046    #[test]
1047    #[ignore]
1048    fn walk_speed() {
1049        let target = std::env::var_os("YAFE_WALK")
1050            .map(PathBuf::from)
1051            .unwrap_or_else(me);
1052        let graph = walk(&target, BUDGET, PATIENCE);
1053        let (files, api_sets, missing) = graph.tally();
1054        println!(
1055            "{} on {} threads: {} modules ({files} files, {api_sets} api sets), \
1056             {missing} missing, {:.1} ms{}",
1057            target.display(),
1058            hands(),
1059            graph.modules.len(),
1060            graph.micros as f64 / 1000.0,
1061            if graph.truncated { " (truncated)" } else { "" }
1062        );
1063        for (i, module) in graph.modules.iter().enumerate() {
1064            let where_ = match (&module.path, module.state) {
1065                (Some(path), State::Found) => path
1066                    .parent()
1067                    .map(|p| p.display().to_string())
1068                    .unwrap_or_default(),
1069                (_, State::ApiSet) => "(api set)".to_owned(),
1070                (_, State::Missing) => "NOT FOUND".to_owned(),
1071                (_, State::Unreadable(why)) => why.to_owned(),
1072                (_, State::Unvisited) => "(not visited)".to_owned(),
1073                (None, State::Found) => String::new(),
1074            };
1075            println!(
1076                "  {:>3} {:<34} {:<8} {:<3} {where_}",
1077                i,
1078                module.name,
1079                machine_name(module.machine),
1080                module.imports.len(),
1081            );
1082        }
1083    }
1084
1085    #[test]
1086    fn an_api_set_is_told_apart_from_a_missing_dll() {
1087        assert!(is_api_set("api-ms-win-core-file-l1-2-0.dll"));
1088        assert!(is_api_set("API-MS-WIN-CRT-RUNTIME-L1-1-0.DLL"));
1089        assert!(is_api_set("ext-ms-win-ntuser-window-l1-1-0.dll"));
1090        assert!(!is_api_set("kernel32.dll"));
1091        // Not merely a prefix: the name has to have something after it.
1092        assert!(!is_api_set("api-ms-"));
1093        assert!(!is_api_set("apiset.dll"));
1094    }
1095
1096    #[test]
1097    fn only_a_binary_is_offered_a_walk() {
1098        for yes in ["exe", "EXE", "dll", "Sys", "ocx", "node", "pyd"] {
1099            assert!(is_image(yes), "{yes} is a binary");
1100        }
1101        for no in ["", "txt", "rs", "zip", "exe2", "dl"] {
1102            assert!(!is_image(no), "{no} is not a binary");
1103        }
1104    }
1105
1106    /// A walk with no patience hands back what it has rather than nothing at all.
1107    ///
1108    /// The root is read whatever the deadline says — it is read before the clock is consulted —
1109    /// so the honest answer to "no time at all" is one module and `truncated`.
1110    #[test]
1111    fn a_walk_out_of_patience_admits_it() {
1112        let graph = walk(&me(), BUDGET, Duration::ZERO);
1113        assert_eq!(graph.root().state, State::Found, "the root is always read");
1114        assert!(graph.truncated, "a walk that stopped early has to say so");
1115        assert!(graph
1116            .modules
1117            .iter()
1118            .skip(1)
1119            .all(|m| m.state == State::Unvisited));
1120    }
1121
1122    /// And a walk with no budget stops at the budget, for the same reason.
1123    #[test]
1124    fn a_walk_stops_at_its_budget_and_admits_it() {
1125        let graph = walk(&me(), 2, PATIENCE);
1126        assert!(graph.modules.len() <= 2);
1127        assert!(graph.truncated);
1128    }
1129
1130    /// Reading in parallel gives the same answer as reading one at a time.
1131    ///
1132    /// The thread count is what [`probe_all`] branches on — under it the level goes inline —
1133    /// so this drives both paths over the same real graph and compares them name for name.
1134    #[test]
1135    fn reading_in_parallel_does_not_change_the_answer() {
1136        let search = search_paths(&me());
1137        let image = read(&me()).expect("the test binary is a PE file");
1138        let names: Vec<String> = image.imports.into_iter().map(|(name, _)| name).collect();
1139        if names.len() < 2 {
1140            println!("this binary imports {} modules; nothing to compare", names.len());
1141            return;
1142        }
1143        let until = Instant::now() + PATIENCE;
1144        let (inline, _) = probe_all(&names, &search, until, 1);
1145        for hands in [2, 4, 8] {
1146            let (threaded, gave_up) = probe_all(&names, &search, until, hands);
1147            assert!(!gave_up, "a warm walk on {hands} threads ran out of time");
1148            assert_eq!(inline.len(), threaded.len());
1149            for (one, many) in inline.iter().zip(&threaded) {
1150                assert_eq!(one.path, many.path, "a different file was resolved");
1151                assert_eq!(one.state, many.state);
1152                assert_eq!(one.machine, many.machine);
1153                assert_eq!(one.imports, many.imports, "a different import list");
1154            }
1155        }
1156    }
1157}
