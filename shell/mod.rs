1//! The parts of this program that *are* the shell rather than merely near it.
2//!
3//! Everything here goes through the same interfaces Explorer uses, so the results
4//! are the same results — the same icons, the same context menu including whatever
5//! extensions are installed, the same copy/move/delete with the same progress
6//! dialogs, the same conflict prompts, the same undo, and the same Recycle Bin.
7//! Re-implementing any of that would mean a file manager whose Delete is *nearly*
8//! Explorer's Delete, and nearly is the wrong target for something that moves a
9//! user's files.
10//!
11//! | module | what it is |
12//! | --- | --- |
13//! | [`icons`] | the system image list, cached by file type |
14//! | [`links`] | `IShellLink` — what a shortcut points at |
15//! | [`clipboard`] | `CF_HDROP` and `Preferred DropEffect` — cut, copy, paste |
16//! | [`ops`] | `IFileOperation` — copy, move, delete, rename, new folder |
17//! | [`menu`] | `IContextMenu` — the real menu, extensions included |
18//! | [`dnd`] | OLE drag and drop, as a source and as a target |
19//!
20//! # Threading
21//!
22//! COM is apartment-threaded and the shell is emphatic about it: `IContextMenu` and
23//! `IFileOperation` both put up windows, so both have to run on a thread with a
24//! message pump — which for this program means the UI thread, initialised as an STA
25//! by [`init`]. The icon lookups are the exception: they call one function that is
26//! documented as free-threaded and touch nothing else, so they run on workers.
27//!
28//! An apartment is a promise, and the promise has a second half that is easy to miss:
29//! anything a thread hands out can be called back into, and the caller *blocks* until it is
30//! answered. A thread of this program ’s that parks in `recv` answers nothing, and what that
31//! breaks is not obvious from the failure. See [`answering_calls`], which is what every wait
32//! here goes through, and the note on [`Modal`].
33
34pub mod clipboard;
35pub mod dnd;
36pub mod icons;
37pub mod links;
38pub mod menu;
39pub mod ops;
40
41/// Initialise COM for the calling thread as a single-threaded apartment.
42///
43/// Called once, from the UI thread, before anything else here. The shell requires an
44/// STA for the interfaces that show UI, and `OleInitialize` rather than
45/// `CoInitializeEx` because drag and drop and the clipboard need OLE's own setup on
46/// top of COM's.
47pub fn init() {
48    #[cfg(windows)]
49    {
50        use windows::Win32::System::Ole::OleInitialize;
51        // SAFETY: called once, on the thread that will own every call below. A failure
52        // means the shell features degrade to nothing, which the callers all handle.
53        let _ = unsafe { OleInitialize(None) };
54    }
55}
56
57/// Wait for a while, answering the cross-apartment calls this thread owes, and nothing else.
58///
59/// A single-threaded apartment is a promise: anything this thread hands out can be called back
60/// into, and the caller blocks until it is answered. A thread that sleeps, or blocks on a
61/// channel, breaks that promise — and on Windows the things it breaks are not obvious:
62///
63/// - **The clipboard stops working.** Whatever put data on the clipboard owns it, and reading
64///   it from anywhere else is a call back into the owning apartment. A shell Copy run on the
65///   modal thread put the files there correctly and *nothing could read them*, this program
66///   included, because that thread was parked in `recv`.
67/// - **A copy refuses the next copy.** The clipboard holds a reference rather than the bytes,
68///   so a reader takes the clipboard lock and then calls in for the data; unanswered, the lock
69///   stays taken. See the note on `retrying` in [`clipboard`].
70///
71/// `CoWaitForMultipleHandles` on an event nobody will ever signal, so the wait *is* the sleep.
72/// Without `COWAIT_DISPATCH_WINDOW_MESSAGES` it services cross-apartment calls and leaves the
73/// window queue alone — which matters on the UI thread, where dispatching a `WM_PAINT` from
74/// inside an egui pass would have egui begin a pass while already inside one.
75#[cfg(windows)]
76pub(crate) fn answering_calls(ms: u64) {
77    use windows::Win32::Foundation::HANDLE;
78    use windows::Win32::System::Com::{CoWaitForMultipleHandles, COWAIT_DISPATCH_CALLS};
79    use windows::Win32::System::Threading::CreateEventW;
80
81    thread_local! {
82        /// One per thread, made once. The modal thread waits like this twenty times a second
83        /// for as long as the program runs, and creating and closing a handle each time would
84        /// be a needless million of them over an afternoon. Never signalled, so it is only
85        /// ever a thing to wait on; leaked at thread exit, which is where the process is
86        /// going anyway.
87        static NEVER: Option<HANDLE> = {
88            // SAFETY: a plain manual-reset event, unnamed and unsignalled.
89            unsafe { CreateEventW(None, true, false, None).ok() }
90        };
91    }
92
93    let waited = NEVER.with(|event| match event {
94        Some(event) => {
95            // SAFETY: the handle belongs to this thread and outlives the wait.
96            unsafe {
97                let _ = CoWaitForMultipleHandles(
98                    COWAIT_DISPATCH_CALLS.0 as u32,
99                    ms as u32,
100                    &[*event],
101                );
102            }
103            true
104        }
105        None => false,
106    });
107    // No event to wait on is no reason to spin.
108    if !waited {
109        std::thread::sleep(std::time::Duration::from_millis(ms));
110    }
111}
112
113#[cfg(not(windows))]
114pub(crate) fn answering_calls(ms: u64) {
115    std::thread::sleep(std::time::Duration::from_millis(ms));
116}
117
118/// Render whatever this program has put on the clipboard, so it outlives the process.
119///
120/// `OleSetClipboard` copies nothing. It leaves the clipboard holding a reference to the data
121/// object *here*, and another process asking for the bytes is a marshalled call back into this
122/// apartment — which is why a copy taken here pastes into Explorer while this window is open,
123/// and why it would paste into nothing at all a moment after the window closed. `OleFlushClipboard`
124/// renders every format for real, so the clipboard keeps the files rather than a pointer to a
125/// process that has gone.
126///
127/// Called on the way out and not after every copy, deliberately: while the object is still
128/// this program's, a paste elsewhere can hand back `CFSTR_PASTESUCCEEDED` and finish a cut
129/// properly. A flush replaces it with a snapshot, and that conversation is over.
130pub fn flush() {
131    #[cfg(windows)]
132    {
133        use windows::Win32::System::Ole::OleFlushClipboard;
134        // SAFETY: documented as safe to call whether or not this process owns the clipboard;
135        // it does nothing when it does not.
136        let _ = unsafe { OleFlushClipboard() };
137    }
138}
139
140/// Serialises the tests that reach into the shell.
141///
142/// Cargo runs a crate's tests as parallel threads of one process, and some of what the
143/// shell does on the way to filling a context menu — an extension asking whether there is
144/// anything to paste, say — takes the **process-wide** clipboard lock. That is enough to
145/// make [`clipboard`]'s round trip come back "OpenClipboard refused", intermittently and
146/// only ever in a full run, which is the worst way to learn it. Every test that goes
147/// through the shell takes this first.
148///
149/// The production code does not need it: the retry in [`clipboard`] covers the contention
150/// a real desktop produces, and there is one UI thread rather than eight.
151#[cfg(test)]
152pub(crate) static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());
153
154/// Take [`ONE_AT_A_TIME`], ignoring a poisoning from some other test's panic.
155#[cfg(test)]
156pub(crate) fn serialised() -> std::sync::MutexGuard<'static, ()> {
157    ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner())
158}
159
160/// The window the shell should parent its dialogs to.
161///
162/// A progress dialog with no owner appears behind the window that started it and
163/// looks like it has hung. `None` is safe — the shell then owns them to the desktop —
164/// but it is worth having.
165#[cfg(windows)]
166#[derive(Clone, Copy, Default)]
167pub struct Owner(pub isize);
168
169#[cfg(not(windows))]
170#[derive(Clone, Copy, Default)]
171pub struct Owner(pub isize);
172
173impl Owner {
174    /// Take the handle out of whatever eframe was given by the platform.
175    pub fn from_handle(handle: &dyn raw_window_handle::HasWindowHandle) -> Self {
176        #[cfg(windows)]
177        {
178            use raw_window_handle::RawWindowHandle;
179            if let Ok(handle) = handle.window_handle() {
180                if let RawWindowHandle::Win32(win32) = handle.as_raw() {
181                    return Self(win32.hwnd.get());
182                }
183            }
184        }
185        #[cfg(not(windows))]
186        let _ = handle;
187        Self(0)
188    }
189
190    #[cfg(windows)]
191    pub(crate) fn hwnd(self) -> windows::Win32::Foundation::HWND {
192        windows::Win32::Foundation::HWND(self.0 as *mut std::ffi::c_void)
193    }
194}
195
196/// A path as a null-terminated wide string, which every call here wants.
197///
198/// Forward slashes become backslashes on the way through, and that is not cosmetic. The
199/// kernel accepts either — `std::fs` opens `C:/Windows/notepad.exe` quite happily — but the
200/// shell *parses* paths, and `SHCreateItemFromParsingName` and `SHParseDisplayName` both
201/// refuse one with a slash in it. So a path that arrived with slashes, which is exactly what
202/// `--open=C:/Windows` gives, used to list its folder perfectly and then fail every single
203/// shell call against it: no icons, no context menu, and copy, paste and delete all reporting
204/// that the items could not be found. One conversion here fixes all of them, because every
205/// call in this module comes through this function.
206#[cfg(windows)]
207pub(crate) fn wide(path: &std::path::Path) -> Vec<u16> {
208    use std::os::windows::ffi::OsStrExt as _;
209    // Unit by unit is safe: `/` is U+002F, which never appears as either half of a surrogate
210    // pair, so no character outside the BMP can be damaged by this.
211    const SLASH: u16 = b'/' as u16;
212    const BACKSLASH: u16 = b'\\' as u16;
213    path.as_os_str()
214        .encode_wide()
215        .map(|unit| if unit == SLASH { BACKSLASH } else { unit })
216        .chain(std::iter::once(0))
217        .collect()
218}
219
220/// Whether reading this path means going over a network.
221///
222/// True for a UNC path, and for a drive letter that is a mapped share. Used by [`menu`] to
223/// decide how much of a context menu it is worth asking the shell for, which is a decision
224/// that has to be made before anything slow is allowed to happen — so this must be cheap.
225///
226/// `GetDriveTypeW` is: it answers from the mount table and is documented not to touch the
227/// network, so a share that has gone away still comes back `DRIVE_REMOTE` rather than hanging.
228/// Measured by `probe_menu_costs` at **376 µs** for a mapped `H:\` the first time and 26 µs
229/// after, against 41 µs and 6 µs for a local drive — so the first look at a share costs about a
230/// third of a millisecond, and the cache is what keeps every right click after it from doing so.
231/// Either way it is four orders of magnitude under the menu it is deciding about.
232pub fn over_network(path: &std::path::Path) -> bool {
233    #[cfg(windows)]
234    {
235        use std::collections::HashMap;
236        use std::sync::Mutex;
237        use windows::Win32::Storage::FileSystem::GetDriveTypeW;
238        use windows::Win32::System::WindowsProgramming::DRIVE_REMOTE;
239
240        // `\\server\share\...`, which is a network path by construction and needs no asking.
241        // `\\?\` and `\\.\` are not: they are the extended-length and device forms of a local
242        // path, and are checked for first because they start the same way.
243        let text = path.as_os_str().to_string_lossy();
244        let bytes = text.as_bytes();
245        if bytes.starts_with(br"\\") && !bytes.starts_with(br"\\?\") && !bytes.starts_with(br"\\.\")
246        {
247            return true;
248        }
249
250        // `H:\a\b` -> `H:\`, which is what `GetDriveTypeW` wants. Anything without a root —
251        // a relative path, or the empty path that means This PC — is not ours to judge.
252        let Some(root) = path.ancestors().last().filter(|r| !r.as_os_str().is_empty()) else {
253            return false;
254        };
255
256        static KNOWN: Mutex<Option<HashMap<std::path::PathBuf, bool>>> = Mutex::new(None);
257        let mut known = KNOWN.lock().unwrap_or_else(|e| e.into_inner());
258        let known = known.get_or_insert_with(HashMap::new);
259        if let Some(&remote) = known.get(root) {
260            return remote;
261        }
262        let root_wide = wide(root);
263        // SAFETY: null-terminated by `wide`, and it outlives the call.
264        let remote =
265            unsafe { GetDriveTypeW(windows::core::PCWSTR(root_wide.as_ptr())) } == DRIVE_REMOTE;
266        known.insert(root.to_owned(), remote);
267        remote
268    }
269    #[cfg(not(windows))]
270    {
271        let _ = path;
272        false
273    }
274}
275
276// ---------------------------------------------------------------------------
277// The modal thread
278// ---------------------------------------------------------------------------
279
280/// Something that puts up a window of its own and does not return until the user is done
281/// with it.
282pub enum Request {
283    /// Run a shell command chosen from the context menu.
284    Invoke {
285        parent: std::path::PathBuf,
286        items: Vec<std::path::PathBuf>,
287        command: menu::Command,
288        owner: Owner,
289    },
290}
291
292/// What one of those turned out to be.
293pub enum Reply {
294    /// A command ran. What it did is unknown, and deliberately not acted on: the folder is
295    /// watched, so a command that changed it is noticed like any other change on disk, and one
296    /// that did not costs no scan. See `crate::app::App::collect_modal`.
297    Invoked,
298}
299
300/// Runs the shell calls that put up a window of their own, on a thread of their own.
301///
302/// # Why not on the UI thread
303///
304/// `InvokeCommand` can open anything from a Properties sheet to an installer. Called from
305/// inside the frame — which is where every other action is performed — it would dispatch
306/// messages to this window's own procedure; winit would turn a `WM_PAINT` into a redraw
307/// request; eframe would call straight back into `Context::run_ui`; and egui would be asked
308/// to begin a pass while already inside one. Whether that panics or deadlocks depends on
309/// timing, which is the worst kind of bug to ship: it works while you are testing it.
310///
311/// It is documented as belonging to a *thread* rather than to a window, so it runs here,
312/// where there is no egui pass to re-enter, and the answer comes back by channel like any
313/// other background result.
314///
315/// # What is deliberately *not* here
316///
317/// **Building the menu.** `QueryContextMenu` only fills an `HMENU` and shows nothing, so
318/// [`menu::build`] is safe to call during a frame — which is what lets the menu be drawn by
319/// this program rather than by Windows.
320///
321/// **Starting a drag.** `DoDragDrop` is modal in the same way and for the same reason, but it
322/// gets a thread of its own per drag rather than sharing this one: it has to join the UI
323/// thread's input queue for the length of the drag to see the button that started it, and this
324/// thread is long-lived and shared. See [`dnd::Drag`].
325pub struct Modal {
326    tx: std::sync::mpsc::Sender<Request>,
327    rx: std::sync::mpsc::Receiver<Reply>,
328    /// Whether a request is outstanding. One at a time: they are modal, and a second
329    /// menu behind the first would be a menu nobody asked for.
330    busy: bool,
331}
332
333impl Modal {
334    pub fn new(ctx: &egui::Context) -> Self {
335        let (tx, requests) = std::sync::mpsc::channel::<Request>();
336        let (replies, rx) = std::sync::mpsc::channel::<Reply>();
337        let ctx = ctx.clone();
338
339        let spawned = std::thread::Builder::new()
340            .name("shell-modal".to_owned())
341            .spawn(move || {
342                // This thread's own apartment: the menu's window and every interface it
343                // touches belong to it.
344                init();
345                // Not `recv()`. This thread runs shell commands, and one of them is Copy: the
346                // clipboard data then belongs to *this* apartment, and every read of it — by
347                // this program or by any other — is a call back into here. Parked in `recv()`
348                // this thread answered none of them, so a Copy from the context menu put the
349                // files on the clipboard and nothing whatsoever could read them back. Fifty
350                // milliseconds of latency on a menu click is not perceptible; a clipboard that
351                // silently does nothing is.
352                loop {
353                    let request = match requests.try_recv() {
354                        Ok(request) => request,
355                        Err(std::sync::mpsc::TryRecvError::Empty) => {
356                            answering_calls(50);
357                            continue;
358                        }
359                        Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
360                    };
361                    let reply = match request {
362                        Request::Invoke {
363                            parent,
364                            items,
365                            command,
366                            owner,
367                        } => {
368                            menu::invoke(&parent, &items, &command, owner);
369                            Reply::Invoked
370                        }
371                    };
372                    if replies.send(reply).is_err() {
373                        return;
374                    }
375                    ctx.request_repaint();
376                }
377            });
378        let _ = spawned;
379
380        Self {
381            tx,
382            rx,
383            busy: false,
384        }
385    }
386
387    /// Ask for a modal gesture. `false` when one is already running.
388    pub fn send(&mut self, request: Request) -> bool {
389        if self.busy {
390            return false;
391        }
392        if self.tx.send(request).is_ok() {
393            self.busy = true;
394            return true;
395        }
396        false
397    }
398
399    /// The answer, once there is one.
400    pub fn poll(&mut self) -> Option<Reply> {
401        match self.rx.try_recv() {
402            Ok(reply) => {
403                self.busy = false;
404                Some(reply)
405            }
406            Err(_) => None,
407        }
408    }
409
410}
