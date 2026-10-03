1//! Notice when a folder changes on disk, so a listing is never stale.
2//!
3//! Without this, a listing is only as fresh as the last thing this program did to it. Anything
4//! anybody *else* did went unseen: a file dragged out to Explorer stayed on screen because
5//! Explorer's move finishes after our drag does and there was nothing to wait on; a build
6//! writing into the folder you were watching showed the folder as it had been; a file deleted
7//! from a terminal left a row that opened nothing. Worse than looking wrong, it made the *next*
8//! gesture fail — dragging a row that no longer names a file cannot start a drag, so the window
9//! appeared to have stopped responding.
10//!
11//! # One thread, not one per folder
12//!
13//! Navigating changes the watched set constantly — every folder you open and leave — so a thread
14//! per folder would mean a thread spawned and joined per click. Instead there is one thread
15//! holding a directory handle and an event per watched folder, parked in
16//! `WaitForMultipleObjects` over all of them plus one more event the UI thread signals when the
17//! set changes. Nothing spins and nothing polls: the thread wakes when a folder changes or when
18//! the set does.
19//!
20//! # What it does not do
21//!
22//! It does not read the notifications. `ReadDirectoryChangesW` will say which file changed and
23//! how, and none of that is worth having here: the answer to any of it is to re-read the folder,
24//! which is one scan either way. Not parsing the buffer also removes the two ways this API is
25//! usually got wrong — the alignment of `FILE_NOTIFY_INFORMATION` and the overflow case where
26//! the buffer was too small and the contents are gone but the fact of the change is not.
27
28use std::collections::HashMap;
29use std::path::{Path, PathBuf};
30use std::sync::{Arc, Mutex};
31
32/// How long to let a burst of changes settle before re-reading, in seconds.
33///
34/// A single copy fires several notifications — the file appears, its size changes, its timestamp
35/// changes — and one folder scan answers all of them. Long enough to collapse a burst, short
36/// enough that a file appearing looks immediate.
37const SETTLE: f64 = 0.15;
38
39/// The most folders one thread can watch, since it waits on them all at once.
40///
41/// `MAXIMUM_WAIT_OBJECTS` is 64 and one of those is the wake event. Reaching this needs 63 tabs
42/// open at once; the ones past it simply are not watched, which is the behaviour this had for
43/// every folder before.
44#[cfg(windows)]
45const MAX_WATCHED: usize = 63;
46
47#[derive(Default)]
48struct Shared {
49    /// The folders the UI wants watched, replaced wholesale by [`Watch::keep`].
50    wanted: Vec<PathBuf>,
51    /// Folders seen to change, waiting to be asked for.
52    changed: Vec<PathBuf>,
53    quit: bool,
54}
55
56/// Watches the folders on screen and reports the ones that change.
57pub struct Watch {
58    shared: Arc<Mutex<Shared>>,
59    /// The last set handed to [`Watch::keep`], so an unchanged frame takes no lock.
60    mine: Vec<PathBuf>,
61    /// Changes waiting out their settle window, and when each is due.
62    pending: HashMap<PathBuf, f64>,
63    /// The event that wakes the thread, as an integer because a `HANDLE` is not `Send`.
64    #[cfg(windows)]
65    wake: isize,
66    #[cfg(windows)]
67    thread: Option<std::thread::JoinHandle<()>>,
68}
69
70impl Watch {
71    pub fn new(ctx: &egui::Context) -> Self {
72        let shared = Arc::new(Mutex::new(Shared::default()));
73        #[cfg(windows)]
74        {
75            let (wake, thread) = win::start(shared.clone(), ctx.clone());
76            Self {
77                shared,
78                mine: Vec::new(),
79                pending: HashMap::new(),
80                wake,
81                thread,
82            }
83        }
84        #[cfg(not(windows))]
85        {
86            let _ = ctx;
87            Self {
88                shared,
89                mine: Vec::new(),
90                pending: HashMap::new(),
91            }
92        }
93    }
94
95    /// Watch exactly these folders and no others.
96    ///
97    /// Called every frame with whatever is on screen, so it has to be cheap when nothing has
98    /// moved: the set is compared against a local copy first and the lock is only taken when it
99    /// actually differs.
100    pub fn keep(&mut self, folders: &[PathBuf]) {
101        let mut wanted: Vec<PathBuf> = folders
102            .iter()
103            .filter(|path| !path.as_os_str().is_empty())
104            .cloned()
105            .collect();
106        wanted.sort();
107        wanted.dedup();
108        if wanted == self.mine {
109            return;
110        }
111        self.mine = wanted.clone();
112        if let Ok(mut shared) = self.shared.lock() {
113            shared.wanted = wanted;
114        }
115        #[cfg(windows)]
116        win::signal(self.wake);
117    }
118
119    /// Folders that have changed and have settled, so are due a re-read.
120    ///
121    /// `now` is egui's own clock, which is the one the caller can wake itself against.
122    pub fn changed(&mut self, now: f64) -> Vec<PathBuf> {
123        if let Ok(mut shared) = self.shared.lock() {
124            for path in shared.changed.drain(..) {
125                // First notification of a burst starts the clock; the rest ride along with it.
126                self.pending.entry(path).or_insert(now + SETTLE);
127            }
128        }
129        if self.pending.is_empty() {
130            return Vec::new();
131        }
132        let due: Vec<PathBuf> = self
133            .pending
134            .iter()
135            .filter(|(_, &at)| at <= now)
136            .map(|(path, _)| path.clone())
137            .collect();
138        for path in &due {
139            self.pending.remove(path);
140        }
141        due
142    }
143
144    /// Whether anything is still waiting out its settle window, and so whether the caller has to
145    /// come back for it.
146    pub fn waiting(&self) -> bool {
147        !self.pending.is_empty()
148    }
149}
150
151impl Drop for Watch {
152    fn drop(&mut self) {
153        #[cfg(windows)]
154        {
155            if let Ok(mut shared) = self.shared.lock() {
156                shared.quit = true;
157            }
158            win::signal(self.wake);
159            if let Some(thread) = self.thread.take() {
160                let _ = thread.join();
161            }
162            win::close(self.wake);
163        }
164    }
165}
166
167#[cfg(windows)]
168mod win {
169    use super::*;
170    use windows::core::PCWSTR;
171    use windows::Win32::Foundation::{
172        CloseHandle, HANDLE, INVALID_HANDLE_VALUE, WAIT_FAILED, WAIT_OBJECT_0,
173    };
174    use windows::Win32::Storage::FileSystem::{
175        CreateFileW, ReadDirectoryChangesW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OVERLAPPED,
176        FILE_LIST_DIRECTORY, FILE_NOTIFY_CHANGE_ATTRIBUTES, FILE_NOTIFY_CHANGE_DIR_NAME,
177        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
178        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
179    };
180    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects};
181    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
182
183    /// Everything worth being told about a folder's contents.
184    ///
185    /// Not `FILE_NOTIFY_CHANGE_LAST_ACCESS`: reading a file in the folder would then re-read the
186    /// folder, and merely showing the listing reads it.
187    const FILTER: windows::Win32::Storage::FileSystem::FILE_NOTIFY_CHANGE =
188        windows::Win32::Storage::FileSystem::FILE_NOTIFY_CHANGE(
189            FILE_NOTIFY_CHANGE_FILE_NAME.0
190                | FILE_NOTIFY_CHANGE_DIR_NAME.0
191                | FILE_NOTIFY_CHANGE_ATTRIBUTES.0
192                | FILE_NOTIFY_CHANGE_SIZE.0
193                | FILE_NOTIFY_CHANGE_LAST_WRITE.0,
194        );
195
196    /// One watched folder: the handle, the event it completes on, and the read in flight.
197    struct Watched {
198        path: PathBuf,
199        dir: HANDLE,
200        event: HANDLE,
201        /// Boxed because the kernel keeps the address until the read completes, and the struct
202        /// itself moves when it goes into the vector.
203        overlapped: Box<OVERLAPPED>,
204        /// `u32` so the buffer is `DWORD`-aligned, which the API requires even though nothing
205        /// here ever reads it.
206        buffer: Box<[u32; 256]>,
207    }
208
209    impl Watched {
210        fn open(path: &Path) -> Option<Self> {
211            let wide = crate::shell::wide(path);
212            // SAFETY: a null-terminated path that outlives the call. `FILE_SHARE_DELETE` is not
213            // optional: without it this handle would stop the folder from being deleted or
214            // renamed, so watching a folder would break the very operations it is watching for.
215            let dir = unsafe {
216                CreateFileW(
217                    PCWSTR(wide.as_ptr()),
218                    FILE_LIST_DIRECTORY.0,
219                    FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
220                    None,
221                    OPEN_EXISTING,
222                    FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
223                    None,
224                )
225                .ok()?
226            };
227            if dir == INVALID_HANDLE_VALUE {
228                return None;
229            }
230            // Auto-reset: the wait that observes it clears it, so re-arming needs nothing.
231            // SAFETY: an unnamed event, released in `Drop`.
232            let event = match unsafe { CreateEventW(None, false, false, PCWSTR::null()) } {
233                Ok(event) => event,
234                Err(_) => {
235                    // SAFETY: opened just above and not handed anywhere.
236                    let _ = unsafe { CloseHandle(dir) };
237                    return None;
238                }
239            };
240            let mut overlapped = Box::new(OVERLAPPED::default());
241            overlapped.hEvent = event;
242            let mut watched = Self {
243                path: path.to_path_buf(),
244                dir,
245                event,
246                overlapped,
247                buffer: Box::new([0u32; 256]),
248            };
249            watched.arm().then_some(watched)
250        }
251
252        /// Ask for the next change. False if the folder has gone.
253        fn arm(&mut self) -> bool {
254            let len = std::mem::size_of_val(self.buffer.as_ref()) as u32;
255            // SAFETY: the buffer and the `OVERLAPPED` are boxed, so their addresses stand still
256            // until the read completes — which is what makes this sound at all. The kernel
257            // writes into the buffer and nothing here reads it while the read is in flight.
258            unsafe {
259                ReadDirectoryChangesW(
260                    self.dir,
261                    self.buffer.as_mut().as_mut_ptr().cast(),
262                    len,
263                    false,
264                    FILTER,
265                    None,
266                    Some(self.overlapped.as_mut() as *mut OVERLAPPED),
267                    None,
268                )
269                .is_ok()
270            }
271        }
272
273        /// Complete the read that just signalled, and ask for the next.
274        fn consume(&mut self) -> bool {
275            let mut written = 0u32;
276            // SAFETY: the read this completes was issued by `arm` on this same `OVERLAPPED`.
277            // The byte count is ignored: zero means the buffer overflowed, which is still a
278            // change, and any other value describes files this does not need to know about.
279            unsafe {
280                let _ = GetOverlappedResult(self.dir, self.overlapped.as_ref(), &mut written, false);
281            }
282            self.arm()
283        }
284    }
285
286    impl std::ops::Drop for Watched {
287        fn drop(&mut self) {
288            // SAFETY: cancel the read *before* closing anything, or the kernel would complete
289            // into a buffer this is about to free.
290            unsafe {
291                let _ = CancelIoEx(self.dir, Some(self.overlapped.as_ref()));
292                let _ = CloseHandle(self.dir);
293                let _ = CloseHandle(self.event);
294            };
295        }
296    }
297
298    /// The wake event and the thread waiting on it.
299    pub fn start(
300        shared: Arc<Mutex<Shared>>,
301        ctx: egui::Context,
302    ) -> (isize, Option<std::thread::JoinHandle<()>>) {
303        // Manual reset, so a signal is not lost if the thread is between waits.
304        // SAFETY: an unnamed event, closed by `Watch::drop`.
305        let Ok(wake) = (unsafe { CreateEventW(None, true, false, PCWSTR::null()) }) else {
306            return (0, None);
307        };
308        // Carried across as an integer: a `HANDLE` is a raw pointer and so not `Send`, which
309        // is a rule about the type rather than about this handle — a kernel object is shared
310        // between threads by design, and this one is closed only after the thread is joined.
311        let raw = wake.0 as isize;
312        let thread = std::thread::Builder::new()
313            .name("folder-watch".to_owned())
314            .spawn(move || run(shared, raw, ctx))
315            .ok();
316        (raw, thread)
317    }
318
319    pub fn signal(wake: isize) {
320        if wake == 0 {
321            return;
322        }
323        // SAFETY: the handle belongs to `Watch`, which outlives every call to this.
324        let _ = unsafe { SetEvent(HANDLE(wake as *mut std::ffi::c_void)) };
325    }
326
327    pub fn close(wake: isize) {
328        if wake == 0 {
329            return;
330        }
331        // SAFETY: closed once, from `Watch::drop`, after the thread has been joined.
332        let _ = unsafe { CloseHandle(HANDLE(wake as *mut std::ffi::c_void)) };
333    }
334
335    fn run(shared: Arc<Mutex<Shared>>, wake: isize, ctx: egui::Context) {
336        use windows::Win32::System::Threading::{ResetEvent, INFINITE};
337
338        let wake = HANDLE(wake as *mut std::ffi::c_void);
339
340        let mut watches: Vec<Watched> = Vec::new();
341        loop {
342            // ---- Reconcile the set -------------------------------------
343            let wanted = {
344                let Ok(shared) = shared.lock() else { return };
345                if shared.quit {
346                    return;
347                }
348                shared.wanted.clone()
349            };
350            // SAFETY: manual-reset, so it is cleared here rather than by the wait — before the
351            // set is read, so a change arriving during reconciliation wakes the next wait
352            // instead of being dropped.
353            let _ = unsafe { ResetEvent(wake) };
354            // Dropping a `Watched` cancels its read and closes its handles.
355            watches.retain(|w| wanted.contains(&w.path));
356            for path in wanted.iter().take(MAX_WATCHED) {
357                if !watches.iter().any(|w| &w.path == path) {
358                    if let Some(watched) = Watched::open(path) {
359                        watches.push(watched);
360                    }
361                }
362            }
363
364            // ---- Wait --------------------------------------------------
365            let mut handles = Vec::with_capacity(watches.len() + 1);
366            handles.push(wake);
367            handles.extend(watches.iter().map(|w| w.event));
368            // SAFETY: every handle is live — the wake event belongs to `Watch`, which joins this
369            // thread before closing it, and the rest are owned by `watches`.
370            let got = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
371            if got == WAIT_FAILED {
372                // Nothing to retry against, and spinning on it would burn a core.
373                std::thread::sleep(std::time::Duration::from_millis(200));
374                continue;
375            }
376            let index = got.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
377            if index == 0 || index > watches.len() {
378                // The wake event, or something unexpected: reconcile and wait again.
379                continue;
380            }
381
382            // ---- Report ------------------------------------------------
383            let watched = &mut watches[index - 1];
384            let path = watched.path.clone();
385            if !watched.consume() {
386                // The folder has gone. Its own disappearance is a change in its parent, which
387                // is watched separately if it is on screen.
388                watches.remove(index - 1);
389            }
390            if let Ok(mut shared) = shared.lock() {
391                if shared.quit {
392                    return;
393                }
394                if !shared.changed.contains(&path) {
395                    shared.changed.push(path);
396                }
397            }
398            ctx.request_repaint();
399        }
400    }
401}
402
403#[cfg(test)]
404mod tests {
405    use super::*;
406
407    /// A change waits out its settle window, then comes back exactly once.
408    ///
409    /// The window is what keeps one copy from becoming five folder scans: writing a file fires
410    /// several notifications — the name, then the size, then the timestamp — and one scan answers
411    /// all of them.
412    #[test]
413    fn a_burst_of_changes_becomes_one_re_read() {
414        let ctx = egui::Context::default();
415        let mut watch = Watch::new(&ctx);
416        let folder = PathBuf::from(r"C:\Temp");
417
418        // Three notifications, as a single file being written would produce.
419        if let Ok(mut shared) = watch.shared.lock() {
420            shared.changed.push(folder.clone());
421            shared.changed.push(folder.clone());
422            shared.changed.push(folder.clone());
423        }
424
425        assert!(
426            watch.changed(0.0).is_empty(),
427            "nothing is due until the burst has settled"
428        );
429        assert!(watch.waiting(), "and the caller is told to come back");
430        assert_eq!(
431            watch.changed(SETTLE + 0.001),
432            vec![folder],
433            "one re-read for the burst, not three"
434        );
435        assert!(!watch.waiting());
436        assert!(
437            watch.changed(10.0).is_empty(),
438            "and it does not come back again"
439        );
440    }
441
442    /// The empty path is This PC, which is a list of volumes rather than a folder.
443    #[test]
444    fn nothing_watches_this_pc() {
445        let ctx = egui::Context::default();
446        let mut watch = Watch::new(&ctx);
447        watch.keep(&[PathBuf::new(), PathBuf::from(r"C:\Temp")]);
448        assert_eq!(watch.mine, vec![PathBuf::from(r"C:\Temp")]);
449    }
450
451    /// Two panes on one folder ask for it once.
452    #[test]
453    fn the_same_folder_twice_is_watched_once() {
454        let ctx = egui::Context::default();
455        let mut watch = Watch::new(&ctx);
456        let one = PathBuf::from(r"C:\Temp");
457        watch.keep(&[one.clone(), one.clone()]);
458        assert_eq!(watch.mine, vec![one]);
459    }
460}
