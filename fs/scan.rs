1//! Reading a directory, as fast as the platform will allow.
2//!
3//! # What is actually expensive
4//!
5//! All the numbers below are from [`tests::scan_speed`], over 60,000 files in a
6//! warm directory on this machine. Run it yourself with
7//! `cargo test --release -- --ignored --nocapture scan_speed`; the shape matters
8//! more than the absolute figures, and the shape is not what you would guess.
9//!
10//! | doing the same work | per entry | vs this scanner |
11//! | --- | --- | --- |
12//! | this module | 657 ns | — |
13//! | `std::fs::read_dir` | 690 ns | 1.05× |
14//! | `read_dir` + a `stat` per entry | 133 µs | **202× slower** |
15//! | `SHGetFileInfo` for one type name | 1.1 ms | **1700× slower** |
16//!
17//! So the syscall is **not** where the win is. `FindFirstFileExW` at
18//! [`FindExInfoBasic`] with [`FIND_FIRST_EX_LARGE_FETCH`] is used here because it
19//! is a few percent quicker and because it hands the UTF-16 name straight into the
20//! arena with no `OsString` in between — but `std::fs::read_dir` is within 5% of it,
21//! and anyone claiming a four-times speedup from the choice of enumeration call has
22//! not measured one.
23//!
24//! What *is* the win is the last two rows, and both are things this program refuses
25//! to do:
26//!
27//! - **Never ask about an entry twice.** The find data already carries the name, the
28//!   size, the times and the attributes. The moment code reaches for `Path::is_dir`
29//!   or `fs::metadata` on top of that it has turned one sequential read into an
30//!   `open`/`query`/`close` per file — 202 times the cost, and the reason most
31//!   file managers crawl on a large folder. Nothing here stats an entry it has
32//!   already enumerated.
33//! - **Never ask the shell what a file is.** `SHGetFileInfo` with `SHGFI_TYPENAME`
34//!   is a `HKEY_CLASSES_ROOT` walk per file, and it measures at over a millisecond
35//!   each here — 67 *seconds* for this folder. [`super::fmt::type_label`] is a
36//!   binary search over a static table at 45 ns, which is the same answer 25,000
37//!   times sooner.
38
39use super::dir::{Dir, DirBuilder, FLAG_DIR, FLAG_HIDDEN, FLAG_LINK, FLAG_READONLY, FLAG_SYSTEM};
40use std::path::{Path, PathBuf};
41use std::time::Instant;
42
43/// Read `path` into a [`Dir`]. Never fails: an unreadable directory comes back as
44/// an empty one carrying the reason.
45///
46/// An empty path is the synthetic "This PC" listing of drives.
47pub fn scan(path: &Path) -> Dir {
48    let started = Instant::now();
49    if path.as_os_str().is_empty() {
50        return super::drives::this_pc(started);
51    }
52    scan_real(path, started)
53}
54
55// ---------------------------------------------------------------------------
56// Windows
57// ---------------------------------------------------------------------------
58
59#[cfg(windows)]
60fn scan_real(path: &Path, started: Instant) -> Dir {
61    use std::ffi::c_void;
62    use windows_sys::Win32::Foundation::{GetLastError, INVALID_HANDLE_VALUE};
63    use windows_sys::Win32::Storage::FileSystem::{
64        FindClose, FindExInfoBasic, FindExSearchNameMatch, FindFirstFileExW, FindNextFileW,
65        FIND_FIRST_EX_LARGE_FETCH, WIN32_FIND_DATAW,
66    };
67
68    let pattern = search_pattern(path);
69    let mut data = WIN32_FIND_DATAW::default();
70
71    let handle = unsafe {
72        FindFirstFileExW(
73            pattern.as_ptr(),
74            FindExInfoBasic,
75            (&mut data) as *mut WIN32_FIND_DATAW as *mut c_void,
76            FindExSearchNameMatch,
77            std::ptr::null(),
78            FIND_FIRST_EX_LARGE_FETCH,
79        )
80    };
81    if handle == INVALID_HANDLE_VALUE {
82        let code = unsafe { GetLastError() };
83        // An empty directory reports "no more files" from the *first* call rather
84        // than coming back with zero entries, so it lands here and is not an error.
85        const ERROR_FILE_NOT_FOUND: u32 = 2;
86        const ERROR_NO_MORE_FILES: u32 = 18;
87        if matches!(code, ERROR_FILE_NOT_FOUND | ERROR_NO_MORE_FILES) {
88            return DirBuilder::new(path).finish(elapsed_micros(started));
89        }
90        return Dir::failed(path, error_text(code));
91    }
92
93    let mut builder = DirBuilder::new(path);
94    loop {
95        let name = trimmed_name(&data.cFileName);
96        if !is_dot_entry(name) {
97            builder.push_wide(
98                name,
99                (data.nFileSizeHigh as u64) << 32 | data.nFileSizeLow as u64,
100                filetime(&data.ftLastWriteTime),
101                flags_of(data.dwFileAttributes),
102            );
103        }
104        if unsafe { FindNextFileW(handle, &mut data) } == 0 {
105            break;
106        }
107    }
108    unsafe { FindClose(handle) };
109
110    builder.finish(elapsed_micros(started))
111}
112
113/// Read `root` and everything under it into one flat [`Dir`], names relative to
114/// `root`, stopping after `budget` entries.
115///
116/// What the flatten button in the path bar shows. The listing is an ordinary `Dir` in
117/// every respect the rest of the program can see: [`Dir::target`] still joins a name
118/// onto the folder, so `sub\deep\file.txt` resolves to the file it names, and the sort,
119/// the filter, the selection, the icons and every file operation work on it unchanged.
120/// The one thing that is different is the names, and [`Dir::leaf`] is there for the two
121/// places that need the file's own name back — renaming, and revealing a row.
122///
123/// # Three rules the walk has to keep
124///
125/// - **A reparse point is not descended into.** `C:\Users\All Users` is a junction to
126///   `C:\ProgramData`, `C:\Documents and Settings` is a junction to `C:\Users`, and a
127///   walk that follows either of those on a system drive never finishes. The link is
128///   still *listed* — it is content of the folder — it is simply not opened. That is
129///   what every backup tool and `robocopy` do by default, for the same reason.
130/// - **It stops somewhere.** A flatten of `C:\` is a request to enumerate a million
131///   files, and this is a button next to a text field. `budget` caps the rows and
132///   `patience` caps the wall clock, whichever comes first; the listing says it was cut
133///   off either way. Because every directory the walk descends into was itself pushed
134///   as an entry first, the row cap bounds the number of directories opened too.
135/// - **Breadth first.** A depth-first walk spends its budget down the leftmost branch
136///   and a cut-off listing shows one deep path and nothing else; breadth first spends
137///   it evenly, so a truncated answer is still a fair picture of the tree. It also
138///   costs one queue entry per directory rather than a recursion whose depth is the
139///   tree's, and it is what makes the directories at each level a *batch* that can be
140///   read in parallel.
141///
142/// # Where the time goes
143///
144/// A directory read is a syscall waiting on the file system, so the walk is not CPU-bound and a
145/// thread that is waiting is a thread another directory could have been read on. The batch of
146/// directories at each level is read on up to [`hands`] of them at once — see [`enumerate`],
147/// which keeps the answers in batch order so the walk is still breadth-first.
148///
149/// Measured warm by [`tests::flatten_speed`], where one thread is the serial walk this replaced:
150///
151/// | tree | entries | 1 thread | 4 | **8** | 16 |
152/// | --- | --- | --- | --- | --- | --- |
153/// | this crate's `src` | 35 | 0.2 ms | 0.2 | **0.2** | 0.2 |
154/// | its `target` | 26,917 | 47.9 ms | 25.5 | **22.8** | 24.9 |
155/// | `C:\Program Files` | 188,729 | 1,945 ms | 672 | **561** | 537 |
156///
157/// So the case that was worth doing something about — a tree big enough to wait for — comes back
158/// **3.5× sooner**, and the case that was already instant is untouched: a batch no bigger than
159/// the thread count is read inline, because spawning threads to read three directories costs
160/// more than reading them. Sixteen threads is a wash against eight, which is why [`hands`] stops
161/// there.
162///
163/// Errors in the middle are skipped rather than reported: a tree of ten thousand
164/// folders where one is denied is not a failed read, and the denied folder simply
165/// contributes nothing. A root that cannot be read at all still comes back as
166/// [`Dir::failed`], which is what the caller shows.
167pub fn scan_deep(root: &Path, budget: usize, patience: std::time::Duration) -> Dir {
168    let started = Instant::now();
169    // "This PC" is not a folder and has no tree: its rows are volumes, each of which is
170    // a place to flatten of its own. Flattening it would mean walking every drive in the
171    // machine, which is not what anybody means by the button.
172    if root.as_os_str().is_empty() {
173        return super::drives::this_pc(started);
174    }
175
176    walk(root, budget, patience, hands())
177}
178
179/// The walk, with the number of enumerating threads spelled out.
180///
181/// Split from [`scan_deep`] for [`tests::flatten_speed`], which is what the figures in this
182/// module's documentation come from: one worker is the old serial walk, and the comparison is
183/// the only honest way to keep the claim about the others.
184fn walk(root: &Path, budget: usize, patience: std::time::Duration, hands: usize) -> Dir {
185    let started = Instant::now();
186    let mut builder = DirBuilder::new(root);
187    // Directories still to open, each with its path relative to the root. Popped from the front
188    // in batches, so the walk stays breadth-first however many threads are reading.
189    let mut pending: std::collections::VecDeque<(PathBuf, String)> =
190        std::collections::VecDeque::from([(root.to_path_buf(), String::new())]);
191    // Composed once per entry and reused, so a tree of 200,000 files is not 200,000
192    // allocations for names that are copied into the arena immediately afterwards.
193    let mut relative = String::with_capacity(260);
194    let mut truncated = false;
195    let mut failed: Option<Dir> = None;
196    // **The folder itself is always read**, whatever the deadline says: a flatten that came back
197    // with nothing at all would be a worse answer than the listing it replaced. So the deadline
198    // arrives after the first batch, which is the root alone.
199    let mut deadline: Option<Instant> = None;
200
201    'walk: while !pending.is_empty() {
202        // Once per batch rather than once per entry: a clock read is cheap, and a batch is the
203        // unit the walk can stop between.
204        if deadline.is_some_and(|until| Instant::now() >= until) {
205            truncated = true;
206            break;
207        }
208        let take = pending.len().min(BATCH);
209        let batch: Vec<(PathBuf, String)> = pending.drain(..take).collect();
210        let (listings, gave_up) = enumerate(&batch, hands, deadline);
211        deadline = Some(started + patience);
212        // A batch that ran out of time mid-way has left directories unread, and the outer check
213        // above cannot be relied on to notice: the batch may have been the last one, and then
214        // the walk would end looking complete. This is the only thing that must not happen.
215        truncated |= gave_up;
216
217        for (here, dir) in listings {
218            // The *root* failing is the only failure worth reporting: it means there is nothing
219            // to flatten. A directory somewhere in the middle that cannot be read contributes
220            // nothing and is not an error — a tree of ten thousand folders where one is denied
221            // is not a failed read.
222            if dir.error.is_some() && dir.path == root {
223                failed = Some(dir);
224                break 'walk;
225            }
226            for i in 0..dir.len() {
227                if builder.len() >= budget {
228                    truncated = true;
229                    break 'walk;
230                }
231                let entry = dir.entries[i];
232                relative.clear();
233                if !here.is_empty() {
234                    relative.push_str(&here);
235                    relative.push('\\');
236                }
237                relative.push_str(dir.name(i));
238                builder.push(&relative, entry.size, entry.modified, entry.flags);
239                if descends(entry.flags) {
240                    pending.push_back((dir.target(i), relative.clone()));
241                }
242            }
243        }
244    }
245
246    if let Some(failed) = failed {
247        return failed;
248    }
249    let mut flat = builder.finish(elapsed_micros(started));
250    flat.truncated = truncated;
251    flat
252}
253
254/// Read a batch of directories, in batch order, on up to `hands` threads.
255///
256/// **This is where a flatten's time goes**, and why it is worth reading in parallel: a directory
257/// read is a syscall waiting on the file system, so a thread that is waiting is a thread another
258/// directory could have been read on. The measured figures are in [`scan_deep`]'s documentation.
259///
260/// Ordered, because the order is what makes the walk breadth-first, and breadth-first is what
261/// makes a *truncated* listing a fair picture of the tree. Each worker returns what it read
262/// tagged with where it came from in the batch, and the results are put back in order — a sort
263/// of a few hundred integers per batch against a syscall each.
264///
265/// **A batch no bigger than the number of threads goes inline**, which is the first batch of
266/// every walk and the whole of one over a shallow folder. Spawning a thread costs about 70µs
267/// here and a warm directory read costs about 100µs, so a batch that gives each thread one
268/// directory spends more on the threads than it saves; the rule asks for two each before it is
269/// worth it. Measured, on this crate's own `src`: 0.2 ms inline against 0.4 ms with threads for
270/// its second batch of three directories.
271///
272/// `until` is the walk's deadline, checked between directories on every thread: a batch of a
273/// thousand folders on a share that has gone away would otherwise take a thousand timeouts to
274/// get through, whatever the caller's patience said. `None` for the first batch, which is the
275/// folder being flattened and is read whatever the deadline says. The second half of the answer
276/// says whether it stopped for that reason, because the caller cannot tell from a short result
277/// alone — and a walk that quietly came back short is the one thing this must not do.
278fn enumerate(
279    batch: &[(PathBuf, String)],
280    hands: usize,
281    until: Option<Instant>,
282) -> (Vec<(String, Dir)>, bool) {
283    let out_of_time = || until.is_some_and(|until| Instant::now() >= until);
284    if batch.len() <= hands || hands <= 1 {
285        let mut out = Vec::with_capacity(batch.len());
286        for (path, here) in batch {
287            if out_of_time() {
288                return (out, true);
289            }
290            out.push((here.clone(), scan_real(path, Instant::now())));
291        }
292        return (out, false);
293    }
294
295    // The next directory in the batch nobody has taken. A shared counter rather than a slice
296    // each, because directories differ in size by orders of magnitude: handing a worker a fixed
297    // sixth of the batch means five workers waiting on whoever got `node_modules`.
298    let next = std::sync::atomic::AtomicUsize::new(0);
299    let parts: Vec<Vec<(usize, String, Dir)>> = std::thread::scope(|scope| {
300        let workers: Vec<_> = (0..hands.min(batch.len()))
301            .map(|_| {
302                let next = &next;
303                scope.spawn(move || {
304                    // Per-thread, and these threads are new: without it, a batch that reaches an
305                    // empty card reader raises "Please insert a disk" from inside the syscall.
306                    silence_device_dialogs();
307                    let mut mine = Vec::new();
308                    loop {
309                        let at = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
310                        let Some((path, here)) = batch.get(at) else {
311                            return mine;
312                        };
313                        if out_of_time() {
314                            return mine;
315                        }
316                        mine.push((at, here.clone(), scan_real(path, Instant::now())));
317                    }
318                })
319            })
320            .collect();
321        // A worker that panicked contributes nothing rather than taking the window with it.
322        workers.into_iter().filter_map(|w| w.join().ok()).collect()
323    });
324
325    let mut all: Vec<(usize, String, Dir)> = parts.into_iter().flatten().collect();
326    all.sort_unstable_by_key(|(at, ..)| *at);
327    let gave_up = all.len() < batch.len();
328    (
329        all.into_iter().map(|(_, here, dir)| (here, dir)).collect(),
330        gave_up,
331    )
332}
333
334/// How many directories are read before the walk stops to build them.
335///
336/// The batch is what bounds two things at once: how many listings are held in memory before they
337/// are copied into the one arena and dropped, and how far past the budget the walk can read
338/// before it notices. A thousand ordinary folders is a couple of megabytes in flight, and it is
339/// large enough that the threads are spawned about ten times over a tree of ten thousand folders
340/// rather than forty.
341const BATCH: usize = 1024;
342
343/// How many threads read directories at once.
344///
345/// The same shape as the loader's worker count and for the same reason: this is bound by the file
346/// system rather than by the CPU, so more threads than a handful buys nothing and costs context
347/// switches. Eight rather than the loader's four because a flatten is one burst of thousands of
348/// reads rather than a steady trickle of one.
349fn hands() -> usize {
350    std::thread::available_parallelism()
351        .map(|n| n.get().clamp(2, 8))
352        .unwrap_or(2)
353}
354
355/// Whether [`scan_deep`] opens an entry with these flags.
356///
357/// A directory, unless it is a reparse point. Named and taken apart from the walk so the rule
358/// that keeps the walk finite can be tested without a link in the filesystem — making one
359/// needs either a privilege this program does not ask for or a shell-out, and the rule is too
360/// important to leave to a test that gets skipped on the machines that have neither.
361#[inline]
362fn descends(flags: u16) -> bool {
363    flags & FLAG_DIR != 0 && flags & FLAG_LINK == 0
364}
365
366/// How many entries a flattened listing may hold.
367///
368/// Two things decide it. A row costs its 32-byte record plus its name, and a *relative
369/// path* is a longer name than a bare one — call it 150 bytes a row, so this is about
370/// 30 MB, which is the most a view nobody asked to keep should be allowed to cost.
371/// And the listing has to stay usable: the details view draws only the rows on screen,
372/// so 200,000 scrolls at the refresh rate, but the sort behind it is one pass over all
373/// of them on every column click.
374///
375/// It is a stopping point rather than a judgement about what is reasonable to flatten.
376/// The status line says when a listing hit it, so the answer is never quietly short.
377pub const FLATTEN_BUDGET: usize = 200_000;
378
379/// How long a flatten may take before it hands back what it has.
380///
381/// The row budget alone is not a bound on *time*: a tree of a hundred nearly-empty
382/// directories on a sleeping network share costs one round trip each and no rows at all,
383/// and the entry cap would never be reached. Ten seconds is long enough for any local
384/// tree worth flattening — this repository is 40ms — and short enough that a mistake
385/// made on `\\server\archive` is something you wait out rather than something that has
386/// hung.
387pub const FLATTEN_PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);
388
389/// `\\?\C:\some\dir\*`, ready for `FindFirstFileExW`.
390///
391/// The `\\?\` prefix lifts the 260-character limit and skips the Win32 path
392/// parser, which is a per-call cost this makes on every directory. It also turns
393/// off `.`/`..` collapsing — fine here, because every path in this program comes
394/// from a canonicalised navigation, never from user text that was not resolved
395/// first.
396#[cfg(windows)]
397fn search_pattern(path: &Path) -> Vec<u16> {
398    use std::os::windows::ffi::OsStrExt;
399
400    let raw: Vec<u16> = path.as_os_str().encode_wide().collect();
401    let mut out: Vec<u16> = Vec::with_capacity(raw.len() + 8);
402
403    const SEP: u16 = b'\\' as u16;
404    let starts_with = |prefix: &str| {
405        raw.len() >= prefix.len()
406            && raw
407                .iter()
408                .zip(prefix.encode_utf16())
409                .take(prefix.len())
410                .all(|(a, b)| *a == b || (*a == '/' as u16 && b == SEP))
411    };
412
413    if starts_with(r"\\?\") || starts_with(r"\\.\") {
414        out.extend_from_slice(&raw);
415    } else if starts_with(r"\\") {
416        // `\\server\share` -> `\\?\UNC\server\share`.
417        out.extend(r"\\?\UNC".encode_utf16());
418        out.extend_from_slice(&raw[1..]);
419    } else if raw.len() >= 2 && raw[1] == b':' as u16 {
420        out.extend(r"\\?\".encode_utf16());
421        out.extend_from_slice(&raw);
422    } else {
423        // Relative, or something exotic. Hand it to the normal parser.
424        out.extend_from_slice(&raw);
425    }
426
427    // The extended prefix takes backslashes only.
428    for unit in &mut out {
429        if *unit == '/' as u16 {
430            *unit = SEP;
431        }
432    }
433    if out.last() != Some(&SEP) {
434        out.push(SEP);
435    }
436    out.push(b'*' as u16);
437    out.push(0);
438    out
439}
440
441/// The name out of a `cFileName`, without the padding after the terminator.
442#[cfg(windows)]
443#[inline]
444fn trimmed_name(buf: &[u16; 260]) -> &[u16] {
445    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
446    &buf[..len]
447}
448
449/// `.` and `..`, which every directory reports and nobody wants to see.
450#[cfg(windows)]
451#[inline]
452fn is_dot_entry(name: &[u16]) -> bool {
453    const DOT: u16 = b'.' as u16;
454    match name.len() {
455        1 => name[0] == DOT,
456        2 => name[0] == DOT && name[1] == DOT,
457        _ => false,
458    }
459}
460
461#[cfg(windows)]
462#[inline]
463fn filetime(ft: &windows_sys::Win32::Foundation::FILETIME) -> u64 {
464    (ft.dwHighDateTime as u64) << 32 | ft.dwLowDateTime as u64
465}
466
467#[cfg(windows)]
468#[inline]
469fn flags_of(attrs: u32) -> u16 {
470    use windows_sys::Win32::Storage::FileSystem as fs;
471    let mut flags = 0;
472    if attrs & fs::FILE_ATTRIBUTE_DIRECTORY != 0 {
473        flags |= FLAG_DIR;
474    }
475    if attrs & fs::FILE_ATTRIBUTE_HIDDEN != 0 {
476        flags |= FLAG_HIDDEN;
477    }
478    if attrs & fs::FILE_ATTRIBUTE_SYSTEM != 0 {
479        flags |= FLAG_SYSTEM;
480    }
481    if attrs & fs::FILE_ATTRIBUTE_REPARSE_POINT != 0 {
482        flags |= FLAG_LINK;
483    }
484    if attrs & fs::FILE_ATTRIBUTE_READONLY != 0 {
485        flags |= FLAG_READONLY;
486    }
487    flags
488}
489
490/// The handful of failures a file manager actually meets, in words.
491///
492/// `FormatMessageW` would give the localised text for all of them, at the cost of
493/// another feature of `windows-sys` and a `LocalFree` on every path; these five
494/// cover everything a user can act on.
495#[cfg(windows)]
496fn error_text(code: u32) -> String {
497    match code {
498        2 | 3 => "This folder no longer exists".to_owned(),
499        5 => "Access denied".to_owned(),
500        15 => "The drive is not available".to_owned(),
501        21 => "The device is not ready".to_owned(),
502        53 | 67 => "The network path was not found".to_owned(),
503        1223 => "Cancelled".to_owned(),
504        other => format!("Could not read this folder (error {other})"),
505    }
506}
507
508/// Ask Windows not to put up an "insert a disk" dialog behind our back.
509///
510/// Any call that touches an empty removable drive — the drive list does, on every
511/// refresh — pops a modal from inside the syscall unless this is set. It is
512/// per-thread, so the loader's workers each call it once.
513#[cfg(windows)]
514pub fn silence_device_dialogs() {
515    use windows_sys::Win32::System::Diagnostics::Debug::{
516        SetThreadErrorMode, SEM_FAILCRITICALERRORS,
517    };
518    unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, std::ptr::null_mut()) };
519}
520
521#[cfg(not(windows))]
522pub fn silence_device_dialogs() {}
523
524// ---------------------------------------------------------------------------
525// Everything else
526// ---------------------------------------------------------------------------
527
528/// The portable path. Correct, and slower — `read_dir` here has to `stat` for the
529/// file type on some platforms, which is exactly what the Windows path avoids.
530#[cfg(not(windows))]
531fn scan_real(path: &Path, started: Instant) -> Dir {
532    let iter = match std::fs::read_dir(path) {
533        Ok(iter) => iter,
534        Err(e) => return Dir::failed(path, e.to_string()),
535    };
536    let mut builder = DirBuilder::new(path);
537    for entry in iter.flatten() {
538        let name = entry.file_name();
539        let Some(name) = name.to_str() else { continue };
540        let meta = match entry.metadata() {
541            Ok(meta) => meta,
542            Err(_) => continue,
543        };
544        let mut flags = 0;
545        if meta.is_dir() {
546            flags |= FLAG_DIR;
547        }
548        if meta.file_type().is_symlink() {
549            flags |= FLAG_LINK;
550        }
551        if name.starts_with('.') {
552            flags |= FLAG_HIDDEN;
553        }
554        if meta.permissions().readonly() {
555            flags |= FLAG_READONLY;
556        }
557        let _ = FLAG_SYSTEM;
558        builder.push(name, meta.len(), unix_to_filetime(&meta), flags);
559    }
560    builder.finish(elapsed_micros(started))
561}
562
563/// Modification time as a `FILETIME`, so both platforms sort and format the same
564/// integer.
565#[cfg(not(windows))]
566fn unix_to_filetime(meta: &std::fs::Metadata) -> u64 {
567    let Ok(time) = meta.modified() else { return 0 };
568    let Ok(since) = time.duration_since(std::time::UNIX_EPOCH) else {
569        return 0;
570    };
571    super::time::UNIX_EPOCH_FILETIME + since.as_secs() * 10_000_000 + since.subsec_nanos() as u64 / 100
572}
573
574fn elapsed_micros(started: Instant) -> u64 {
575    started.elapsed().as_micros() as u64
576}
577
578#[cfg(test)]
579mod tests {
580    use super::*;
581
582    /// Read a directory this program's own source lives in, which is guaranteed to
583    /// exist and to have both files and subdirectories in it.
584    fn here() -> Dir {
585        scan(Path::new(env!("CARGO_MANIFEST_DIR")))
586    }
587
588    /// This crate's own `src`, which has files at the top, three subdirectories under it and
589    /// nothing enormous anywhere — unlike the crate root, which has `target` in it.
590    fn sources() -> PathBuf {
591        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
592    }
593
594    /// What a flatten costs, and what reading in parallel buys.
595    ///
596    /// ```text
597    /// cargo test --release -- --ignored --nocapture flatten_speed
598    /// ```
599    ///
600    /// Over this crate's own `target` directory by default, which is the largest warm tree on
601    /// hand; `YAFE_FLATTEN_ROOT` points it somewhere else. One worker is the serial walk this
602    /// replaced, so the comparison is against the real alternative rather than against nothing.
603    /// Two passes, and the second is the one to read: the first warms the file system's cache,
604    /// and a flatten of a folder somebody is looking at is a warm read.
605    #[test]
606    #[ignore = "walks a large tree; run explicitly"]
607    fn flatten_speed() {
608        let root = std::env::var("YAFE_FLATTEN_ROOT")
609            .map(PathBuf::from)
610            .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target"));
611        if !root.is_dir() {
612            println!("no {}; skipping", root.display());
613            return;
614        }
615        println!("flattening {}", root.display());
616        let patience = std::time::Duration::from_secs(600);
617        for pass in 1..=2 {
618            for hands in [1usize, 2, 4, 8, 16] {
619                let started = Instant::now();
620                let dir = walk(&root, FLATTEN_BUDGET, patience, hands);
621                let elapsed = started.elapsed();
622                let per = elapsed.as_nanos() as f64 / dir.len().max(1) as f64;
623                println!(
624                    "pass {pass}  {hands:>2} thread(s): {:>7} entries in {:>8.1} ms  ({per:>6.0} ns/entry){}",
625                    dir.len(),
626                    elapsed.as_secs_f64() * 1000.0,
627                    if dir.truncated { "  [truncated]" } else { "" }
628                );
629            }
630        }
631    }
632
633    /// However many threads read it, the answer is the same listing.
634    ///
635    /// The order is part of that: it is what makes a truncated walk a fair picture of the tree,
636    /// and it is the thing a parallel read is most likely to lose.
637    #[test]
638    fn reading_in_parallel_does_not_change_the_answer() {
639        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
640        let names = |hands: usize| -> Vec<String> {
641            let dir = walk(&root, FLATTEN_BUDGET, FLATTEN_PATIENCE, hands);
642            (0..dir.len()).map(|i| dir.name(i).to_owned()).collect()
643        };
644        let serial = names(1);
645        assert!(serial.len() > 20, "the fixture tree is too small to mean much");
646        for hands in [2, 4, 8] {
647            assert_eq!(
648                names(hands),
649                serial,
650                "{hands} threads produced a different listing"
651            );
652        }
653        // And the budget still cuts in the same place, which is the property the order is for.
654        let cut = |hands: usize| -> Vec<String> {
655            let dir = walk(&root, 12, FLATTEN_PATIENCE, hands);
656            assert!(dir.truncated);
657            (0..dir.len()).map(|i| dir.name(i).to_owned()).collect()
658        };
659        assert_eq!(cut(8), cut(1), "a truncated walk kept different rows");
660    }
661
662    #[test]
663    fn a_flattened_listing_holds_paths_relative_to_the_folder_it_walked() {
664        let dir = scan_deep(&sources(), FLATTEN_BUDGET, FLATTEN_PATIENCE);
665        assert!(dir.error.is_none(), "{:?}", dir.error);
666        assert!(!dir.truncated, "this crate's own sources fit in any budget");
667        let named = |want: &str| (0..dir.len()).find(|&i| dir.name(i) == want);
668
669        // A child of the root is a bare name, exactly as a shallow scan has it.
670        assert!(named("main.rs").is_some(), "the root's own files are in it");
671        // A folder is content too: listed, as well as walked into.
672        let ui = named("ui").expect("`src\\ui` is a row of its own");
673        assert!(dir.entries[ui].is_dir());
674
675        // And a grandchild carries the folders in front of it, which is what the row shows and
676        // what the sort and the filter see.
677        let deep = named("ui\\filelist.rs").expect("the walk reached into `ui`");
678        assert_eq!(dir.leaf(deep), "filelist.rs", "the file's own name");
679        assert_eq!(dir.ext(deep), "rs", "the extension is the last component's");
680        assert_eq!(
681            dir.target(deep),
682            sources().join("ui").join("filelist.rs"),
683            "a row has to lead to the file it names"
684        );
685        // Every count is the tree's, not the folder's.
686        assert!(
687            dir.file_count > 20,
688            "only {} files: the walk did not go down",
689            dir.file_count
690        );
691    }
692
693    #[test]
694    fn a_reparse_point_is_listed_but_never_descended_into() {
695        // The rule that keeps a flatten of `C:\` finite. `C:\Users\All Users` is a junction to
696        // `C:\ProgramData` and `C:\Documents and Settings` is one to `C:\Users`, so a walk that
697        // follows either never finishes — it does not even loop visibly, it just keeps finding
698        // more tree. Every backup tool refuses the same thing for the same reason.
699        assert!(descends(FLAG_DIR), "a plain folder is walked into");
700        assert!(
701            !descends(FLAG_DIR | FLAG_LINK),
702            "a junction must not be followed"
703        );
704        assert!(!descends(0), "a file is not a folder");
705        assert!(!descends(FLAG_HIDDEN), "nor is a hidden one");
706        // A hidden *folder* is still a folder: `.git` is content of the tree, and hiding a row
707        // is the display's business — `show_hidden` — not the walk's.
708        assert!(descends(FLAG_DIR | FLAG_HIDDEN));
709    }
710
711    #[test]
712    fn a_flatten_stops_at_its_budget_and_admits_it() {
713        let dir = scan_deep(&sources(), 3, FLATTEN_PATIENCE);
714        assert_eq!(dir.len(), 3, "the budget is a hard stop");
715        assert!(dir.truncated, "a listing missing rows has to say so");
716    }
717
718    #[test]
719    fn a_flatten_out_of_patience_hands_back_what_it_has() {
720        // No time at all: the folder itself is still read — a flatten that came back with
721        // nothing would be a worse answer than the listing it replaced — and nothing under it
722        // is opened.
723        let dir = scan_deep(&sources(), FLATTEN_BUDGET, std::time::Duration::ZERO);
724        assert!(dir.truncated);
725        assert!(!dir.is_empty(), "the root's own children are the floor");
726        assert!(
727            (0..dir.len()).all(|i| !dir.name(i).contains('\\')),
728            "it descended anyway, with no time to do it in"
729        );
730    }
731
732    #[test]
733    fn a_listing_has_no_dot_entries() {
734        let dir = here();
735        assert!(dir.error.is_none(), "{:?}", dir.error);
736        for i in 0..dir.len() {
737            let name = dir.name(i);
738            assert!(name != "." && name != "..", "`{name}` should be filtered out");
739            assert!(!name.is_empty());
740        }
741    }
742
743    #[test]
744    fn sizes_and_kinds_come_from_the_enumeration() {
745        let dir = here();
746        let named = |want: &str| (0..dir.len()).find(|&i| dir.name(i) == want);
747
748        let cargo = named("Cargo.toml").expect("this crate has a manifest");
749        assert!(!dir.entries[cargo].is_dir());
750        assert!(dir.entries[cargo].size > 0, "the size came from the find data");
751        assert!(
752            dir.entries[cargo].modified > super::super::time::UNIX_EPOCH_FILETIME,
753            "and so did the timestamp"
754        );
755        assert_eq!(dir.ext(cargo), "toml");
756
757        let src = named("src").expect("this crate has sources");
758        assert!(dir.entries[src].is_dir());
759        assert_eq!(dir.ext(src), "", "a directory's dots are part of its name");
760    }
761
762    #[test]
763    fn a_missing_directory_reports_why() {
764        let dir = scan(Path::new(r"Q:\no\such\place\at\all"));
765        assert!(dir.is_empty());
766        assert!(dir.error.is_some(), "a failed read has to say so");
767    }
768
769    #[test]
770    fn an_empty_directory_is_not_an_error() {
771        let mut path = std::env::temp_dir();
772        path.push(format!("yafe-empty-{}", std::process::id()));
773        std::fs::create_dir_all(&path).expect("temp dir");
774
775        let dir = scan(&path);
776        assert!(dir.is_empty());
777        assert!(
778            dir.error.is_none(),
779            "an empty folder reports `no more files` from the *first* call, which is \
780             not a failure: {:?}",
781            dir.error
782        );
783
784        let _ = std::fs::remove_dir(&path);
785    }
786
787    /// The claim this whole module exists to make, checked rather than asserted.
788    ///
789    /// Ignored by default because it writes 60,000 files. Run it deliberately:
790    ///
791    /// ```text
792    /// cargo test --release -- --ignored --nocapture scan_speed
793    /// ```
794    #[test]
795    #[ignore = "creates 60k files; run explicitly"]
796    fn scan_speed() {
797        const COUNT: usize = 60_000;
798
799        let mut root = std::env::temp_dir();
800        root.push(format!("yafe-bench-{}", std::process::id()));
801        std::fs::create_dir_all(&root).expect("temp dir");
802
803        // Names of mixed length and extension, so the transcode and the extension
804        // split are both exercised rather than measured on one shape.
805        let exts = ["rs", "txt", "png", "e57", "", "tar.gz"];
806        for i in 0..COUNT {
807            let ext = exts[i % exts.len()];
808            let name = if ext.is_empty() {
809                format!("entry_{i:06}")
810            } else {
811                format!("some_moderately_long_name_{i:06}.{ext}")
812            };
813            let _ = std::fs::write(root.join(name), b"x");
814        }
815
816        // Warm: the first read pays for the directory's metadata coming into cache,
817        // and what is being measured is the steady state a user actually sees.
818        let _ = scan(&root);
819
820        let mut best = u64::MAX;
821        for _ in 0..5 {
822            let dir = scan(&root);
823            assert_eq!(dir.len(), COUNT, "{:?}", dir.error);
824            best = best.min(dir.scan_micros);
825        }
826        let per_entry_ns = best as f64 * 1000.0 / COUNT as f64;
827        println!(
828            "scan of {COUNT} entries: {:.1} ms  ({per_entry_ns:.0} ns/entry)",
829            best as f64 / 1000.0
830        );
831
832        // The same folder through the standard library, for the comparison the module
833        // documentation makes. Same shape of work: every name, every size, every
834        // timestamp, every attribute — into the same arena.
835        let mut std_best = u128::MAX;
836        for _ in 0..5 {
837            let started = Instant::now();
838            let mut names = String::new();
839            let mut count = 0usize;
840            let mut bytes = 0u64;
841            for entry in std::fs::read_dir(&root).expect("read_dir").flatten() {
842                let name = entry.file_name();
843                names.push_str(&name.to_string_lossy());
844                let meta = entry.metadata().expect("metadata");
845                bytes += meta.len();
846                count += 1;
847            }
848            std::hint::black_box((&names, bytes));
849            assert_eq!(count, COUNT);
850            std_best = std_best.min(started.elapsed().as_micros());
851        }
852        println!(
853            "std::fs::read_dir, same work: {:.1} ms  ({:.0} ns/entry, {:.2}x)",
854            std_best as f64 / 1000.0,
855            std_best as f64 * 1000.0 / COUNT as f64,
856            std_best as f64 / best as f64
857        );
858
859        // The mistake that actually costs: asking the filesystem about each entry
860        // *again*, by path, after the enumeration has already answered. This is what
861        // `Path::is_dir` in a loop compiles down to, and it is the single easiest way
862        // to turn a fast listing into a slow one.
863        {
864            let started = Instant::now();
865            let mut dirs = 0usize;
866            for entry in std::fs::read_dir(&root).expect("read_dir").flatten() {
867                let path = entry.path();
868                if path.is_dir() {
869                    dirs += 1;
870                }
871                let _ = std::fs::metadata(&path).map(|m| m.len());
872            }
873            std::hint::black_box(dirs);
874            let restat = started.elapsed().as_micros();
875            println!(
876                "read_dir + a stat per entry:  {:.1} ms  ({:.0} ns/entry, {:.1}x slower)",
877                restat as f64 / 1000.0,
878                restat as f64 * 1000.0 / COUNT as f64,
879                restat as f64 / best as f64
880            );
881        }
882
883        // And the other per-entry cost this program refuses to pay: asking the shell
884        // what a file *is*, which is what fills Explorer's Type column.
885        #[cfg(windows)]
886        {
887            use std::os::windows::ffi::OsStrExt as _;
888            use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_TYPENAME};
889
890            // A thousand is plenty to get a per-file cost, and sixty thousand of these
891            // would make the test unbearable — which is rather the point.
892            const SAMPLE: usize = 1_000;
893            let names: Vec<Vec<u16>> = (0..SAMPLE)
894                .map(|i| {
895                    let ext = exts[i % exts.len()];
896                    let name = if ext.is_empty() {
897                        format!("entry_{i:06}")
898                    } else {
899                        format!("some_moderately_long_name_{i:06}.{ext}")
900                    };
901                    root.join(name)
902                        .as_os_str()
903                        .encode_wide()
904                        .chain(std::iter::once(0))
905                        .collect()
906                })
907                .collect();
908
909            let started = Instant::now();
910            for wide in &names {
911                let mut info = SHFILEINFOW::default();
912                unsafe {
913                    SHGetFileInfoW(
914                        wide.as_ptr(),
915                        0,
916                        &mut info,
917                        std::mem::size_of::<SHFILEINFOW>() as u32,
918                        SHGFI_TYPENAME,
919                    )
920                };
921                std::hint::black_box(info.szTypeName[0]);
922            }
923            let shell_ns = started.elapsed().as_nanos() as f64 / SAMPLE as f64;
924            println!(
925                "SHGetFileInfo type name:      {:.0} ns/entry  \
926                 (= {:.0} ms for {COUNT} entries, {:.0}x the whole scan)",
927                shell_ns,
928                shell_ns * COUNT as f64 / 1_000_000.0,
929                shell_ns * COUNT as f64 / 1000.0 / best as f64
930            );
931
932            // The table this program uses instead, over the same sample.
933            let started = Instant::now();
934            let mut label = String::new();
935            for i in 0..SAMPLE {
936                label.clear();
937                super::super::fmt::type_label(exts[i % exts.len()], false, &mut label);
938                std::hint::black_box(label.len());
939            }
940            println!(
941                "the static table instead:     {:.0} ns/entry",
942                started.elapsed().as_nanos() as f64 / SAMPLE as f64
943            );
944        }
945
946        // Sorting is the other half of what happens before a listing appears.
947        let mut order = Vec::new();
948        let dir = scan(&root);
949        let started = Instant::now();
950        super::super::sort::build_order(
951            &dir,
952            &mut order,
953            super::super::Column::Name,
954            true,
955            false,
956            "",
957        );
958        let sort_us = started.elapsed().as_micros();
959        println!("natural sort of {COUNT} entries: {:.1} ms", sort_us as f64 / 1000.0);
960        assert_eq!(order.len(), COUNT);
961
962        // Not a tight bound — a loaded machine or a slow volume can be several times
963        // this. It is here to catch a regression of the kind that matters: a `stat`
964        // per entry, or an allocation per name, which would be ten times over.
965        assert!(
966            per_entry_ns < 3_000.0,
967            "{per_entry_ns:.0} ns per entry — something has started doing per-file work"
968        );
969
970        let _ = std::fs::remove_dir_all(&root);
971    }
972}
