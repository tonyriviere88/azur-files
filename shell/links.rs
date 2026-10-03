1//! What a shortcut points at, found without the listing waiting for it.
2//!
3//! Two kinds of row are a shortcut, and they are unrelated to each other:
4//!
5//! - **A `.lnk` file**, which is the shell's own shortcut and the one people mean. Its target
6//!   is not in the file system at all, it is in the file's contents, and reading it means
7//!   `IShellLink` — a COM object per file.
8//! - **A reparse point**, which is a symlink or a junction. Its target is metadata and comes
9//!   back from one syscall.
10//!
11//! Both are asked about the same way and answered into the same column, because from the row's
12//! point of view they are one thing: a name that stands for somewhere else.
13//!
14//! # Why this is a service and not a function
15//!
16//! [`crate::fs`]'s rule is that the listing never goes back to the disk for something the
17//! enumeration already told it — and a shortcut's target is the one thing on a row that the
18//! enumeration cannot tell it. So this is the exception, built the way the other exception is
19//! ([`crate::shell::icons`]): asked once per row per view, answered on a worker, delivered to
20//! the tab that asked, and dropped if that tab has moved on.
21//!
22//! It has to be off the UI thread for the same reason the per-file icons do. A `.lnk` pointing
23//! at a share that is not currently reachable is the classic Explorer hang, and `IPersistFile
24//! ::Load` is where it happens. Nothing here is allowed to make the window wait.
25//!
26//! `SLGP_RAWPATH` is the other half of that: it returns the path the shortcut *stores* rather
27//! than asking the shell to find where the target has moved to, which is what makes a stale
28//! shortcut cost a local file read instead of a network timeout. The cost is that a shortcut
29//! whose target has moved shows where it used to be — which is the honest answer to "what does
30//! this point at", and the same one `Properties` shows.
31
32use std::path::{Path, PathBuf};
33use std::sync::atomic::{AtomicUsize, Ordering};
34use std::sync::mpsc::{channel, Receiver, Sender};
35use std::sync::Arc;
36
37/// Which kind of shortcut a row is, which is what decides how its target is found.
38#[derive(Clone, Copy, PartialEq, Eq, Debug)]
39pub enum Kind {
40    /// A `.lnk` file: `IShellLink`, on a thread that is allowed to block.
41    Shortcut,
42    /// A symlink or a junction: one syscall.
43    Reparse,
44}
45
46/// What kind of shortcut, if any, a row is — from the enumeration alone, so this costs nothing
47/// to ask about every row of every listing.
48///
49/// `.url` files are deliberately not included: an internet shortcut's target is a URL rather
50/// than a path, it is an `.ini` file rather than a shell object, and a listing is not a browser.
51pub fn kind_of(ext: &str, is_reparse: bool) -> Option<Kind> {
52    if is_reparse {
53        Some(Kind::Reparse)
54    } else if ext.eq_ignore_ascii_case("lnk") {
55        Some(Kind::Shortcut)
56    } else {
57        None
58    }
59}
60
61struct Job {
62    view: u64,
63    row: u32,
64    path: PathBuf,
65    kind: Kind,
66}
67
68struct Ready {
69    view: u64,
70    row: u32,
71    /// `None` when there is nothing to show: an unreadable shortcut, one pointing at a shell
72    /// folder with no path behind it, or a platform where this does not apply.
73    target: Option<String>,
74}
75
76/// The shortcut-target service. One per application.
77pub struct Links {
78    /// The worker, started on the first request — a window that never opens a folder of
79    /// shortcuts never starts a thread.
80    jobs: Option<Sender<Job>>,
81    answers: Receiver<Ready>,
82    /// Kept so the worker can be started later with a live channel to answer on.
83    replies: Sender<Ready>,
84    /// How many requests are outstanding, so a folder of ten thousand shortcuts cannot queue
85    /// ten thousand of them. A row that is turned away is simply asked again next frame.
86    queued: Arc<AtomicUsize>,
87    ctx: egui::Context,
88}
89
90/// The most requests that may be in flight at once.
91///
92/// Only the rows on screen are ever asked about, so this is reached by scrolling fast rather
93/// than by opening a big folder. Turning a request away costs nothing: the row draws without
94/// its context for one frame and asks again.
95const QUEUE_CAP: usize = 64;
96
97impl Links {
98    pub fn new(ctx: &egui::Context) -> Self {
99        let (replies, answers) = channel();
100        Self {
101            jobs: None,
102            answers,
103            replies,
104            queued: Arc::new(AtomicUsize::new(0)),
105            ctx: ctx.clone(),
106        }
107    }
108
109    /// Ask what the shortcut at `path` points at. `false` if it was turned away.
110    ///
111    /// `view` and `row` are how the answer finds its way back: the tab's current view of its
112    /// current folder, and the entry index within it. An answer to a view nobody holds any more
113    /// is dropped on delivery, so nothing about a folder outlives looking at it.
114    pub fn request(&mut self, view: u64, row: u32, path: PathBuf, kind: Kind) -> bool {
115        if self.queued.load(Ordering::Relaxed) >= QUEUE_CAP {
116            return false;
117        }
118        let jobs = self.worker().clone();
119        self.queued.fetch_add(1, Ordering::Relaxed);
120        if jobs
121            .send(Job {
122                view,
123                row,
124                path,
125                kind,
126            })
127            .is_err()
128        {
129            self.queued.fetch_sub(1, Ordering::Relaxed);
130            return false;
131        }
132        true
133    }
134
135    /// Everything resolved since the last call, as `(view, row, target)`. Drained.
136    pub fn answers(&mut self) -> Vec<(u64, u32, Option<String>)> {
137        self.answers
138            .try_iter()
139            .map(|ready| (ready.view, ready.row, ready.target))
140            .collect()
141    }
142
143    /// The worker, started on demand.
144    ///
145    /// One thread with a queue rather than a thread per file, for the reason
146    /// [`crate::shell::icons`] gives at length: a folder of shortcuts scrolled quickly would
147    /// otherwise be a thread per row, each with its own stack.
148    fn worker(&mut self) -> &Sender<Job> {
149        let replies = self.replies.clone();
150        let queued = self.queued.clone();
151        let ctx = self.ctx.clone();
152        self.jobs.get_or_insert_with(|| {
153            let (send, receive) = channel::<Job>();
154            let spawned = std::thread::Builder::new()
155                .name("shell-links".to_owned())
156                .spawn(move || work(receive, replies, queued, ctx));
157            // A machine that will not give us a thread gets rows without their context, which
158            // is what they looked like before this existed.
159            let _ = spawned;
160            send
161        })
162    }
163}
164
165/// Resolve until the channel closes, which is when the application is gone.
166fn work(jobs: Receiver<Job>, replies: Sender<Ready>, queued: Arc<AtomicUsize>, ctx: egui::Context) {
167    apartment();
168    // Any read can touch an empty removable drive, which would otherwise raise "Please insert
169    // a disk into drive E:" from inside the syscall.
170    crate::fs::scan::silence_device_dialogs();
171
172    while let Ok(job) = jobs.recv() {
173        let target = match job.kind {
174            Kind::Shortcut => shortcut_target(&job.path),
175            Kind::Reparse => reparse_target(&job.path),
176        };
177        queued.fetch_sub(1, Ordering::Relaxed);
178        if replies
179            .send(Ready {
180                view: job.view,
181                row: job.row,
182                target,
183            })
184            .is_err()
185        {
186            return;
187        }
188        ctx.request_repaint();
189    }
190}
191
192/// Join the multi-threaded apartment, once, for the life of the thread.
193///
194/// **Not a single-threaded one**, which is the interesting choice and the one this program's own
195/// rule decides: an STA is a promise to answer calls back into it, and a worker parked in
196/// `recv` answers nothing — see [`crate::shell::answering_calls`] for what that breaks. Nothing
197/// here hands an interface out, so there is nothing to be called back about, and the MTA needs
198/// no message pump. `ShellLink` is registered as `Both`, so it is created in this apartment
199/// rather than marshalled into one.
200///
201/// Never uninitialised, because the thread lives as long as the process does.
202fn apartment() {
203    #[cfg(windows)]
204    {
205        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
206        // SAFETY: called once, on this thread, before any COM call on it.
207        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
208    }
209}
210
211/// The path stored inside a `.lnk`.
212fn shortcut_target(path: &Path) -> Option<String> {
213    read_shortcut(path).map(|(target, _)| target)
214}
215
216/// **The folder a shortcut leads to, if it leads to one.**
217///
218/// This is what makes a folder shortcut open *in this window* rather than in Explorer, and it
219/// is the one thing in this module that runs on the caller's thread — the UI thread, at the
220/// moment a row is opened. Three reasons that is the right way round here, where it would not
221/// be for the display:
222///
223/// - **It has to be an answer, not an answer later.** Double-clicking a shortcut has to do the
224///   same thing every time. Reaching for the column [`Links`] fills in would mean the gesture
225///   depended on whether the row had been on screen long enough, which is the kind of
226///   intermittent that is never reported and never fixed.
227/// - **It is one small local read**, of a file in the folder already being listed, and only for
228///   a row whose name ends in `.lnk`. The alternative on that path is `ShellExecute`, which
229///   does considerably more on the same thread.
230/// - **Nothing is asked of the *target*.** Whether it is a folder comes from the attributes the
231///   shortcut itself stores, so a shortcut to a share that is not currently reachable costs
232///   nothing to classify. A shortcut whose stored attributes have gone stale — the target
233///   replaced by a file since — falls through to the shell, which is what used to happen to all
234///   of them.
235///
236/// `None` for anything this cannot answer: not a `.lnk`, a shortcut to a file, or one pointing
237/// at a shell object with no path at all — a library, the Recycle Bin, Control Panel. Those go
238/// to the shell, which is the only thing that can open them.
239pub fn folder_target(path: &Path) -> Option<PathBuf> {
240    // The extension first, so nothing but a shortcut ever costs a COM object.
241    if !path
242        .extension()
243        .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
244    {
245        return None;
246    }
247    let (target, folder) = read_shortcut(path)?;
248    folder.then(|| PathBuf::from(target))
249}
250
251/// The path a `.lnk` stores, and whether the attributes it stores with it say directory.
252#[cfg(windows)]
253fn read_shortcut(path: &Path) -> Option<(String, bool)> {
254    use std::os::windows::ffi::OsStrExt;
255    use windows::core::{Interface, PCWSTR};
256    use windows::Win32::Storage::FileSystem::{
257        WIN32_FIND_DATAW, FILE_ATTRIBUTE_DIRECTORY,
258    };
259    use windows::Win32::System::Com::{
260        CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER, STGM_READ,
261    };
262    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink, SLGP_RAWPATH};
263
264    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
265    // A target longer than `MAX_PATH` is what `SLGP_RAWPATH` can hand back — it is the string
266    // the file holds, not a path the API has parsed — so the buffer is not `MAX_PATH`.
267    let mut buffer = [0u16; 1024];
268    let mut find = WIN32_FIND_DATAW::default();
269
270    // SAFETY: every pointer below is to a local that outlives the call, and each call's result
271    // is checked before the next one uses what it produced.
272    unsafe {
273        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
274        let file: IPersistFile = link.cast().ok()?;
275        file.Load(PCWSTR(wide.as_ptr()), STGM_READ).ok()?;
276        link.GetPath(&mut buffer, &mut find, SLGP_RAWPATH.0 as u32).ok()?;
277    }
278
279    let len = buffer.iter().position(|&unit| unit == 0).unwrap_or(0);
280    // Empty means the shortcut points at something with no path: the Recycle Bin, Control
281    // Panel, a printer. There is nothing useful to show, so the row keeps its name alone.
282    if len == 0 {
283        return None;
284    }
285    // The attributes the shortcut *stores*, filled in when it was made. Nothing is asked of the
286    // target itself, which is what keeps a shortcut to an unreachable share cheap.
287    let folder = find.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
288    Some((String::from_utf16_lossy(&buffer[..len]), folder))
289}
290
291#[cfg(not(windows))]
292fn read_shortcut(_path: &Path) -> Option<(String, bool)> {
293    None
294}
295
296/// Where a symlink or a junction leads.
297fn reparse_target(path: &Path) -> Option<String> {
298    let target = std::fs::read_link(path).ok()?;
299    let text = target.to_string_lossy();
300    // A junction stores `\\?\C:\…`. The prefix is there to get the path past the Win32 parser
301    // and means nothing to a person reading a row.
302    Some(
303        text.strip_prefix(r"\\?\")
304            .unwrap_or(&text)
305            .trim_end_matches('\\')
306            .to_owned(),
307    )
308}
309
310/// Write a `.lnk` at `at` pointing at `target`. `false` if the shell refused.
311///
312/// Test-only, and the only way to have a real shortcut to resolve: a fixture checked into the
313/// repository would be a binary blob nobody could read, and one from `C:\Users` is not the
314/// same on two machines. It uses the same two interfaces the reading does, from the other end.
315#[cfg(all(test, windows))]
316pub(crate) fn write_shortcut(at: &Path, target: &Path) -> bool {
317    use std::os::windows::ffi::OsStrExt;
318    use windows::core::{Interface, PCWSTR};
319    use windows::Win32::System::Com::{CoCreateInstance, IPersistFile, CLSCTX_INPROC_SERVER};
320    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
321
322    fn made(at: &[u16], target: &[u16]) -> Option<()> {
323        // SAFETY: both slices are NUL-terminated locals of the caller, alive for the calls.
324        unsafe {
325            let link: IShellLinkW =
326                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
327            link.SetPath(PCWSTR(target.as_ptr())).ok()?;
328            let file: IPersistFile = link.cast().ok()?;
329            file.Save(PCWSTR(at.as_ptr()), true).ok()?;
330        }
331        Some(())
332    }
333
334    // The writing thread needs an apartment as much as the reading one does.
335    apartment();
336    let wide = |path: &Path| -> Vec<u16> {
337        path.as_os_str().encode_wide().chain(Some(0)).collect()
338    };
339    made(&wide(at), &wide(target)).is_some()
340}
341
342#[cfg(test)]
343mod tests {
344    use super::*;
345
346    #[test]
347    fn only_a_lnk_or_a_reparse_point_is_a_shortcut() {
348        assert_eq!(kind_of("lnk", false), Some(Kind::Shortcut));
349        // Extensions are stored as they were on disk, so the comparison cannot be `==`.
350        assert_eq!(kind_of("LNK", false), Some(Kind::Shortcut));
351        assert_eq!(kind_of("Lnk", false), Some(Kind::Shortcut));
352        assert_eq!(kind_of("", true), Some(Kind::Reparse));
353        // A reparse point wins: a `.lnk` that is also a symlink is asked about as the symlink,
354        // which is the answer that says why the row is not where you expect it to be.
355        assert_eq!(kind_of("lnk", true), Some(Kind::Reparse));
356        assert_eq!(kind_of("txt", false), None);
357        assert_eq!(kind_of("", false), None);
358        // Not a shortcut as far as this is concerned — see `kind_of`.
359        assert_eq!(kind_of("url", false), None);
360    }
361
362    /// A real shortcut, written and then read back: the folder one opens here, the file one
363    /// does not.
364    ///
365    /// The point of the whole thing. A shortcut to a folder used to be handed to the shell,
366    /// which opened a second file manager over the top of this one.
367    #[cfg(windows)]
368    #[test]
369    fn a_shortcut_to_a_folder_resolves_to_the_folder_and_one_to_a_file_does_not() {
370        let root = std::env::temp_dir().join(format!("yafe-lnk-{}", std::process::id()));
371        let folder = root.join("somewhere");
372        let file = root.join("something.txt");
373        let _ = std::fs::remove_dir_all(&root);
374        std::fs::create_dir_all(&folder).expect("a directory in the temp folder");
375        std::fs::write(&file, b"x").expect("a file in it");
376
377        let to_folder = root.join("folder.lnk");
378        let to_file = root.join("file.lnk");
379        assert!(
380            write_shortcut(&to_folder, &folder) && write_shortcut(&to_file, &file),
381            "the shell would not write a shortcut here"
382        );
383
384        assert_eq!(
385            folder_target(&to_folder).as_deref(),
386            Some(folder.as_path()),
387            "a shortcut to a folder has to resolve to that folder"
388        );
389        assert_eq!(
390            folder_target(&to_file),
391            None,
392            "a shortcut to a file is the shell's business, not a place to navigate to"
393        );
394        // And the display half agrees about where both of them point.
395        assert_eq!(
396            shortcut_target(&to_file).as_deref(),
397            Some(file.to_string_lossy().as_ref())
398        );
399
400        // Nothing that is not a shortcut costs a COM object, which is what the extension test
401        // in `folder_target` is for.
402        assert_eq!(folder_target(&file), None);
403        assert_eq!(folder_target(&folder), None);
404        let _ = std::fs::remove_dir_all(&root);
405    }
406
407    /// A real reparse point, where the machine allows one to be made.
408    ///
409    /// Creating a symlink needs either Developer Mode or an elevated process, so this skips
410    /// rather than fails where neither is true — and says so, because a test that quietly does
411    /// nothing is worse than one that is not there. The junctions this is really for
412    /// (`C:\Users\All Users`) cannot be made without shelling out to `mklink`, which a test has
413    /// no business doing; they are read by the same one syscall.
414    #[test]
415    fn a_symlink_reports_where_it_leads() {
416        let root = std::env::temp_dir().join(format!("yafe-link-{}", std::process::id()));
417        let target = root.join("target");
418        let link = root.join("link");
419        let _ = std::fs::remove_dir_all(&root);
420        std::fs::create_dir_all(&target).expect("a directory in the temp folder");
421
422        #[cfg(windows)]
423        let made = std::os::windows::fs::symlink_dir(&target, &link);
424        #[cfg(not(windows))]
425        let made = std::os::unix::fs::symlink(&target, &link);
426
427        if made.is_err() {
428            println!("no privilege to create a symlink here; skipping");
429            let _ = std::fs::remove_dir_all(&root);
430            return;
431        }
432        let got = reparse_target(&link).expect("a symlink has a target");
433        assert_eq!(
434            got,
435            target.to_string_lossy(),
436            "the target came back as something else"
437        );
438        // And the enumeration agrees that the row is one, which is what asks the question.
439        let dir = crate::fs::scan::scan(&root);
440        let row = (0..dir.len())
441            .find(|&i| dir.name(i) == "link")
442            .expect("the link is in the listing");
443        assert_eq!(
444            kind_of(dir.ext(row), dir.entries[row].is_link()),
445            Some(Kind::Reparse)
446        );
447        let _ = std::fs::remove_dir_all(&root);
448    }
449
450    /// A junction's target comes back without the prefix that gets a path past the parser.
451    #[test]
452    fn a_reparse_target_is_shown_the_way_a_person_writes_it() {
453        // `read_link` is what produces these; this checks the tidying done to its answer, which
454        // is the part that is this program's own.
455        let tidy = |raw: &str| {
456            let text = std::borrow::Cow::Borrowed(raw);
457            text.strip_prefix(r"\\?\")
458                .unwrap_or(&text)
459                .trim_end_matches('\\')
460                .to_owned()
461        };
462        assert_eq!(tidy(r"\\?\C:\ProgramData"), r"C:\ProgramData");
463        assert_eq!(tidy(r"C:\ProgramData"), r"C:\ProgramData");
464        assert_eq!(tidy(r"\\?\C:\Users\"), r"C:\Users");
465        assert_eq!(tidy(r"\\server\share"), r"\\server\share");
466    }
467}
