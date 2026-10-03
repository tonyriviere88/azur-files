1//! Copy, move, delete, rename and new-folder — through `IFileOperation`.
2//!
3//! This is the shell's own engine, which is not a detail. Going through it means this
4//! program gets, for free and *correctly*:
5//!
6//! - the **Recycle Bin**, and with it undo — a delete that cannot be undone is a
7//!   different feature from the one users expect Delete to be;
8//! - the **progress dialog**, with its estimate, its pause and its cancel;
9//! - the **conflict prompts** — "there is already a file with this name", with the
10//!   comparison of both and the keep-both option;
11//! - the **elevation prompt**, when a destination needs administrator rights;
12//! - **long path, junction, reparse point and cloud placeholder** handling, none of
13//!   which a hand-rolled recursive copy gets right;
14//! - correct behaviour on a collision that differs only in case, on a file in use, and
15//!   on a path over 260 characters.
16//!
17//! Re-implementing that over `std::fs` would produce something that looks the same in
18//! the easy cases and quietly loses data in the hard ones.
19//!
20//! What the shell puts on screen, and when, is worth knowing rather than assuming —
21//! `what_the_shell_puts_on_screen` lists it:
22//!
23//! | | |
24//! | --- | --- |
25//! | a copy onto an existing name | `Replace or Skip Files`, in an `OperationStatusWindow` |
26//! | Shift+Delete | `Delete File`, a plain `#32770` confirmation |
27//! | Delete to the Recycle Bin | nothing, which is Windows’ own default |
28//! | a copy into the folder it is already in | nothing, and a `one - Copy.txt` appears |
29//!
30//! The last row is the one that needed doing. Copying something into the folder it already
31//! lives in cannot mean “replace it with itself”, so Explorer does not ask: it makes
32//! `one - Copy.txt` and gets on with it. Without `FOF_RENAMEONCOLLISION` the same paste stopped
33//! on the Replace-or-Skip dialog, which is not what Ctrl+C Ctrl+V does anywhere else in
34//! Windows. The name is the shell’s own and localised — measured as `one - Copie.txt` here.
35//!
36//! # Why each operation gets its own thread
37//!
38//! `PerformOperations` is synchronous: it shows the progress dialog and does not
39//! return until the work is done or cancelled. Called on the UI thread it would freeze
40//! this window for the duration — so each operation runs on a thread of its own, which
41//! initialises COM as its own apartment and pumps the dialog there. The window stays
42//! live, the dialog is parented to it, and the affected folders are re-read when the
43//! thread reports back.
44
45use std::path::{Path, PathBuf};
46use std::sync::mpsc::{channel, Receiver, Sender};
47
48use super::Owner;
49
50/// What an operation did, once it has finished.
51#[derive(Clone, Debug)]
52pub struct Done {
53    /// The folders whose contents may have changed, so they can be re-read.
54    pub touched: Vec<PathBuf>,
55    /// Empty when it worked — including when the user cancelled, which is an answer
56    /// rather than a failure.
57    pub error: Option<String>,
58    /// What still has to happen now that it has.
59    pub after: After,
60    /// The name the shell gave a newly created folder.
61    ///
62    /// Asked for rather than guessed. `NewItem` is given the name to *start* from and the shell
63    /// picks the first free one, so what actually appears may be `New folder (3)` — and
64    /// nothing on this side can know which without being told. `IFileOperationProgressSink`
65    /// is how it is told.
66    pub created: Option<String>,
67}
68
69/// What is left to do once an operation finishes, beyond re-reading the folders.
70#[derive(Clone, PartialEq, Eq, Debug, Default)]
71pub enum After {
72    #[default]
73    Nothing,
74    /// This was a paste of a cut: if it worked, the clipboard has to be told so, and then
75    /// emptied. Carried through the operation rather than done when it starts, because a
76    /// paste the user cancels at the conflict dialog has to leave the cut where it was. The
77    /// number is the clipboard's sequence when the paste began, so a clipboard that has since
78    /// changed is left alone.
79    FinishCut(u32),
80    /// A folder was created: select it in this pane and open its name for editing, once the
81    /// re-read brings it in. Which name that is comes back on [`Done::created`].
82    NameIt(crate::pane::PaneId),
83}
84
85/// The work to do.
86#[derive(Clone, Debug)]
87pub enum Job {
88    Copy { items: Vec<PathBuf>, into: PathBuf },
89    Move { items: Vec<PathBuf>, into: PathBuf },
90    /// `to_bin` sends them to the Recycle Bin, which is what Delete does; `false` is
91    /// Shift+Delete.
92    Delete { items: Vec<PathBuf>, to_bin: bool },
93    Rename { item: PathBuf, name: String },
94    NewFolder { parent: PathBuf, name: String },
95}
96
97impl Job {
98    /// A present-tense description, for the status line while it runs.
99    pub fn describe(&self) -> String {
100        let count = |items: &Vec<PathBuf>| {
101            if items.len() == 1 {
102                "1 item".to_owned()
103            } else {
104                format!("{} items", items.len())
105            }
106        };
107        match self {
108            Self::Copy { items, .. } => format!("Copying {}…", count(items)),
109            Self::Move { items, .. } => format!("Moving {}…", count(items)),
110            Self::Delete {
111                items,
112                to_bin: true,
113            } => format!("Recycling {}…", count(items)),
114            Self::Delete { items, .. } => format!("Deleting {}…", count(items)),
115            Self::Rename { .. } => "Renaming…".to_owned(),
116            Self::NewFolder { .. } => "Creating a folder…".to_owned(),
117        }
118    }
119
120    /// The folders this will change, so they can be re-read afterwards.
121    pub fn touches(&self) -> Vec<PathBuf> {
122        let parents = |items: &Vec<PathBuf>| -> Vec<PathBuf> {
123            items
124                .iter()
125                .filter_map(|p| p.parent().map(Path::to_path_buf))
126                .collect()
127        };
128        let mut touched = match self {
129            Self::Copy { items, into } | Self::Move { items, into } => {
130                let mut all = parents(items);
131                all.push(into.clone());
132                all
133            }
134            Self::Delete { items, .. } => parents(items),
135            Self::Rename { item, .. } => item.parent().map(Path::to_path_buf).into_iter().collect(),
136            Self::NewFolder { parent, .. } => vec![parent.clone()],
137        };
138        touched.sort();
139        touched.dedup();
140        touched
141    }
142}
143
144/// Whether a test is allowed to hand a job to the real shell. **Off by default.**
145///
146/// This is not caution, it is a bug that was shipped and found — the same one, in the same shape,
147/// as the guard on [`crate::config::Config::save`]. `the_clipboard_events_map_to_the_right_actions`
148/// selects the first row of the harness's folder, which is `CARGO_MANIFEST_DIR`, and feeds the
149/// window a Shift+Delete to check that the event maps to `Action::Delete` rather than to a cut.
150/// The frame it does that in *applies* the action, so a green test quietly asked `IFileOperation`
151/// to **permanently delete `.cargo` from this repository** and put the shell's confirmation dialog
152/// up to ask about it. It ran on its own thread, so the test finished and passed while the prompt
153/// was still on screen; whether the folder survived came down to a human not clicking Yes.
154///
155/// A test process has no business asking the shell to move or delete a user's files. The one test
156/// that must — `copy_cut_paste_and_delete_end_to_end`, which works inside `target/sandbox` — turns
157/// this on for its duration through [`for_real`].
158#[cfg(test)]
159pub(crate) static FOR_REAL: std::sync::atomic::AtomicBool =
160    std::sync::atomic::AtomicBool::new(false);
161
162/// Let the real shell do the work for as long as the returned guard is alive.
163///
164/// A guard rather than a pair of calls, so a test that fails an assertion half way through still
165/// leaves the flag off for whatever runs next.
166#[cfg(test)]
167pub(crate) fn for_real() -> impl Drop {
168    struct Guard;
169    impl Drop for Guard {
170        fn drop(&mut self) {
171            FOR_REAL.store(false, std::sync::atomic::Ordering::SeqCst);
172        }
173    }
174    FOR_REAL.store(true, std::sync::atomic::Ordering::SeqCst);
175    Guard
176}
177
178/// Runs jobs, one thread each, and reports back.
179pub struct Operations {
180    tx: Sender<Done>,
181    rx: Receiver<Done>,
182    /// What is running, oldest first, for the status line.
183    running: Vec<String>,
184}
185
186impl Operations {
187    pub fn new() -> Self {
188        let (tx, rx) = channel();
189        Self {
190            tx,
191            rx,
192            running: Vec::new(),
193        }
194    }
195
196    /// Start a job. Returns immediately; the window stays live while it runs.
197    pub fn start(&mut self, job: Job, owner: Owner, ctx: &egui::Context) {
198        self.start_then(job, After::Nothing, owner, ctx);
199    }
200
201    /// The same, with something to do once it has finished.
202    pub fn start_then(&mut self, job: Job, after: After, owner: Owner, ctx: &egui::Context) {
203        self.running.push(job.describe());
204        let touched = job.touches();
205        let tx = self.tx.clone();
206        let ctx = ctx.clone();
207
208        // **Not from a test, unless a test asked for it.** See [`FOR_REAL`]. This is the choke
209        // point every copy, move, delete, rename and new folder goes through, which is why the
210        // guard is here and not at the five call sites.
211        #[cfg(test)]
212        if !FOR_REAL.load(std::sync::atomic::Ordering::SeqCst) {
213            let _ = tx.send(Done {
214                touched,
215                error: None,
216                after,
217                created: None,
218            });
219            return;
220        }
221
222        // Taken here and dropped on the job's own thread, so that a directory a drop claimed
223        // into goes when the job that consumes it is done — and goes there rather than on the
224        // UI thread, because removing a staged archive is real work. Built after the guard
225        // above so a test can never make one.
226        let scratch = claimed(&job).map(Scratch);
227        let spawned = std::thread::Builder::new()
228            .name("file-operation".to_owned())
229            .spawn(move || {
230                let _scratch = scratch;
231                let (error, created) = run(&job, owner);
232                let _ = tx.send(Done {
233                    touched,
234                    error,
235                    after,
236                    created,
237                });
238                ctx.request_repaint();
239            });
240        if spawned.is_err() {
241            self.running.pop();
242            let _ = self.tx.send(Done {
243                touched: Vec::new(),
244                error: Some("Could not start the operation".to_owned()),
245                after: After::Nothing,
246                created: None,
247            });
248        }
249    }
250
251    /// Anything that has finished since the last frame.
252    pub fn drain(&mut self) -> Vec<Done> {
253        let finished: Vec<Done> = self.rx.try_iter().collect();
254        for _ in 0..finished.len() {
255            if !self.running.is_empty() {
256                self.running.remove(0);
257            }
258        }
259        finished
260    }
261
262    /// What is in progress, for the status line.
263    pub fn in_progress(&self) -> Option<&str> {
264        self.running.first().map(String::as_str)
265    }
266}
267
268impl Default for Operations {
269    fn default() -> Self {
270        Self::new()
271    }
272}
273
274/// The staging directory a drop claimed its items into, if that is where they live.
275///
276/// Recognised from the path rather than carried down from the drop, so that every route which
277/// ends in a job gets the cleanup without knowing about it — including the right-button menu,
278/// which can sit open for as long as the user likes before it becomes one. What makes that safe
279/// to act on is [`crate::shell::dnd::is_staging`], which asks both for the name and for the
280/// directory to be sitting in `%TEMP%`.
281fn claimed(job: &Job) -> Option<PathBuf> {
282    let items = match job {
283        Job::Copy { items, .. } | Job::Move { items, .. } => items,
284        _ => return None,
285    };
286    let dir = items.first()?.parent()?;
287    if crate::shell::dnd::is_staging(dir) {
288        return Some(dir.to_path_buf());
289    }
290    // One level up as well, for an item that had to be nested to keep its name.
291    let up = dir.parent()?;
292    crate::shell::dnd::is_staging(up).then(|| up.to_path_buf())
293}
294
295/// A directory that goes when this does.
296///
297/// A guard rather than a statement so that a panic in the job, or a thread that never starts,
298/// cannot leak an extracted archive into `%TEMP%`.
299struct Scratch(PathBuf);
300
301impl Drop for Scratch {
302    fn drop(&mut self) {
303        let _ = std::fs::remove_dir_all(&self.0);
304    }
305}
306
307// ---------------------------------------------------------------------------
308// Windows
309// ---------------------------------------------------------------------------
310
311/// Do the work. Runs on its own thread, with its own apartment.
312#[cfg(windows)]
313fn run(job: &Job, owner: Owner) -> (Option<String>, Option<String>) {
314    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
315
316    // SAFETY: this thread exists for this call, initialises its own apartment, and
317    // uninitialises it before returning. Nothing else here touches COM.
318    unsafe {
319        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
320            return (Some("Could not talk to the shell".to_owned()), None);
321        }
322        let created: NewName = Default::default();
323        let result = perform(job, owner, &created);
324        let name = created.lock().ok().and_then(|held| held.clone());
325        CoUninitialize();
326        (result, name)
327    }
328}
329
330/// Where [`Sink`] leaves the name the shell chose.
331#[cfg(windows)]
332type NewName = std::sync::Arc<std::sync::Mutex<Option<String>>>;
333
334/// Catches the name the shell gives a new folder.
335///
336/// `IFileOperation` takes one of these per item and reports what it did with it. Only
337/// `PostNewItem` is of any interest here — the rest are the progress and per-item callbacks a
338/// copy dialog would use, and the shell is showing its own. They are here because the interface
339/// requires them, and they are all the same three lines.
340#[cfg(windows)]
341#[windows::core::implement(windows::Win32::UI::Shell::IFileOperationProgressSink)]
342struct Sink(NewName);
343
344#[cfg(windows)]
345impl windows::Win32::UI::Shell::IFileOperationProgressSink_Impl for Sink_Impl {
346    /// The one that matters: the name the shell settled on, and the item it made.
347    fn PostNewItem(
348        &self,
349        _flags: u32,
350        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
351        name: &windows_core::PCWSTR,
352        _template: &windows_core::PCWSTR,
353        _attributes: u32,
354        result: windows_core::HRESULT,
355        item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
356    ) -> windows_core::Result<()> {
357        if result.is_err() {
358            return Ok(());
359        }
360        // The item's own name for preference, since it is what the folder now holds; the
361        // `psznewname` the shell passes alongside is the same string in every case seen, and is
362        // the fallback for a shell that hands over an item this cannot name.
363        let settled = item
364            .as_ref()
365            .and_then(|item| {
366                // SAFETY: a display name read from an item the shell has just handed over.
367                unsafe {
368                    item.GetDisplayName(windows::Win32::UI::Shell::SIGDN_PARENTRELATIVEPARSING)
369                        .ok()
370                        .and_then(|wide| {
371                            let text = wide.to_string().ok();
372                            windows::Win32::System::Com::CoTaskMemFree(Some(
373                                wide.0 as *const std::ffi::c_void,
374                            ));
375                            text
376                        })
377                }
378            })
379            // SAFETY: a null-terminated string owned by the caller for the length of the call.
380            .or_else(|| unsafe { name.to_string().ok() });
381
382        if let (Some(settled), Ok(mut held)) = (settled, self.0.lock()) {
383            *held = Some(settled);
384        }
385        Ok(())
386    }
387
388    // The rest of the interface. A copy dialog would use these to draw progress and per-item
389    // state; the shell is drawing its own, so there is nothing for this program to add.
390    fn StartOperations(&self) -> windows_core::Result<()> {
391        Ok(())
392    }
393    fn FinishOperations(&self, _result: windows_core::HRESULT) -> windows_core::Result<()> {
394        Ok(())
395    }
396    fn PreRenameItem(
397        &self,
398        _flags: u32,
399        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
400        _name: &windows_core::PCWSTR,
401    ) -> windows_core::Result<()> {
402        Ok(())
403    }
404    fn PostRenameItem(
405        &self,
406        _flags: u32,
407        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
408        _name: &windows_core::PCWSTR,
409        _result: windows_core::HRESULT,
410        _made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
411    ) -> windows_core::Result<()> {
412        Ok(())
413    }
414    fn PreMoveItem(
415        &self,
416        _flags: u32,
417        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
418        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
419        _name: &windows_core::PCWSTR,
420    ) -> windows_core::Result<()> {
421        Ok(())
422    }
423    fn PostMoveItem(
424        &self,
425        _flags: u32,
426        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
427        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
428        _name: &windows_core::PCWSTR,
429        _result: windows_core::HRESULT,
430        _made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
431    ) -> windows_core::Result<()> {
432        Ok(())
433    }
434    fn PreCopyItem(
435        &self,
436        _flags: u32,
437        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
438        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
439        _name: &windows_core::PCWSTR,
440    ) -> windows_core::Result<()> {
441        Ok(())
442    }
443    fn PostCopyItem(
444        &self,
445        _flags: u32,
446        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
447        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
448        _name: &windows_core::PCWSTR,
449        _result: windows_core::HRESULT,
450        _made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
451    ) -> windows_core::Result<()> {
452        Ok(())
453    }
454    fn PreDeleteItem(
455        &self,
456        _flags: u32,
457        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
458    ) -> windows_core::Result<()> {
459        Ok(())
460    }
461    fn PostDeleteItem(
462        &self,
463        _flags: u32,
464        _item: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
465        _result: windows_core::HRESULT,
466        _made: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
467    ) -> windows_core::Result<()> {
468        Ok(())
469    }
470    fn PreNewItem(
471        &self,
472        _flags: u32,
473        _into: windows_core::Ref<windows::Win32::UI::Shell::IShellItem>,
474        _name: &windows_core::PCWSTR,
475    ) -> windows_core::Result<()> {
476        Ok(())
477    }
478    fn UpdateProgress(&self, _total: u32, _so_far: u32) -> windows_core::Result<()> {
479        Ok(())
480    }
481    fn ResetTimer(&self) -> windows_core::Result<()> {
482        Ok(())
483    }
484    fn PauseTimer(&self) -> windows_core::Result<()> {
485        Ok(())
486    }
487    fn ResumeTimer(&self) -> windows_core::Result<()> {
488        Ok(())
489    }
490}
491
492/// The body, split out so the apartment is torn down on every path out.
493#[cfg(windows)]
494unsafe fn perform(job: &Job, owner: Owner, created: &NewName) -> Option<String> {
495    use windows::core::{HSTRING, PCWSTR};
496    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY;
497    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
498    use windows::Win32::UI::Shell::{
499        FileOperation, IFileOperation, IShellItem, FILEOPERATION_FLAGS, FOFX_ADDUNDORECORD,
500        FOFX_RECYCLEONDELETE, FOFX_SHOWELEVATIONPROMPT, FOF_ALLOWUNDO, FOF_NOCONFIRMMKDIR,
501        FOF_RENAMEONCOLLISION,
502    };
503
504    let op: IFileOperation = match CoCreateInstance(&FileOperation, None, CLSCTX_ALL) {
505        Ok(op) => op,
506        Err(e) => return friendly(&e),
507    };
508
509    // Parented to this window, or the progress dialog comes up behind it and the
510    // application looks hung while it waits for an answer nobody can see.
511    if owner.0 != 0 {
512        let _ = op.SetOwnerWindow(owner.hwnd());
513    }
514
515    // `ALLOWUNDO` with `ADDUNDORECORD` puts the operation in the shell's undo stack,
516    // so Ctrl+Z in Explorer takes it back. `RECYCLEONDELETE` is the difference between
517    // Delete and Shift+Delete.
518    let mut flags = FOF_ALLOWUNDO.0 | FOFX_ADDUNDORECORD.0 | FOFX_SHOWELEVATIONPROMPT.0;
519    match job {
520        Job::Delete { to_bin: false, .. } => flags &= !FOF_ALLOWUNDO.0,
521        Job::Delete { .. } => flags |= FOFX_RECYCLEONDELETE.0,
522        Job::NewFolder { .. } => flags |= FOF_NOCONFIRMMKDIR.0,
523        // Copying something into the folder it is already in cannot mean "replace it with
524        // itself", so Explorer does not ask: it makes `one - Copy.txt` and gets on with it.
525        // `FOF_RENAMEONCOLLISION` is what produces that name — the shell's own, localised,
526        // measured to come out as `one - Copie.txt` on this machine — and without it a paste
527        // into the current folder stops on the Replace-or-Skip dialog, which is not what
528        // Ctrl+C Ctrl+V does anywhere else in Windows.
529        //
530        // Only when *every* item is already there. A batch from more than one folder that
531        // happens to include one of the destination's own is a genuine name clash for the
532        // others, and those are the user's to answer.
533        Job::Copy { items, into } if all_already_in(items, into) => {
534            flags |= FOF_RENAMEONCOLLISION.0
535        }
536        _ => {}
537    }
538    if let Err(e) = op.SetOperationFlags(FILEOPERATION_FLAGS(flags)) {
539        return friendly(&e);
540    }
541
542    let queued = match job {
543        Job::Copy { items, into } => {
544            let dest: IShellItem = match item(into) {
545                Ok(dest) => dest,
546                Err(e) => return friendly(&e),
547            };
548            queue(items, |src| op.CopyItem(src, &dest, PCWSTR::null(), None))
549        }
550        Job::Move { items, into } => {
551            let dest: IShellItem = match item(into) {
552                Ok(dest) => dest,
553                Err(e) => return friendly(&e),
554            };
555            queue(items, |src| op.MoveItem(src, &dest, PCWSTR::null(), None))
556        }
557        Job::Delete { items, .. } => queue(items, |src| op.DeleteItem(src, None)),
558        Job::Rename { item: path, name } => {
559            let src: IShellItem = match item(path) {
560                Ok(src) => src,
561                Err(e) => return friendly(&e),
562            };
563            match op.RenameItem(&src, &HSTRING::from(name.as_str()), None) {
564                Ok(()) => 1,
565                Err(e) => return friendly(&e),
566            }
567        }
568        Job::NewFolder { parent, name } => {
569            let dest: IShellItem = match item(parent) {
570                Ok(dest) => dest,
571                Err(e) => return friendly(&e),
572            };
573            // The sink is how the real name comes back; see `Sink`.
574            let sink: windows::Win32::UI::Shell::IFileOperationProgressSink =
575                Sink(created.clone()).into();
576            match op.NewItem(
577                &dest,
578                FILE_ATTRIBUTE_DIRECTORY.0,
579                &HSTRING::from(name.as_str()),
580                PCWSTR::null(),
581                Some(&sink),
582            ) {
583                Ok(()) => 1,
584                Err(e) => return friendly(&e),
585            }
586        }
587    };
588
589    if queued == 0 {
590        return Some("None of those items could be found".to_owned());
591    }
592
593    // Blocks on *this* thread while the shell shows its progress, asks about conflicts
594    // and does the work.
595    if let Err(e) = op.PerformOperations() {
596        return friendly(&e);
597    }
598    None
599}
600
601/// Whether every one of these items already lives in `into`.
602///
603/// Compared case-insensitively, because Windows paths are, and a copy of `C:\\Temp\\a.txt`
604/// into `C:\\temp` is the same copy-into-its-own-folder as any other.
605pub(crate) fn all_already_in(items: &[PathBuf], into: &Path) -> bool {
606    let same = |a: &Path, b: &Path| {
607        a.as_os_str()
608            .to_string_lossy()
609            .replace('/', "\\")
610            .eq_ignore_ascii_case(&b.as_os_str().to_string_lossy().replace('/', "\\"))
611    };
612    !items.is_empty() && items.iter().all(|i| i.parent().is_some_and(|p| same(p, into)))
613}
614
615/// Add every item to the operation, reporting how many were accepted.
616///
617/// An item that has gone missing between the click and here is skipped rather than
618/// failing the batch, which is what happens when a selection is a few seconds stale.
619#[cfg(windows)]
620unsafe fn queue(
621    items: &[PathBuf],
622    mut add: impl FnMut(&windows::Win32::UI::Shell::IShellItem) -> windows::core::Result<()>,
623) -> usize {
624    let mut queued = 0;
625    for path in items {
626        if let Ok(shell_item) = item(path) {
627            if add(&shell_item).is_ok() {
628                queued += 1;
629            }
630        }
631    }
632    queued
633}
634
635/// A path as an `IShellItem`.
636#[cfg(windows)]
637pub(crate) unsafe fn item(
638    path: &Path,
639) -> windows::core::Result<windows::Win32::UI::Shell::IShellItem> {
640    use windows::Win32::UI::Shell::SHCreateItemFromParsingName;
641    let wide = super::wide(path);
642    SHCreateItemFromParsingName(windows::core::PCWSTR(wide.as_ptr()), None)
643}
644
645/// Turn an `HRESULT` into something worth showing, or `None` when it is not worth
646/// showing at all.
647///
648/// The shell's own dialog has already explained anything the user can act on, and a
649/// cancel is a decision rather than a fault — so both come back as `None` and the
650/// status line stays quiet.
651#[cfg(windows)]
652fn friendly(error: &windows::core::Error) -> Option<String> {
653    const E_ACCESSDENIED: i32 = -2147024891; // 0x80070005
654    const ERROR_CANCELLED: i32 = -2147023673; // 0x800704C7
655    const COPYENGINE_E_USER_CANCELLED: i32 = -2144927744; // 0x80270000
656    match error.code().0 {
657        ERROR_CANCELLED | COPYENGINE_E_USER_CANCELLED => None,
658        E_ACCESSDENIED => Some("Access denied".to_owned()),
659        code => {
660            let message = error.message();
661            Some(if message.is_empty() {
662                format!("The shell refused: 0x{code:08x}")
663            } else {
664                message
665            })
666        }
667    }
668}
669
670#[cfg(not(windows))]
671fn run(_job: &Job, _owner: Owner) -> (Option<String>, Option<String>) {
672    (
673        Some("File operations are implemented against the Windows shell only".to_owned()),
674        None,
675    )
676}
677
678#[cfg(test)]
679mod tests {
680    use super::*;
681
682    #[test]
683    fn a_job_knows_which_folders_it_changes() {
684        let job = Job::Move {
685            items: vec![
686                PathBuf::from(r"C:\a\one.txt"),
687                PathBuf::from(r"C:\a\two.txt"),
688                PathBuf::from(r"C:\b\three.txt"),
689            ],
690            into: PathBuf::from(r"C:\dest"),
691        };
692        let touched = job.touches();
693        assert!(touched.contains(&PathBuf::from(r"C:\a")));
694        assert!(touched.contains(&PathBuf::from(r"C:\b")));
695        assert!(touched.contains(&PathBuf::from(r"C:\dest")));
696        assert_eq!(touched.len(), 3, "and each folder once: {touched:?}");
697    }
698
699    #[test]
700    fn a_delete_only_touches_the_sources() {
701        let job = Job::Delete {
702            items: vec![PathBuf::from(r"C:\a\one.txt")],
703            to_bin: true,
704        };
705        assert_eq!(job.touches(), [PathBuf::from(r"C:\a")]);
706    }
707
708    /// A job started the way the window starts them does not reach the shell from a test.
709    ///
710    /// The guard this checks is the one described on [`FOR_REAL`], and the reason it is worth a
711    /// test of its own is how the bug behaved: the job goes to a thread, so the test that asked
712    /// for it finished and *passed* while the shell's confirmation dialog was on screen asking
713    /// whether to permanently delete a folder out of this repository. There was nothing in any
714    /// test output to notice, which is exactly the class of failure a guard is for.
715    ///
716    /// The job named here is the one that was actually issued: `Shift+Delete` on the first row of
717    /// `CARGO_MANIFEST_DIR`.
718    #[test]
719    fn a_test_cannot_hand_a_job_to_the_shell_by_accident() {
720        use std::sync::atomic::Ordering;
721
722        assert!(
723            !FOR_REAL.load(Ordering::SeqCst),
724            "the real shell must be off unless a test has asked for it"
725        );
726
727        let ctx = egui::Context::default();
728        let mut ops = Operations::new();
729        ops.start(
730            Job::Delete {
731                items: vec![PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".cargo")],
732                to_bin: false,
733            },
734            Owner::default(),
735            &ctx,
736        );
737
738        // It reports back as a job that did nothing, rather than being dropped silently: the
739        // window takes the folders in `touched` as its cue to re-read, and a job that never
740        // answers leaves `Copying 1 item…` in the status line for the rest of the session.
741        let finished = ops.drain();
742        assert_eq!(finished.len(), 1, "the job never reported back");
743        assert!(finished[0].error.is_none(), "{:?}", finished[0].error);
744        assert!(
745            ops.in_progress().is_none(),
746            "the status line still says something is running"
747        );
748        assert!(
749            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".cargo").exists(),
750            "the guard let a permanent delete through"
751        );
752
753        // ---- Both directions, on something expendable ----------------------
754        //
755        // The delete above cannot be un-guarded to prove the guard does anything, for the
756        // obvious reason. A new folder can: it is the one job that completes without the shell
757        // asking anything, so the same call can be watched through the gate shut and open.
758        #[cfg(windows)]
759        {
760            let mut root = std::env::temp_dir();
761            root.push(format!("yafe-guard-{}", std::process::id()));
762            let _ = std::fs::remove_dir_all(&root);
763            std::fs::create_dir_all(&root).expect("temp dir");
764
765            let make = |ops: &mut Operations| {
766                ops.start(
767                    Job::NewFolder {
768                        parent: root.clone(),
769                        name: "made".to_owned(),
770                    },
771                    Owner::default(),
772                    &ctx,
773                );
774                // The work is on a thread, so the answer has to be waited for rather than
775                // assumed either way.
776                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
777                while ops.in_progress().is_some() && std::time::Instant::now() < deadline {
778                    ops.drain();
779                    std::thread::yield_now();
780                }
781                std::fs::read_dir(&root).into_iter().flatten().count()
782            };
783
784            assert_eq!(make(&mut ops), 0, "a guarded job reached the shell anyway");
785            {
786                let _for_real = for_real();
787                assert_eq!(
788                    make(&mut ops),
789                    1,
790                    "the opt-in did not reach the shell — every test that needs it is \
791                     testing nothing"
792                );
793            }
794            let _ = std::fs::remove_dir_all(&root);
795        }
796
797        assert!(
798            !FOR_REAL.load(Ordering::SeqCst),
799            "the guard has to close again when it goes out of scope"
800        );
801    }
802
803    /// The engine itself, end to end, on real files.
804    ///
805    /// Only the operations that cannot raise a dialog: a copy into an empty folder, a
806    /// rename to a free name and a new folder all complete without asking anything, so
807    /// the test finishes on its own. Delete is deliberately not exercised here — a
808    /// permanent delete prompts, and a recycle would leave litter in the user's own
809    /// Recycle Bin, which a test has no business doing.
810    #[test]
811    #[cfg(windows)]
812    fn the_shell_engine_copies_renames_and_creates() {
813        use std::time::{Duration, Instant};
814
815        let _serialised = crate::shell::serialised();
816        crate::shell::init();
817
818        let mut root = std::env::temp_dir();
819        root.push(format!("yafe-ops-{}", std::process::id()));
820        let from = root.join("from");
821        let into = root.join("into");
822        std::fs::create_dir_all(&from).expect("temp dir");
823        std::fs::create_dir_all(&into).expect("temp dir");
824        let one = from.join("one.txt");
825        std::fs::write(&one, b"one").expect("write");
826
827        /// Run a job to completion, on a thread of its own.
828        ///
829        /// On a thread of its own because that is where production runs it, and because
830        /// `run` initialises an apartment and *uninitialises* it on the way out — doing
831        /// that on the test thread would tear down the apartment every other shell test
832        /// on this thread is relying on, and the failure would surface somewhere else
833        /// entirely.
834        fn run_now(job: Job) -> Option<String> {
835            std::thread::spawn(move || super::run(&job, Owner::default()).0)
836                .join()
837                .expect("the operation thread panicked")
838        }
839
840        // ---- Copy ----
841        assert_eq!(
842            run_now(Job::Copy {
843                items: vec![one.clone()],
844                into: into.clone(),
845            }),
846            None
847        );
848        assert!(
849            into.join("one.txt").exists(),
850            "the shell should have copied the file"
851        );
852        assert!(one.exists(), "and left the original alone");
853
854        // ---- Rename ----
855        assert_eq!(
856            run_now(Job::Rename {
857                item: into.join("one.txt"),
858                name: "two.txt".to_owned(),
859            }),
860            None
861        );
862        assert!(into.join("two.txt").exists(), "renamed");
863        assert!(!into.join("one.txt").exists(), "and the old name is gone");
864
865        // ---- New folder ----
866        assert_eq!(
867            run_now(Job::NewFolder {
868                parent: into.clone(),
869                name: "made".to_owned(),
870            }),
871            None
872        );
873        // `NewItem` returns before the directory entry is necessarily visible, so this
874        // waits rather than asserting on a race.
875        let deadline = Instant::now() + Duration::from_secs(5);
876        while !into.join("made").is_dir() && Instant::now() < deadline {
877            std::thread::sleep(Duration::from_millis(20));
878        }
879        assert!(into.join("made").is_dir(), "the folder should have been made");
880
881        let _ = std::fs::remove_dir_all(&root);
882    }
883
884
885    /// What the shell actually puts on screen, and for which operation.
886    ///
887    /// A probe rather than an assertion: the answer is other people's UI. Every window this
888    /// process owns is listed before and after the operation starts, and whatever is new is
889    /// what the shell raised -- class, title, and whether it is owned by this program's
890    /// window. Each one is then closed so the run finishes on its own.
891    ///
892    /// Uses `target/sandbox`, which is expendable.
893    #[test]
894    #[ignore = "puts real shell dialogs on screen; run explicitly with --nocapture"]
895    #[cfg(windows)]
896    fn what_the_shell_puts_on_screen() {
897        use std::time::{Duration, Instant};
898
899        let _serialised = crate::shell::serialised();
900        crate::shell::init();
901
902        // Joined a component at a time: a `join("target/sandbox")` keeps the forward slashes,
903        // and `SHCreateItemFromParsingName` refuses a path that has any in it.
904        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
905            .join("target")
906            .join("sandbox")
907            .join("dialogs");
908        let _ = std::fs::remove_dir_all(&root);
909        let from = root.join("from");
910        let into = root.join("into");
911        std::fs::create_dir_all(&from).expect("sandbox");
912        std::fs::create_dir_all(&into).expect("sandbox");
913        std::fs::write(from.join("one.txt"), b"from").expect("write");
914        std::fs::write(into.join("one.txt"), b"a different one").expect("write");
915        std::fs::write(from.join("gone.txt"), b"to delete").expect("write");
916        std::fs::write(from.join("binned.txt"), b"to recycle").expect("write");
917
918        for (what, job) in [
919            (
920                "copy onto an existing name",
921                Job::Copy {
922                    items: vec![from.join("one.txt")],
923                    into: into.clone(),
924                },
925            ),
926            (
927                "permanent delete",
928                Job::Delete {
929                    items: vec![from.join("gone.txt")],
930                    to_bin: false,
931                },
932            ),
933            (
934                "recycle",
935                Job::Delete {
936                    items: vec![from.join("binned.txt")],
937                    to_bin: true,
938                },
939            ),
940        ] {
941            let before = windows_of_this_process();
942            let handle = std::thread::spawn(move || super::run(&job, Owner::default()).0);
943
944            // Watch for anything new for a couple of seconds, then shut it.
945            let mut seen: Vec<(String, String)> = Vec::new();
946            let deadline = Instant::now() + Duration::from_secs(3);
947            while Instant::now() < deadline {
948                for (hwnd, class, title) in windows_of_this_process() {
949                    if before.iter().any(|(h, _, _)| *h == hwnd) {
950                        continue;
951                    }
952                    if seen.iter().any(|(c, t)| *c == class && *t == title) {
953                        continue;
954                    }
955                    seen.push((class.clone(), title.clone()));
956                    println!("  {what}: `{title}` [{class}]");
957                    close(hwnd);
958                }
959                if handle.is_finished() {
960                    break;
961                }
962                std::thread::sleep(Duration::from_millis(50));
963            }
964            let outcome = handle.join().expect("the operation thread panicked");
965            if seen.is_empty() {
966                println!("  {what}: nothing on screen");
967            }
968            println!("  {what}: finished as {outcome:?}");
969        }
970
971        let _ = std::fs::remove_dir_all(&root);
972    }
973
974    /// Run a job on its own thread, closing any window the shell raises, and report both
975    /// what it finished as and what it put on screen.
976    #[cfg(all(test, windows))]
977    fn run_watching(job: Job) -> (Option<String>, Vec<(String, String)>) {
978        use std::time::{Duration, Instant};
979
980        let before = windows_of_this_process();
981        let handle = std::thread::spawn(move || super::run(&job, Owner::default()).0);
982        let mut seen: Vec<(String, String)> = Vec::new();
983        // Watched for a while before anything is closed. A shell operation raises a progress
984        // window of its own accord and finishes behind it; closing that on sight cancels work
985        // that was never waiting for an answer, which is how this probe first reported a
986        // same-folder copy as "interrupted".
987        let patience = Instant::now() + Duration::from_millis(1500);
988        let deadline = Instant::now() + Duration::from_secs(8);
989        while Instant::now() < deadline {
990            let mut fresh = Vec::new();
991            for (hwnd, class, title) in windows_of_this_process() {
992                if before.iter().any(|(h, _, _)| *h == hwnd) {
993                    continue;
994                }
995                if !seen.iter().any(|(c, t)| *c == class && *t == title) {
996                    seen.push((class, title));
997                }
998                fresh.push(hwnd);
999            }
1000            if handle.is_finished() {
1001                break;
1002            }
1003            if Instant::now() > patience {
1004                // Still going, so something is waiting to be answered.
1005                for hwnd in fresh {
1006                    close(hwnd);
1007                }
1008            }
1009            std::thread::sleep(Duration::from_millis(50));
1010        }
1011        (
1012            handle.join().expect("the operation thread panicked"),
1013            seen,
1014        )
1015    }
1016
1017    /// Every visible top-level window this process owns, with its class and title.
1018    #[cfg(all(test, windows))]
1019    fn windows_of_this_process() -> Vec<(isize, String, String)> {
1020        use windows::core::BOOL;
1021        use windows::Win32::Foundation::{HWND, LPARAM, TRUE};
1022        use windows::Win32::System::Threading::GetCurrentProcessId;
1023        use windows::Win32::UI::WindowsAndMessaging::{
1024            EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
1025        };
1026
1027        unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> BOOL {
1028            let found = &mut *(lparam.0 as *mut Vec<(isize, String, String)>);
1029            let mut pid = 0u32;
1030            GetWindowThreadProcessId(hwnd, Some(&mut pid));
1031            if pid != GetCurrentProcessId() || !IsWindowVisible(hwnd).as_bool() {
1032                return TRUE;
1033            }
1034            let mut class = [0u16; 256];
1035            let n = GetClassNameW(hwnd, &mut class);
1036            let mut title = [0u16; 512];
1037            let m = GetWindowTextW(hwnd, &mut title);
1038            found.push((
1039                hwnd.0 as isize,
1040                String::from_utf16_lossy(&class[..n.max(0) as usize]),
1041                String::from_utf16_lossy(&title[..m.max(0) as usize]),
1042            ));
1043            TRUE
1044        }
1045
1046        let mut found: Vec<(isize, String, String)> = Vec::new();
1047        // SAFETY: the callback only writes through the pointer it is handed, which outlives
1048        // the enumeration.
1049        unsafe {
1050            let _ = EnumWindows(Some(visit), LPARAM(&mut found as *mut _ as isize));
1051        }
1052        found
1053    }
1054
1055    /// Ask a window to go away, which for a shell dialog is a cancel.
1056    #[cfg(all(test, windows))]
1057    fn close(hwnd: isize) {
1058        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
1059        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
1060        // SAFETY: posting is asynchronous and safe against a window that has already gone.
1061        unsafe {
1062            let _ = PostMessageW(
1063                Some(HWND(hwnd as *mut std::ffi::c_void)),
1064                WM_CLOSE,
1065                WPARAM(0),
1066                LPARAM(0),
1067            );
1068        }
1069    }
1070
1071    /// A path with forward slashes in it has to work, because one can get this far.
1072    ///
1073    /// `--open=C:/Windows` is a perfectly ordinary thing to type, and `std::fs` is perfectly
1074    /// happy with it -- the listing appears, the icons are asked for, the menu is asked for.
1075    /// The shell *parses* paths rather than passing them to the kernel, and refuses a slash,
1076    /// so every one of those quietly did nothing. Normalising in `shell::wide` fixes all of
1077    /// them at once; this is the check that it stays fixed.
1078    #[test]
1079    #[cfg(windows)]
1080    fn the_shell_takes_a_path_with_forward_slashes() {
1081        let _serialised = crate::shell::serialised();
1082        crate::shell::init();
1083
1084        let root = sandbox("slashes");
1085        let one = root.join("one.txt");
1086        std::fs::write(&one, b"one").expect("write");
1087
1088        let slashed = PathBuf::from(one.to_string_lossy().replace('\\', "/"));
1089        assert!(
1090            slashed.exists(),
1091            "the slashed path has to be a real path to std::fs, or this proves nothing"
1092        );
1093        assert!(
1094            slashed.to_string_lossy().contains('/'),
1095            "and it has to actually have a slash in it: {}",
1096            slashed.display()
1097        );
1098
1099        // SAFETY: a pure lookup; nothing is retained.
1100        unsafe {
1101            assert!(
1102                item(&slashed).is_ok(),
1103                "the shell refused {} -- `wide` is not normalising separators",
1104                slashed.display()
1105            );
1106        }
1107
1108        let _ = std::fs::remove_dir_all(&root);
1109    }
1110
1111    #[test]
1112    fn what_counts_as_a_copy_into_its_own_folder() {
1113        let here = PathBuf::from(r"C:\Temp");
1114        assert!(all_already_in(&[here.join("a.txt")], &here));
1115        // Windows paths are case-insensitive and so is this.
1116        assert!(all_already_in(&[PathBuf::from(r"C:\TEMP\a.txt")], &here));
1117        // A slashed destination is the same destination.
1118        assert!(all_already_in(&[here.join("a.txt")], Path::new("C:/Temp")));
1119        // One item from somewhere else makes it a real name clash, which the user answers.
1120        assert!(!all_already_in(
1121            &[here.join("a.txt"), PathBuf::from(r"C:\Other\a.txt")],
1122            &here
1123        ));
1124        assert!(!all_already_in(&[], &here), "and nothing is not a copy");
1125    }
1126
1127    /// Ctrl+C then Ctrl+V in the same folder, which is the one collision Explorer does not
1128    /// ask about.
1129    ///
1130    /// The name is the shell's and it is localised -- `one - Copy.txt` in English, measured as
1131    /// `one - Copie.txt` here -- so what is asserted is that a second file appeared and that
1132    /// nothing was put on screen to get it.
1133    #[test]
1134    #[cfg(windows)]
1135    fn a_copy_into_its_own_folder_renames_rather_than_asking() {
1136        let _serialised = crate::shell::serialised();
1137        crate::shell::init();
1138
1139        let root = sandbox("same-folder");
1140        let one = root.join("one.txt");
1141        std::fs::write(&one, b"one").expect("write");
1142
1143        let (outcome, on_screen) = run_watching(Job::Copy {
1144            items: vec![one.clone()],
1145            into: root.clone(),
1146        });
1147        assert_eq!(outcome, None, "the copy should have gone through");
1148        assert!(
1149            on_screen.is_empty(),
1150            "nothing should have been asked, and this came up: {on_screen:?}"
1151        );
1152
1153        let mut names: Vec<String> = std::fs::read_dir(&root)
1154            .expect("read the folder back")
1155            .filter_map(|e| e.ok())
1156            .map(|e| e.file_name().to_string_lossy().into_owned())
1157            .collect();
1158        names.sort();
1159        assert_eq!(names.len(), 2, "expected two files, found {names:?}");
1160        assert!(names.contains(&"one.txt".to_owned()), "{names:?}");
1161        let copy = names.iter().find(|n| *n != "one.txt").expect("the copy");
1162        assert!(
1163            copy.starts_with("one ") && copy.ends_with(".txt"),
1164            "the shell named the copy `{copy}`, which does not look like Explorer's"
1165        );
1166
1167        let _ = std::fs::remove_dir_all(&root);
1168    }
1169
1170    /// A fresh, empty folder under `target/sandbox`, which is expendable.
1171    #[cfg(all(test, windows))]
1172    fn sandbox(name: &str) -> PathBuf {
1173        // Joined a component at a time: `join("target/sandbox")` would keep the forward
1174        // slashes, and half of what is tested here is about exactly that.
1175        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
1176            .join("target")
1177            .join("sandbox")
1178            .join(name);
1179        let _ = std::fs::remove_dir_all(&root);
1180        std::fs::create_dir_all(&root).expect("sandbox");
1181        root
1182    }
1183
1184    #[test]
1185    fn descriptions_count_and_name_the_operation() {
1186        assert_eq!(
1187            Job::Delete {
1188                items: vec![PathBuf::from("x")],
1189                to_bin: true
1190            }
1191            .describe(),
1192            "Recycling 1 item…"
1193        );
1194        assert_eq!(
1195            Job::Delete {
1196                items: vec![PathBuf::from("x"), PathBuf::from("y")],
1197                to_bin: false
1198            }
1199            .describe(),
1200            "Deleting 2 items…"
1201        );
1202        assert_eq!(
1203            Job::Copy {
1204                items: vec![PathBuf::from("x"), PathBuf::from("y")],
1205                into: PathBuf::from("z")
1206            }
1207            .describe(),
1208            "Copying 2 items…"
1209        );
1210    }
1211}
