1//! Drag and drop, both ways, through OLE.
2//!
3//! The same mechanism Explorer uses, which is what makes it interoperate: files
4//! dragged out of this window land in Explorer, in an archiver, in an editor's tab
5//! bar, in an upload field; files dragged in from any of those arrive here. Nothing
6//! about it is specific to this program.
7//!
8//! # Dragging out
9//!
10//! [`drag_out`] builds the shell's own data object for the selection and hands it to
11//! `DoDragDrop`, with a tiny `IDropSource` to answer the two questions OLE asks during
12//! a drag — has the gesture been abandoned, and what cursor to show. `DoDragDrop` is
13//! modal: it runs its own message loop and does not return until the drop happens or the
14//! drag is cancelled, so whichever thread calls it does nothing else until then. It is not
15//! the UI thread, for the reason set out on [`Drag`]: a window that cannot paint during a
16//! drag cannot show what the drag is about to do.
17//!
18//! # Dropping in
19//!
20//! winit already registers a drop target on the window to produce its own
21//! `HoveredFile` / `DroppedFile` events, and those events carry only paths — no
22//! modifier state, so no way to tell a copy from a move, and no way to answer with an
23//! effect so the cursor shows what will happen. That is most of what a drop *is*.
24//!
25//! So [`Zone::attach`] revokes winit's target and registers this one, which answers
26//! `DragOver` with a real `DROPEFFECT` and reports the modifiers on `Drop`. The rule it
27//! applies is Explorer's:
28//!
29//! | | |
30//! | --- | --- |
31//! | `Ctrl` held | copy |
32//! | `Shift` held | move |
33//! | neither, same volume | move |
34//! | neither, different volume | copy |
35//! | neither, source under `%TEMP%` | copy — see [`under_temp`] |
36//!
37//! Whatever comes out of that is then narrowed to what the source said it would allow.
38//! `pdwEffect` arrives holding the effects the source passed to `DoDragDrop`, and answering
39//! with one that is not among them is not a harmless liberty: `DROPEFFECT_MOVE` returned to a
40//! source is an *instruction* to delete what it handed over.
41//!
42//! The callbacks arrive on the UI thread from inside winit's message pump, where the
43//! application state is not reachable — so they read and write a small shared block
44//! instead, which the frame loop publishes into and drains from.
45
46use std::path::PathBuf;
47use std::sync::{Arc, Mutex};
48
49use crate::shell::clipboard::Effect;
50
51/// What a zone does with whatever lands on it.
52#[derive(Clone, Debug, PartialEq, Eq)]
53pub enum Onto {
54    /// Copy or move the items into this folder.
55    Folder(PathBuf),
56    /// Pin them in the sidebar. Nothing is copied and nothing is moved, which is why it
57    /// reports itself to the pointer as a link — the same answer Explorer gives when you
58    /// drag a folder onto Quick Access.
59    Bookmarks,
60}
61
62/// What a completed drop asks for.
63#[derive(Clone, Debug)]
64pub struct Dropped {
65    pub items: Vec<PathBuf>,
66    pub effect: Effect,
67    /// Where it landed, in physical pixels.
68    pub at: (i32, i32),
69    /// What the zone under it was for.
70    pub onto: Onto,
71    /// Whether the *right* button carried the drag, which in Windows means "ask me what to do
72    /// with it" rather than "do the obvious thing".
73    ///
74    /// Read from the last `DragOver` rather than from the drop, because by the time `Drop` is
75    /// called the button has been released and its bit is gone.
76    pub asked: bool,
77}
78
79/// Where a drop would go, published by the frame loop for the drop target to read.
80///
81/// The target callbacks cannot reach the application, and they have to answer
82/// `DragOver` *immediately* with an effect — so the answer has to already be here.
83#[derive(Clone, Default)]
84pub struct Targets {
85    /// Each droppable region and what it is for, in physical pixels, back to front.
86    pub zones: Vec<((i32, i32, i32, i32), Onto)>,
87}
88
89impl Targets {
90    /// What is at a point, if anything.
91    pub fn at(&self, (x, y): (i32, i32)) -> Option<&Onto> {
92        self.zones
93            .iter()
94            .rev()
95            .find(|((l, t, r, b), _)| x >= *l && x < *r && y >= *t && y < *b)
96            .map(|(_, onto)| onto)
97    }
98}
99
100/// The block the OLE callbacks and the frame loop share.
101#[derive(Default)]
102pub struct Shared {
103
104    /// Published by the frame loop.
105    pub targets: Targets,
106    /// Whether the right button was down the last time the drag was seen moving.
107    pub right_button: bool,
108    /// Where a drag is hovering, for the frame loop to highlight.
109    pub hovering: Option<(i32, i32)>,
110    /// Completed drops, waiting to be acted on.
111    pub dropped: Vec<Dropped>,
112}
113
114/// The receiving side.
115pub struct Zone {
116    shared: Arc<Mutex<Shared>>,
117    #[cfg(windows)]
118    registered: bool,
119    #[cfg(windows)]
120    hwnd: isize,
121}
122
123impl Zone {
124    pub fn new() -> Self {
125        Self {
126            shared: Arc::new(Mutex::new(Shared::default())),
127            #[cfg(windows)]
128            registered: false,
129            #[cfg(windows)]
130            hwnd: 0,
131        }
132    }
133
134    /// Tell the target where drops may land this frame.
135    /// What a drop at this point would be for, as the OLE callbacks see it.
136    ///
137    /// For tests: the callbacks answer from the published zones on another stack entirely, and
138    /// this is the only way to ask them the same question from here.
139    #[cfg(test)]
140    pub fn resolve(&self, at: (i32, i32)) -> Option<Onto> {
141        self.shared.lock().ok()?.targets.at(at).cloned()
142    }
143
144    pub fn publish(&self, targets: Targets) {
145        if let Ok(mut shared) = self.shared.lock() {
146            shared.targets = targets;
147        }
148    }
149
150    /// Where a drag is currently hovering, for the highlight.
151    pub fn hovering(&self) -> Option<(i32, i32)> {
152        self.shared.lock().ok().and_then(|shared| shared.hovering)
153    }
154
155    /// Take any completed drops.
156    pub fn take_drops(&self) -> Vec<Dropped> {
157        self.shared
158            .lock()
159            .map(|mut shared| std::mem::take(&mut shared.dropped))
160            .unwrap_or_default()
161    }
162
163    /// Register on the window, replacing the one winit installed.
164    ///
165    /// Idempotent, and safe to call before the window exists — it does nothing until
166    /// there is a handle.
167    pub fn attach(&mut self, owner: super::Owner) {
168        #[cfg(windows)]
169        {
170            if self.registered || owner.0 == 0 {
171                return;
172            }
173            use windows::Win32::System::Ole::{IDropTarget, RegisterDragDrop, RevokeDragDrop};
174
175            let target: IDropTarget = win::Target::new(self.shared.clone(), owner.0).into();
176            // SAFETY: winit registered its own target on this window; ours replaces it.
177            // OLE takes a reference of its own, and the local one is deliberately leaked
178            // so the target outlives this scope — `Drop` revokes it, which releases it.
179            unsafe {
180                let _ = RevokeDragDrop(owner.hwnd());
181                if RegisterDragDrop(owner.hwnd(), &target).is_ok() {
182                    self.registered = true;
183                    self.hwnd = owner.0;
184                    std::mem::forget(target);
185                }
186            }
187        }
188        #[cfg(not(windows))]
189        let _ = owner;
190    }
191}
192
193impl Default for Zone {
194    fn default() -> Self {
195        Self::new()
196    }
197}
198
199#[cfg(windows)]
200impl Drop for Zone {
201    fn drop(&mut self) {
202        if self.registered {
203            use windows::Win32::Foundation::HWND;
204            use windows::Win32::System::Ole::RevokeDragDrop;
205            // SAFETY: registered by `attach` on this handle, revoked once.
206            let _ = unsafe { RevokeDragDrop(HWND(self.hwnd as *mut std::ffi::c_void)) };
207        }
208    }
209}
210
211/// A drag this program started, in flight on an apartment of its own.
212///
213/// `DoDragDrop` is modal — it runs its own message loop and does not return until the drop
214/// lands or the gesture is abandoned — so the thread that calls it does nothing else for the
215/// length of the drag. On the UI thread that means no frames: not one repaint reaches the
216/// window while a drag started *inside* it is running, so the row a drop would land in could
217/// not be highlighted and a file selected by the drag itself could not be seen to be selected.
218/// Neither is cosmetic. They are the only feedback the gesture has.
219///
220/// So it runs here instead, and the UI thread keeps painting underneath it. The drop target is
221/// registered in the UI thread's apartment, and OLE marshals the callbacks back into it — the
222/// same machinery that lets Explorer call into this process at all — so the highlight is driven
223/// by the same `DragOver` that answers the cursor.
224pub struct Drag {
225    done: std::sync::mpsc::Receiver<Option<Effect>>,
226}
227
228impl Drag {
229    /// What the target did with the files, once the drag has ended.
230    ///
231    /// `None` while it is still in flight. `Some(None)` for a drag that was abandoned, or
232    /// dropped somewhere that took nothing.
233    pub fn finished(&self) -> Option<Option<Effect>> {
234        match self.done.try_recv() {
235            Ok(effect) => Some(effect),
236            Err(std::sync::mpsc::TryRecvError::Empty) => None,
237            // The thread went away without answering. Still an ended drag, and leaving the
238            // handle in place would wedge every later one.
239            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(None),
240        }
241    }
242
243    /// A drag with nothing behind it, and the end of the wire to finish it from.
244    ///
245    /// For the test that a drag in flight keeps the window painting — the property this whole
246    /// arrangement exists for, and one no unit test could reach if the only way to have a drag
247    /// were to hold a real mouse button down.
248    #[cfg(test)]
249    pub fn pretend() -> (Self, std::sync::mpsc::Sender<Option<Effect>>) {
250        let (tx, done) = std::sync::mpsc::channel();
251        (Self { done }, tx)
252    }
253}
254
255/// Pick these files up and start dragging them.
256///
257/// Returns as soon as the drag is under way. Ask the handle for the outcome — a move has taken
258/// the files out of the folder they were in, so the source needs re-reading.
259pub fn drag_out(items: Vec<PathBuf>) -> Option<Drag> {
260    if items.is_empty() {
261        return None;
262    }
263    #[cfg(windows)]
264    {
265        // The thread that owns the window and its input, for the attachment below.
266        let ui_thread = unsafe {
267            windows::Win32::System::Threading::GetCurrentThreadId()
268        };
269        let (tx, done) = std::sync::mpsc::channel();
270        std::thread::Builder::new()
271            .name("drag-source".to_owned())
272            .spawn(move || {
273                // This thread's own apartment: the data object, the drop source and the modal
274                // loop all belong to it.
275                crate::shell::init();
276                let _ = tx.send(win::drag_out(&items, ui_thread));
277            })
278            .ok()?;
279        Some(Drag { done })
280    }
281    #[cfg(not(windows))]
282    {
283        let _ = items;
284        None
285    }
286}
287
288/// Explorer's rule for what an unmodified drag means.
289///
290/// Within a volume a drag moves; across volumes it copies. Which is not arbitrary — a
291/// move within a volume is a rename of a directory entry and effectively free, while
292/// across volumes it is a copy followed by a delete, and defaulting to that would make
293/// an accidental drag both slow and destructive.
294pub fn default_effect(source: Option<&std::path::Path>, target: &std::path::Path) -> Effect {
295    let volume = |path: &std::path::Path| -> Option<String> {
296        let text = path.to_string_lossy();
297        // A drive letter, or the `\\server\share` of a UNC path.
298        if text.len() >= 2 && text.as_bytes()[1] == b':' {
299            return Some(text[..2].to_lowercase());
300        }
301        if let Some(rest) = text.strip_prefix(r"\\") {
302            let mut parts = rest.split(['\\', '/']);
303            let server = parts.next()?;
304            let share = parts.next()?;
305            return Some(format!(r"\\{server}\{share}").to_lowercase());
306        }
307        None
308    };
309    match (source.and_then(volume), volume(target)) {
310        (Some(from), Some(to)) if from == to => Effect::Move,
311        _ => Effect::Copy,
312    }
313}
314
315/// Whether a path is inside the user's temporary directory.
316///
317/// This is the signal that what a source is offering is a *materialisation* rather than the
318/// user's own files: the contents of an archive, a mail attachment, anything a source had to
319/// unpack somewhere before it had a path to put in a `CF_HDROP` at all. It deletes them again
320/// once the drag is over, so the same-volume rule above — which would call a drop into any
321/// folder on `C:` a move, `%TEMP%` being on `C:` — is answering a question the user cannot
322/// have asked. Dragging the contents of a 7-Zip archive into a folder reported
323/// `DROPEFFECT_MOVE` to 7-Zip, which took it as leave to delete its extraction, and it did so
324/// while the copy was still reading out of it.
325///
326/// Only the *default* is decided here. `Shift` still asks for a move and still gets one, if
327/// the source allows one.
328pub fn under_temp(path: &std::path::Path) -> bool {
329    use std::path::{Path, PathBuf};
330
331    let temp = std::env::temp_dir();
332    if temp.as_os_str().is_empty() {
333        return false;
334    }
335    // Case-folded, because `starts_with` is not; and still component-wise through it, so
336    // `C:\Temporary` is not taken for something inside `C:\Temp`.
337    let fold = |path: &Path| PathBuf::from(path.to_string_lossy().to_lowercase());
338    if fold(path).starts_with(fold(&temp)) {
339        return true;
340    }
341    // `%TEMP%` is an 8.3 short path on some machines while `CF_HDROP` carries the long form.
342    // They are the same directory and only resolving both shows it. Worth a pair of syscalls
343    // because this runs once per drag, on `DragEnter`, and not once per mouse move.
344    match (std::fs::canonicalize(path), std::fs::canonicalize(&temp)) {
345        (Ok(path), Ok(temp)) => fold(&path).starts_with(fold(&temp)),
346        _ => false,
347    }
348}
349
350/// What marks a staging directory as this program's own. See [`claim`].
351const STAGING: &str = "yafe-drop-";
352
353/// Take a source's temporary files, before it takes them back.
354///
355/// The problem this solves is one of timing and cannot be solved by being quick. A source that
356/// materialises its data — an archiver — deletes it again as soon as `DoDragDrop` returns,
357/// which is as soon as `IDropTarget::Drop` returns. So the whole of the copy would have to
358/// happen inside `Drop`, on the UI thread, with the window unable to paint: the drop of a large
359/// archive would freeze everything, and no amount of scoping helps, because a thread stuck
360/// inside a callback cannot paint one tab and not another.
361///
362/// What it *can* do inside `Drop` is stop being the source's problem. The files are in `%TEMP%`
363/// by definition — that is what [`under_temp`] established — so a staging directory made
364/// alongside them is on the same volume, and moving them into it is a directory-entry rename:
365/// microseconds, whatever the archive weighs. The source is then welcome to delete a folder
366/// that is empty, and the copy to the real destination runs on the ops thread like every other
367/// one, with the window live and every tab usable.
368///
369/// It also makes the right-button menu safe, which it could not otherwise be: `Copy here` is
370/// answered whenever the user gets round to it, long after any source has cleaned up.
371///
372/// Non-temporary items are returned untouched — the same rename applied to a drag from
373/// Explorer would move the user's actual files into a scratch folder. That is the whole reason
374/// the test is narrow.
375pub(crate) fn claim(items: Vec<PathBuf>) -> Vec<PathBuf> {
376    if !items.first().is_some_and(|first| under_temp(first)) {
377        return items;
378    }
379    let Some(staging) = staging() else {
380        return items;
381    };
382    items
383        .into_iter()
384        .enumerate()
385        .map(|(index, item)| claim_one(&staging, index, item))
386        .collect()
387}
388
389/// One item into the staging directory, under its own name.
390///
391/// The name has to survive: `IFileOperation` names what it copies after the source, so an item
392/// staged under a different name would arrive at the destination with it.
393fn claim_one(staging: &std::path::Path, index: usize, item: PathBuf) -> PathBuf {
394    let Some(name) = item.file_name() else {
395        return item;
396    };
397    let mut to = staging.join(name);
398    // Two items with one name, which means they came from different folders. Resolved with a
399    // folder rather than a suffix, for the reason above.
400    if to.exists() {
401        let nested = staging.join(index.to_string());
402        if std::fs::create_dir_all(&nested).is_err() {
403            return item;
404        }
405        to = nested.join(name);
406    }
407    match std::fs::rename(&item, &to) {
408        Ok(()) => to,
409        // Nothing moved, so the original is still the right answer — and still a race.
410        Err(_) => item,
411    }
412}
413
414/// A directory of this program's own, directly inside `%TEMP%` — so on the same volume as
415/// anything [`claim`] will put in it, which is what makes the claim a rename.
416fn staging() -> Option<PathBuf> {
417    use std::sync::atomic::{AtomicU32, Ordering};
418
419    static NEXT: AtomicU32 = AtomicU32::new(0);
420    let temp = std::env::temp_dir();
421    // Bounded rather than `loop`: if something is answering `AlreadyExists` to every name this
422    // can produce, the answer is to give up and let the drop go on unclaimed.
423    for _ in 0..64 {
424        let next = NEXT.fetch_add(1, Ordering::Relaxed);
425        let dir = temp.join(format!("{STAGING}{}-{next}", std::process::id()));
426        match std::fs::create_dir(&dir) {
427            Ok(()) => return Some(dir),
428            Err(why) if why.kind() == std::io::ErrorKind::AlreadyExists => continue,
429            Err(_) => return None,
430        }
431    }
432    None
433}
434
435/// Whether a directory is a staging directory belonging to *this* process.
436///
437/// This is the test that authorises a recursive delete — `crate::shell::ops` removes the
438/// directory a job's items were claimed into once the job is done — so it asks for both halves:
439/// the name, and that the thing is sitting directly in the temporary directory. Neither on its
440/// own would be enough to be trusted with `remove_dir_all`.
441pub(crate) fn is_staging(dir: &std::path::Path) -> bool {
442    if dir.parent() != Some(std::env::temp_dir().as_path()) {
443        return false;
444    }
445    let Some(name) = dir.file_name().and_then(|name| name.to_str()) else {
446        return false;
447    };
448    let Some(rest) = name.strip_prefix(STAGING) else {
449        return false;
450    };
451    let Some((pid, _)) = rest.split_once('-') else {
452        return false;
453    };
454    pid.parse::<u32>().is_ok_and(|pid| pid == std::process::id())
455}
456
457/// Remove staging directories left behind by a run that is over.
458///
459/// Called once at startup. Nothing should ever be left — the job that consumes a claim takes
460/// the directory with it — but being killed between the claim and the copy would otherwise
461/// leave an extracted archive in `%TEMP%` for good. A directory belonging to a process that is
462/// still running is left alone, which is the safe way round: another window may be copying out
463/// of it, and that is the exact bug all of this exists to fix.
464pub fn sweep() {
465    let temp = std::env::temp_dir();
466    let Ok(entries) = std::fs::read_dir(&temp) else {
467        return;
468    };
469    for path in entries.flatten().map(|entry| entry.path()) {
470        let Some(rest) = path
471            .file_name()
472            .and_then(|name| name.to_str())
473            .and_then(|name| name.strip_prefix(STAGING))
474        else {
475            continue;
476        };
477        let Some(pid) = rest.split_once('-').and_then(|(pid, _)| pid.parse::<u32>().ok()) else {
478            continue;
479        };
480        if pid == std::process::id() || running(pid) {
481            continue;
482        }
483        let _ = std::fs::remove_dir_all(&path);
484    }
485}
486
487/// Whether a process id is still in use.
488///
489/// A handle that opens means yes, and a recycled id means yes as well — both answers keep the
490/// directory, which is the harmless mistake to make. Only an id nothing answers to gets swept.
491#[cfg(windows)]
492fn running(pid: u32) -> bool {
493    use windows::Win32::Foundation::CloseHandle;
494    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
495
496    // SAFETY: a query for a handle that is closed again immediately.
497    unsafe {
498        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
499            Ok(handle) => {
500                let _ = CloseHandle(handle);
501                true
502            }
503            Err(_) => false,
504        }
505    }
506}
507
508/// Nothing is ever claimed off Windows, so nothing is ever swept.
509#[cfg(not(windows))]
510fn running(_pid: u32) -> bool {
511    true
512}
513
514#[cfg(windows)]
515mod win {
516    use super::*;
517    use windows::core::{implement, Ref, BOOL, HRESULT};
518    use windows::Win32::Foundation::{POINTL, S_OK};
519    use windows::Win32::System::Com::IDataObject;
520    use windows::Win32::System::Ole::{
521        DoDragDrop, IDropSource, IDropSource_Impl, IDropTarget, IDropTarget_Impl, DROPEFFECT,
522        DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
523    };
524    use windows::Win32::System::SystemServices::{
525        MK_CONTROL, MK_LBUTTON, MK_RBUTTON, MK_SHIFT, MODIFIERKEYS_FLAGS,
526    };
527
528    /// The three `DRAGDROP_S_*` values a source returns. Success codes, not errors,
529    /// which is why they are spelled out rather than gone looking for.
530    const DRAGDROP_S_DROP: HRESULT = HRESULT(0x0004_0100u32 as i32);
531    const DRAGDROP_S_CANCEL: HRESULT = HRESULT(0x0004_0101u32 as i32);
532    const DRAGDROP_S_USEDEFAULTCURSORS: HRESULT = HRESULT(0x0004_0102u32 as i32);
533
534    // ---- Dragging out --------------------------------------------------
535
536    /// The source half of a drag: the two questions OLE asks while one is running.
537    #[implement(IDropSource)]
538    struct Source;
539
540    impl IDropSource_Impl for Source_Impl {
541        fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
542            if escape.as_bool() {
543                return DRAGDROP_S_CANCEL;
544            }
545            // The drag ends when the button that started it comes up. Both are checked
546            // because a right-drag is a legitimate gesture — it is what produces
547            // Explorer's "copy here / move here / create shortcut" menu on drop.
548            if keys.0 & (MK_LBUTTON.0 | MK_RBUTTON.0) == 0 {
549                return DRAGDROP_S_DROP;
550            }
551            S_OK
552        }
553
554        fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
555            // Let OLE show the standard copy, move and no-entry cursors rather than
556            // inventing a set that would not match anything else on the desktop.
557            DRAGDROP_S_USEDEFAULTCURSORS
558        }
559    }
560
561    /// Run the drag to its end. Called on the drag's own thread, never on the UI one.
562    ///
563    /// `ui_thread` owns the window the gesture started in, and its input queue is the one that
564    /// knows the button is down. `DoDragDrop` reads that state to decide which button it is
565    /// following and when to stop following it, and on Windows key state and mouse capture are
566    /// per *input queue* rather than per process — so from a fresh thread it would see no button
567    /// held and end the drag before the pointer had moved. `AttachThreadInput` joins the two
568    /// queues for the length of the drag, which is what makes the capture and the button state
569    /// reachable from here. It is undone on the way out, including when the drag fails.
570    pub fn drag_out(items: &[PathBuf], ui_thread: u32) -> Option<Effect> {
571        use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
572
573        if items.is_empty() {
574            return None;
575        }
576        let data = data_object(items)?;
577        let source: IDropSource = Source.into();
578
579        /// Undoes the attachment however this function leaves.
580        struct Attached(u32, u32);
581        impl std::ops::Drop for Attached {
582            fn drop(&mut self) {
583                // SAFETY: undoes exactly the attachment made below, once.
584                let _ = unsafe { AttachThreadInput(self.0, self.1, false) };
585            }
586        }
587        // SAFETY: both threads are alive for the length of the drag — this one by definition,
588        // the UI one because it is the one waiting on the outcome.
589        let attached = unsafe {
590            let mine = GetCurrentThreadId();
591            AttachThreadInput(mine, ui_thread, true)
592                .as_bool()
593                .then(|| Attached(mine, ui_thread))
594        };
595
596        let mut effect = DROPEFFECT_NONE;
597        // SAFETY: both interfaces outlive the call, and `DoDragDrop` runs its own modal
598        // loop on the calling thread — which is the one holding the apartment.
599        let hr = unsafe {
600            DoDragDrop(
601                &data,
602                &source,
603                // `LINK` as well, because this window has a target of its own that answers with
604                // it: the Bookmarks group, which pins a folder rather than copying it. A target
605                // returning an effect the source never offered is a target OLE refuses, so
606                // leaving it out made dragging a folder onto Bookmarks do nothing at all.
607                DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
608                &mut effect,
609            )
610        };
611        drop(attached);
612        if hr != DRAGDROP_S_DROP {
613            return None;
614        }
615        if effect.0 & DROPEFFECT_MOVE.0 != 0 {
616            Some(Effect::Move)
617        } else if effect.0 & DROPEFFECT_COPY.0 != 0 {
618            Some(Effect::Copy)
619        } else {
620            None
621        }
622    }
623
624    /// The shell's own data object for a selection, so a target gets every format
625    /// Explorer would have offered rather than only the one this program knows about.
626    fn data_object(items: &[PathBuf]) -> Option<IDataObject> {
627        use windows::core::PCWSTR;
628        use windows::Win32::UI::Shell::Common::ITEMIDLIST;
629        use windows::Win32::UI::Shell::{
630            SHCreateShellItemArrayFromIDLists, SHParseDisplayName, BHID_DataObject,
631        };
632
633        struct Pidl(*mut ITEMIDLIST);
634        impl std::ops::Drop for Pidl {
635            fn drop(&mut self) {
636                if !self.0.is_null() {
637                    // SAFETY: allocated by `SHParseDisplayName`, freed once.
638                    unsafe { windows::Win32::UI::Shell::ILFree(Some(self.0)) };
639                }
640            }
641        }
642
643        let pidls: Vec<Pidl> = items
644            .iter()
645            .filter_map(|path| {
646                let wide = crate::shell::wide(path);
647                let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
648                // SAFETY: `wide` is null-terminated and outlives the call.
649                let ok = unsafe {
650                    SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut raw, 0, None).is_ok()
651                };
652                (ok && !raw.is_null()).then_some(Pidl(raw))
653            })
654            .collect();
655        if pidls.is_empty() {
656            return None;
657        }
658        let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.0 as *const _).collect();
659
660        // SAFETY: the PIDLs outlive the array, which copies what it needs.
661        unsafe {
662            let array = SHCreateShellItemArrayFromIDLists(&raw).ok()?;
663            array.BindToHandler(None, &BHID_DataObject).ok()
664        }
665    }
666
667    // ---- Dropping in ---------------------------------------------------
668
669    /// The receiving half. Every method runs on the UI thread, inside the window's
670    /// message pump, where the application is not reachable — so all it does is read and
671    /// write the shared block.
672    #[implement(IDropTarget)]
673    pub struct Target {
674        shared: Arc<Mutex<Shared>>,
675        /// What the drag in flight is carrying, read between `DragEnter` and `Drop` so the
676        /// effect rules can see where it came from.
677        held: Mutex<Option<Incoming>>,
678        /// The window this is registered on, for turning a screen point into a client one.
679        /// Held here rather than in `shared` because the callbacks convert points while
680        /// holding that lock, and a `Mutex` is not reentrant.
681        hwnd: isize,
682    }
683
684    impl Target {
685        pub fn new(shared: Arc<Mutex<Shared>>, hwnd: isize) -> Self {
686            Self {
687                shared,
688                held: Mutex::new(None),
689                hwnd,
690            }
691        }
692    }
693
694    /// What a drag is carrying, as far as it can be known *while it is still moving* — read
695    /// once, when it arrives, to answer `DragOver` with.
696    ///
697    /// `GetData` is not a getter. It is a request that the source *render* what it is
698    /// offering, and a source may do arbitrary work to answer one — an archiver renders
699    /// `CF_HDROP` by extracting files to a temporary folder, because until it has it has no
700    /// paths to put in one. This was being called from `effect_at`, which runs on every
701    /// `DragOver`, so a drag crossing the window asked the source to render its data dozens of
702    /// times a second.
703    ///
704    /// Which cuts the other way too, and is why this is not what the drop then acts on: a
705    /// source is entitled to have nothing to give until the drop is real, and answering a
706    /// speculative request during the drag is the part it is allowed to skip. `Drop` asks
707    /// again for that reason and falls back to this only if the second answer is empty.
708    struct Incoming {
709        /// Every path the data object offered, via `CF_HDROP`. Empty is not an error — see
710        /// above.
711        items: Vec<PathBuf>,
712        /// Whether those paths are a temporary the source is going to take back — see
713        /// [`super::under_temp`]. Decided from the first path: a data object carrying files
714        /// from two places at once is not a thing any source produces.
715        temporary: bool,
716    }
717
718    impl Incoming {
719        fn read(data: Option<&IDataObject>) -> Self {
720            let items = data.and_then(paths_of).unwrap_or_default();
721            let temporary = items.first().is_some_and(|first| super::under_temp(first));
722            Self { items, temporary }
723        }
724    }
725
726    /// The nearest thing to `wanted` that the source is willing to allow.
727    ///
728    /// `pdwEffect` is in/out on all three callbacks: on the way in it holds the effects the
729    /// source passed to `DoDragDrop`. Answering with one that is not in that set was how
730    /// dragging the contents of an archive into a folder on the same volume told the archiver
731    /// to delete its extraction — the drag had only ever been offered as a copy.
732    ///
733    /// A move degrades to a copy and never the other way round. The fallback for an effect the
734    /// source will not allow has to be the one that destroys nothing, which is also why
735    /// pinning — which copies nothing at all — may report a copy but must never report a move.
736    fn permitted(wanted: DROPEFFECT, allowed: DROPEFFECT) -> DROPEFFECT {
737        // A source that fills this in as nothing has told us nothing, rather than that it
738        // refuses every drop. Taken as no restriction, which is what ignoring the field
739        // altogether amounted to.
740        if allowed == DROPEFFECT_NONE {
741            return wanted;
742        }
743        let offers = |effect: DROPEFFECT| allowed.0 & effect.0 != 0;
744        match wanted {
745            asked if offers(asked) => asked,
746            DROPEFFECT_MOVE | DROPEFFECT_LINK if offers(DROPEFFECT_COPY) => DROPEFFECT_COPY,
747            _ => DROPEFFECT_NONE,
748        }
749    }
750
751    impl Target_Impl {
752        /// A screen point in the window's own coordinates, which is what the zones are in.
753        ///
754        /// `IDropTarget` is handed **screen** coordinates and [`Targets`] is published in the
755        /// window's own. They were compared directly, and the only reason anything worked at all
756        /// is that the two overlap when a window sits near the top left of the screen: a drop
757        /// resolved to whichever zone the *screen* point happened to fall in, which was almost
758        /// always the whole pane rather than the folder row under the pointer. So a file dropped
759        /// on a folder went into the folder already being shown, where it was filtered out as a
760        /// no-op — and dragging appeared to do nothing whatsoever.
761        ///
762        /// The handle is the target's own and not the shared block's, deliberately: the callbacks
763        /// below call this *while holding* that lock, and a `Mutex` is not reentrant.
764        fn in_client(&self, pt: &POINTL) -> (i32, i32) {
765            use windows::Win32::Foundation::{HWND, POINT};
766            use windows::Win32::Graphics::Gdi::ScreenToClient;
767
768            if self.hwnd == 0 {
769                return (pt.x, pt.y);
770            }
771            let hwnd = self.hwnd;
772            let mut point = POINT { x: pt.x, y: pt.y };
773            // SAFETY: a coordinate conversion against a live window handle.
774            unsafe {
775                let _ = ScreenToClient(HWND(hwnd as *mut std::ffi::c_void), &mut point);
776            }
777            (point.x, point.y)
778        }
779
780        /// What this drag would do at a point, given the keys held and what the source allows.
781        fn effect_at(
782            &self,
783            keys: MODIFIERKEYS_FLAGS,
784            pt: &POINTL,
785            allowed: DROPEFFECT,
786        ) -> DROPEFFECT {
787            let at = self.in_client(pt);
788            let onto = {
789                let Ok(mut shared) = self.shared.lock() else {
790                    return DROPEFFECT_NONE;
791                };
792                shared.hovering = Some(at);
793                // Remembered here because `Drop` is called with the button already released.
794                if keys.0 & MK_RBUTTON.0 != 0 {
795                    shared.right_button = true;
796                }
797                shared.targets.at(at).cloned()
798            };
799            let target = match onto {
800                Some(Onto::Folder(path)) => path,
801                // Pinning moves nothing, so it answers `LINK` whatever is held down. It is
802                // also the only honest answer: a copy cursor over the sidebar would be
803                // promising a copy that is not going to happen.
804                Some(Onto::Bookmarks) => return permitted(DROPEFFECT_LINK, allowed),
805                None => return DROPEFFECT_NONE,
806            };
807
808            if keys.0 & MK_CONTROL.0 != 0 {
809                return permitted(DROPEFFECT_COPY, allowed);
810            }
811            if keys.0 & MK_SHIFT.0 != 0 {
812                return permitted(DROPEFFECT_MOVE, allowed);
813            }
814            // Where it came from, read from the data object when the drag arrived rather than
815            // guessed — a drag can come from anywhere, including from nowhere with a path.
816            let (source, temporary) = self
817                .held
818                .lock()
819                .ok()
820                .and_then(|held| {
821                    let incoming = held.as_ref()?;
822                    Some((incoming.items.first().cloned(), incoming.temporary))
823                })
824                .unwrap_or((None, false));
825            // Nothing the source is about to delete out from under the copy is a move.
826            if temporary {
827                return permitted(DROPEFFECT_COPY, allowed);
828            }
829            let wanted = match super::default_effect(source.as_deref(), &target) {
830                Effect::Move => DROPEFFECT_MOVE,
831                Effect::Copy => DROPEFFECT_COPY,
832            };
833            permitted(wanted, allowed)
834        }
835    }
836
837    impl IDropTarget_Impl for Target_Impl {
838        fn DragEnter(
839            &self,
840            data: Ref<IDataObject>,
841            keys: MODIFIERKEYS_FLAGS,
842            pt: &POINTL,
843            effect: *mut DROPEFFECT,
844        ) -> windows::core::Result<()> {
845            // Once, here — see [`Incoming`]. The lock is taken and let go before `effect_at`,
846            // which takes it again and would deadlock on a `Mutex` that is not reentrant.
847            if let Ok(mut held) = self.held.lock() {
848                *held = Some(Incoming::read(data.as_ref()));
849            }
850            // SAFETY: OLE always passes a valid out-pointer here. It is read before it is
851            // written because on the way in it holds the effects the source allows.
852            unsafe {
853                let allowed = *effect;
854                *effect = self.effect_at(keys, pt, allowed);
855            }
856            Ok(())
857        }
858
859        fn DragOver(
860            &self,
861            keys: MODIFIERKEYS_FLAGS,
862            pt: &POINTL,
863            effect: *mut DROPEFFECT,
864        ) -> windows::core::Result<()> {
865            // SAFETY: as above.
866            unsafe {
867                let allowed = *effect;
868                *effect = self.effect_at(keys, pt, allowed);
869            }
870            Ok(())
871        }
872
873        fn DragLeave(&self) -> windows::core::Result<()> {
874            if let Ok(mut held) = self.held.lock() {
875                *held = None;
876            }
877            if let Ok(mut shared) = self.shared.lock() {
878                shared.hovering = None;
879                shared.right_button = false;
880            }
881            Ok(())
882        }
883
884        fn Drop(
885            &self,
886            data: Ref<IDataObject>,
887            keys: MODIFIERKEYS_FLAGS,
888            pt: &POINTL,
889            effect: *mut DROPEFFECT,
890        ) -> windows::core::Result<()> {
891            // What `DragEnter` read, unless it somehow did not run — in which case read it now
892            // rather than leave the drop with nothing. Filled before `effect_at`, which takes
893            // this same lock, and taken back out after.
894            if let Ok(mut held) = self.held.lock() {
895                if held.is_none() {
896                    *held = Some(Incoming::read(data.as_ref()));
897                }
898            }
899            // SAFETY: OLE always passes a valid out-pointer here, holding on the way in the
900            // effects the source allows.
901            let chosen = unsafe {
902                let allowed = *effect;
903                let chosen = self.effect_at(keys, pt, allowed);
904                *effect = chosen;
905                chosen
906            };
907
908            // Asked for again here rather than reused from `DragEnter`, because for a source
909            // that renders on demand *this* is the call that matters: 7-Zip extracts the
910            // archive to answer it, and has nothing to give until the drop is real. Reusing
911            // the drag-time read left the drop with an empty list, so nothing was extracted
912            // and nothing was copied. The drag-time read stays as the fallback, for a source
913            // that renders once and not again.
914            let cached = self
915                .held
916                .lock()
917                .ok()
918                .and_then(|mut held| held.take())
919                .map(|incoming| incoming.items)
920                .unwrap_or_default();
921            let items = match data.as_ref().and_then(paths_of) {
922                Some(fresh) if !fresh.is_empty() => fresh,
923                _ => cached,
924            };
925            // Converted before the lock is taken, not inside it.
926            let at = self.in_client(pt);
927            // Where it landed, and the drag forgotten, in a turn of the lock of its own — the
928            // claim below touches the filesystem, and doing that while the frame loop waits on
929            // this lock would stall the window for exactly as long as the claim takes.
930            let landing = self.shared.lock().ok().map(|mut shared| {
931                shared.hovering = None;
932                let onto = shared.targets.at(at).cloned();
933                (onto, std::mem::take(&mut shared.right_button))
934            });
935            let Some((Some(onto), asked)) = landing else {
936                return Ok(());
937            };
938            if items.is_empty() || chosen == DROPEFFECT_NONE {
939                return Ok(());
940            }
941            // The one thing that has to happen before this returns: see [`super::claim`]. Not
942            // for a pin, which copies nothing and would otherwise bookmark a scratch folder.
943            let items = match onto {
944                Onto::Folder(_) => super::claim(items),
945                Onto::Bookmarks => items,
946            };
947            if let Ok(mut shared) = self.shared.lock() {
948                shared.dropped.push(Dropped {
949                    items,
950                    effect: if chosen.0 & DROPEFFECT_MOVE.0 != 0 {
951                        Effect::Move
952                    } else {
953                        Effect::Copy
954                    },
955                    at,
956                    onto,
957                    asked,
958                });
959            }
960            Ok(())
961        }
962    }
963
964    /// Every path a data object is offering, via `CF_HDROP`.
965    fn paths_of(data: &IDataObject) -> Option<Vec<PathBuf>> {
966        use windows::Win32::System::Com::{FORMATETC, TYMED_HGLOBAL};
967        use windows::Win32::System::Ole::ReleaseStgMedium;
968        use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
969
970        const CF_HDROP: u16 = 15;
971        let format = FORMATETC {
972            cfFormat: CF_HDROP,
973            ptd: std::ptr::null_mut(),
974            dwAspect: 1,
975            lindex: -1,
976            tymed: TYMED_HGLOBAL.0 as u32,
977        };
978        // SAFETY: the medium is released on every path out, and the handle is only read
979        // while it is held.
980        unsafe {
981            let medium = data.GetData(&format).ok()?;
982            let handle = medium.u.hGlobal;
983            if handle.is_invalid() {
984                return None;
985            }
986            let drop = HDROP(handle.0);
987            let count = DragQueryFileW(drop, u32::MAX, None);
988            let mut items = Vec::with_capacity(count as usize);
989            for index in 0..count {
990                // The length first: a path can be longer than `MAX_PATH`, and a fixed
991                // buffer would silently truncate one.
992                let len = DragQueryFileW(drop, index, None);
993                if len == 0 {
994                    continue;
995                }
996                let mut buffer = vec![0u16; len as usize + 1];
997                let written = DragQueryFileW(drop, index, Some(&mut buffer));
998                if written > 0 {
999                    buffer.truncate(written as usize);
1000                    items.push(PathBuf::from(String::from_utf16_lossy(&buffer)));
1001                }
1002            }
1003            let mut medium = medium;
1004            ReleaseStgMedium(&mut medium);
1005            Some(items)
1006        }
1007    }
1008}
1009
1010#[cfg(test)]
1011mod tests {
1012    use super::*;
1013    use std::path::Path;
1014
1015    #[test]
1016    fn a_drag_within_a_volume_moves_and_across_copies() {
1017        // The rule that makes an accidental drag cheap rather than expensive.
1018        assert_eq!(
1019            default_effect(Some(Path::new(r"C:\a\one.txt")), Path::new(r"C:\b")),
1020            Effect::Move
1021        );
1022        assert_eq!(
1023            default_effect(Some(Path::new(r"C:\a\one.txt")), Path::new(r"D:\b")),
1024            Effect::Copy
1025        );
1026        // Case is not a volume difference.
1027        assert_eq!(
1028            default_effect(Some(Path::new(r"c:\a\one.txt")), Path::new(r"C:\b")),
1029            Effect::Move
1030        );
1031    }
1032
1033    #[test]
1034    fn a_network_share_is_its_own_volume() {
1035        assert_eq!(
1036            default_effect(
1037                Some(Path::new(r"\\server\share\one.txt")),
1038                Path::new(r"\\server\share\sub")
1039            ),
1040            Effect::Move
1041        );
1042        assert_eq!(
1043            default_effect(
1044                Some(Path::new(r"\\server\share\one.txt")),
1045                Path::new(r"\\server\other\sub")
1046            ),
1047            Effect::Copy,
1048            "two shares on one server are still two volumes"
1049        );
1050        assert_eq!(
1051            default_effect(
1052                Some(Path::new(r"\\server\share\one.txt")),
1053                Path::new(r"C:\b")
1054            ),
1055            Effect::Copy
1056        );
1057    }
1058
1059    #[test]
1060    fn an_unknown_source_copies() {
1061        // A drag from somewhere with no volume — a virtual folder, a browser — cannot be
1062        // a move, and guessing otherwise would be the destructive guess.
1063        assert_eq!(default_effect(None, Path::new(r"C:\b")), Effect::Copy);
1064    }
1065
1066    #[test]
1067    fn zones_resolve_the_front_one_first() {
1068        let under = Onto::Folder(PathBuf::from(r"C:\under"));
1069        let over = Onto::Folder(PathBuf::from(r"C:\over"));
1070        let targets = Targets {
1071            zones: vec![
1072                ((0, 0, 100, 100), Onto::Bookmarks),
1073                ((0, 0, 100, 100), under.clone()),
1074                ((50, 50, 150, 150), over.clone()),
1075            ],
1076        };
1077        assert_eq!(
1078            targets.at((60, 60)),
1079            Some(&over),
1080            "the later zone is the one in front"
1081        );
1082        assert_eq!(
1083            targets.at((10, 10)),
1084            Some(&under),
1085            "a listing over the sidebar's own zone means the listing"
1086        );
1087        assert_eq!(targets.at((200, 200)), None);
1088    }
1089}
