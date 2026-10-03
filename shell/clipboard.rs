1//! Cut, copy and paste — on the same clipboard Explorer uses.
2//!
3//! Interoperability is the whole point: files cut here paste into Explorer, files
4//! copied in Explorer paste here, and both work with 7-Zip, Notepad++, every Office
5//! application and anything else that speaks the shell's clipboard. That means using
6//! the shell's own formats rather than inventing one.
7//!
8//! Two formats carry it:
9//!
10//! - **`CF_HDROP`** — a `DROPFILES` header followed by the paths as wide strings, one
11//!   after another, with an extra terminator at the end. This is the format that has
12//!   meant "these files" since Windows 3.1 and every target understands it.
13//! - **`"Preferred DropEffect"`** — a registered format holding a single `DWORD`.
14//!   `DROPEFFECT_COPY` means copy; `DROPEFFECT_MOVE` means **cut**. There is no
15//!   separate "cut" clipboard: a cut is a copy that asks the *target* to move. Which
16//!   is also why a cut with no paste leaves the files exactly where they were.
17//!
18//! The data object is built by the shell itself, from the items' PIDLs, so it carries
19//! `CFSTR_SHELLIDLIST` and the rest of the formats Explorer offers alongside
20//! `CF_HDROP` — a target that prefers one of those gets it.
21//!
22//! # Reading one back is not reading `CF_HDROP`
23//!
24//! `CF_HDROP` can only carry paths in the file system, and not everything Explorer lets you
25//! copy is one. Copy a file out of a `.zip` and the data object it offers is
26//! `Shell IDList Array`, `FileGroupDescriptorW` and `FileContents` — measured, and with no
27//! `CF_HDROP` at all — so a paste that only knows `CF_HDROP` reads nothing and does nothing.
28//!
29//! So a paste asks the shell instead: `SHCreateShellItemArrayFromDataObject` turns any of
30//! those into items, and each item's `SIGDN_DESKTOPABSOLUTEPARSING` name —
31//! `C:\bundle.zip\inner.txt` for the archive case — goes straight back into
32//! `SHCreateItemFromParsingName` on the other side. `IFileOperation` then extracts it exactly
33//! as Explorer does. `CF_HDROP` stays as the fallback, for a source that offers only that.
34//!
35//! # The end of a cut
36//!
37//! A paste that *moved* has to say so. `CFSTR_PASTESUCCEEDED` is how the source finds out:
38//! it is what makes Explorer stop showing the items faded, and what lets a source whose
39//! items exist only while it holds them let go of them. Then the clipboard is emptied,
40//! because a cut that has been pasted must not be pastable twice — the files are no longer
41//! where it says they are. See [`cut_pasted`], and note that it happens when the operation
42//! *finishes*: doing it when the paste starts loses the cut for anyone who answers the
43//! conflict dialog with Cancel.
44
45use std::path::{Path, PathBuf};
46
47/// What the clipboard is asking a paste to do.
48#[derive(Clone, Copy, PartialEq, Eq, Debug)]
49pub enum Effect {
50    Copy,
51    Move,
52}
53
54/// What is on the clipboard, if it is files.
55pub struct Pasteable {
56    pub items: Vec<PathBuf>,
57    pub effect: Effect,
58}
59
60/// `DROPEFFECT_COPY` and `DROPEFFECT_MOVE`, which are the values in the preferred effect and
61/// are also what a drag reports.
62///
63/// Windows numbers them `NONE = 0, COPY = 1, MOVE = 2, LINK = 4` — copy first. These were
64/// written as `1 << 1` and `1 << 0`, which is the same pair of bits the other way round, and the
65/// consequence was not subtle: a copy taken here announced itself to every other program as a
66/// **cut**, a cut announced itself as a copy, and reading somebody else’s copy came back as a
67/// move. Copy a file in Explorer, paste it here, and the original was *gone*.
68///
69/// Taken from the Win32 headers now rather than written out, so they cannot be transposed again.
70#[cfg(windows)]
71const DROPEFFECT_COPY: u32 = windows::Win32::System::Ole::DROPEFFECT_COPY.0;
72#[cfg(windows)]
73const DROPEFFECT_MOVE: u32 = windows::Win32::System::Ole::DROPEFFECT_MOVE.0;
74
75/// Put files on the clipboard.
76///
77/// `Effect::Move` is a cut: nothing is moved here, and nothing will be unless
78/// something pastes.
79pub fn put(paths: &[PathBuf], effect: Effect) -> Result<(), String> {
80    #[cfg(windows)]
81    {
82        win::put(paths, effect)
83    }
84    #[cfg(not(windows))]
85    {
86        let _ = (paths, effect);
87        Err("The clipboard is implemented against the Windows shell only".to_owned())
88    }
89}
90
91/// Whether there is anything a paste could act on.
92///
93/// Cheap enough to ask once a frame, which is what greys out the menu entry.
94pub fn has_files() -> bool {
95    #[cfg(windows)]
96    {
97        win::has_files()
98    }
99    #[cfg(not(windows))]
100    {
101        false
102    }
103}
104
105/// Read the files off the clipboard, and what to do with them.
106pub fn get() -> Option<Pasteable> {
107    #[cfg(windows)]
108    {
109        win::get()
110    }
111    #[cfg(not(windows))]
112    {
113        None
114    }
115}
116
117/// [`win::settle`], for the tests outside this module.
118#[cfg(test)]
119pub fn settle_for_tests() {
120    #[cfg(windows)]
121    win::settle();
122}
123
124/// How many times the clipboard has changed, which is how to tell whether it is still the
125/// one you were looking at.
126///
127/// Cheap, and it takes no lock at all.
128pub fn sequence() -> u32 {
129    #[cfg(windows)]
130    {
131        use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
132        // SAFETY: a pure query.
133        unsafe { GetClipboardSequenceNumber() }
134    }
135    #[cfg(not(windows))]
136    {
137        0
138    }
139}
140
141/// Finish a cut whose paste has just succeeded: tell the source, then empty the clipboard.
142///
143/// `was` is the sequence number from when the paste started. If the clipboard has changed
144/// since — the user copied something else while the shell was still working — nothing happens,
145/// because emptying *that* would be this program throwing away data it was never given.
146///
147/// Identified by sequence number rather than by comparing the items, which is what this did
148/// first and what did not work: a *successful* move leaves the clipboard naming files that are
149/// no longer there, the data object can no longer render them, and the comparison that was
150/// supposed to prove "still the same cut" instead read nothing and concluded "something else".
151/// So the cut stayed on the clipboard after being pasted, ready to move files that had already
152/// moved. The sequence number is a fact about the clipboard rather than about the files.
153pub fn cut_pasted(was: u32) {
154    #[cfg(windows)]
155    {
156        win::cut_pasted(was);
157    }
158    #[cfg(not(windows))]
159    let _ = was;
160}
161
162/// Empty the clipboard, retrying like every other call here.
163///
164/// It was `let _ = OleSetClipboard(None)`, and the swallowed error was not academic: emptying
165/// the clipboard at the end of a cut lost the same race everything else here loses, silently,
166/// so a cut that had been pasted stayed on the clipboard and Ctrl+V would move files that had
167/// already moved. Found by the end-to-end test, which asserted the clipboard was empty and
168/// found it was not.
169///
170/// Only the tests reach for this directly; the program empties the clipboard as the last step
171/// of finishing a cut, inside [`cut_pasted`].
172#[cfg(test)]
173pub fn clear() {
174    #[cfg(windows)]
175    {
176        win::clear();
177    }
178}
179
180#[cfg(windows)]
181mod win {
182    use super::*;
183    use windows::core::{Interface, PCWSTR};
184    use windows::Win32::Foundation::HANDLE;
185    use windows::Win32::System::Com::{
186        IDataObject, DATADIR_GET, FORMATETC, STGMEDIUM, TYMED_HGLOBAL,
187    };
188    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
189    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
190    use windows::Win32::System::Ole::{OleGetClipboard, OleSetClipboard};
191    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
192    use windows::Win32::UI::Shell::{
193        SHCreateShellItemArrayFromIDLists, SHParseDisplayName, BHID_DataObject,
194    };
195
196    /// `CF_HDROP`, which is a fixed value rather than a registered one.
197    const CF_HDROP: u16 = 15;
198
199    /// The registered format that says whether a paste should copy or move.
200    fn preferred_effect_format() -> u16 {
201        // SAFETY: registering the same name twice returns the same id, so this is safe
202        // to call as often as it is convenient.
203        let id = unsafe { RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect")) };
204        id as u16
205    }
206
207    /// A PIDL that frees itself.
208    struct Pidl(*mut ITEMIDLIST);
209
210    impl Drop for Pidl {
211        fn drop(&mut self) {
212            if !self.0.is_null() {
213                // SAFETY: allocated by `SHParseDisplayName`, freed exactly once.
214                unsafe { windows::Win32::UI::Shell::ILFree(Some(self.0)) };
215            }
216        }
217    }
218
219    fn pidl_of(path: &Path) -> Option<Pidl> {
220        let wide = crate::shell::wide(path);
221        let mut raw: *mut ITEMIDLIST = std::ptr::null_mut();
222        // SAFETY: `wide` is null-terminated and outlives the call; `raw` is only read
223        // when the call succeeded.
224        let ok = unsafe {
225            SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut raw, 0, None).is_ok()
226        };
227        (ok && !raw.is_null()).then_some(Pidl(raw))
228    }
229
230    /// The shell's own data object for these paths, carrying the cut-or-copy flag.
231    ///
232    /// Separate from [`put`] because it is the part with the decisions in it — the shell's
233    /// object rather than a hand-rolled `CF_HDROP`, plus the `Preferred DropEffect` block —
234    /// and because it can then be built and read back in a test without going anywhere near
235    /// the one clipboard the whole desktop shares.
236    pub fn data_object(paths: &[PathBuf], effect: Effect) -> Result<IDataObject, String> {
237        if paths.is_empty() {
238            return Err("Nothing selected".to_owned());
239        }
240
241        // Built from the items themselves, so it offers every format Explorer offers —
242        // not just the one this program knows about.
243        let pidls: Vec<Pidl> = paths.iter().filter_map(|p| pidl_of(p)).collect();
244        if pidls.is_empty() {
245            return Err("Those items could not be resolved".to_owned());
246        }
247        let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| p.0 as *const _).collect();
248
249        // SAFETY: the PIDLs outlive the array, which copies what it needs; the data
250        // object is reference counted from here on.
251        unsafe {
252            let array = SHCreateShellItemArrayFromIDLists(&raw)
253                .map_err(|e| format!("The shell refused: {}", e.message()))?;
254            let data: IDataObject = array
255                .BindToHandler(None, &BHID_DataObject)
256                .map_err(|e| format!("The shell refused: {}", e.message()))?;
257
258            // Cut or copy. Without this the target has to guess, and guesses copy.
259            let value = match effect {
260                Effect::Copy => DROPEFFECT_COPY,
261                Effect::Move => DROPEFFECT_MOVE,
262            };
263            if let Some(medium) = global_dword(value) {
264                let format = FORMATETC {
265                    cfFormat: preferred_effect_format(),
266                    ptd: std::ptr::null_mut(),
267                    dwAspect: 1, // DVASPECT_CONTENT
268                    lindex: -1,
269                    tymed: TYMED_HGLOBAL.0 as u32,
270                };
271                // `release: true` hands the block to the data object, which frees it.
272                let _ = data.SetData(&format, &medium, true);
273            }
274            Ok(data)
275        }
276    }
277
278    pub fn put(paths: &[PathBuf], effect: Effect) -> Result<(), String> {
279        let data = data_object(paths, effect)?;
280        // The clipboard is a single global lock, and `OleSetClipboard` fails outright if
281        // anything else holds it — a clipboard manager polling it, an Office add-in, the
282        // previous owner not having let go yet. Contention is normal and transient, so this
283        // retries rather than reporting a failure the user can do nothing about.
284        // Microsoft's own guidance for `OpenClipboard` says the same.
285        //
286        // SAFETY: `data` is a live reference-counted object owned by this scope.
287        unsafe {
288            retrying(|| OleSetClipboard(&data)).map_err(|_| {
289                // Deliberately not the `HRESULT`'s own words: `CLIPBRD_E_CANT_OPEN` renders as
290                // "OpenClipboard failed", which tells the user nothing they can act on.
291                "Another program is holding the clipboard — try again".to_owned()
292            })
293        }
294    }
295
296    /// Read a data object the way a paste does — which is all [`get`] is, once the
297    /// clipboard has handed the object over.
298    pub fn read(data: &IDataObject) -> Option<Pasteable> {
299        // SAFETY: every medium taken below is released by the readers.
300        unsafe {
301            // The shell first, so a copy taken from inside an archive is not invisible; see
302            // the note at the top of this file.
303            let items = read_shell_items(data).or_else(|| read_hdrop(data))?;
304            if items.is_empty() {
305                return None;
306            }
307            let effect = read_effect(data).unwrap_or(Effect::Copy);
308            Some(Pasteable { items, effect })
309        }
310    }
311
312    /// Whatever the data object is offering, as parsing names.
313    ///
314    /// Works for anything the shell can name, which includes things that are not files:
315    /// an entry inside a `.zip` comes back as `C:\bundle.zip\inner.txt`, and
316    /// `SHCreateItemFromParsingName` takes that straight back.
317    unsafe fn read_shell_items(data: &IDataObject) -> Option<Vec<PathBuf>> {
318        use windows::Win32::System::Com::CoTaskMemFree;
319        use windows::Win32::UI::Shell::{
320            IShellItemArray, SHCreateShellItemArrayFromDataObject, SIGDN_DESKTOPABSOLUTEPARSING,
321        };
322
323        let array: IShellItemArray = SHCreateShellItemArrayFromDataObject(data).ok()?;
324        let count = array.GetCount().ok()?;
325        let mut items = Vec::with_capacity(count as usize);
326        for index in 0..count {
327            let Ok(shell_item) = array.GetItemAt(index) else {
328                continue;
329            };
330            let Ok(name) = shell_item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING) else {
331                continue;
332            };
333            if !name.is_null() {
334                if let Ok(text) = name.to_string() {
335                    items.push(PathBuf::from(text));
336                }
337                // The shell allocated it; this frees it.
338                CoTaskMemFree(Some(name.0 as *const std::ffi::c_void));
339            }
340        }
341        (!items.is_empty()).then_some(items)
342    }
343
344    /// `CFSTR_PASTESUCCEEDED`, registered the same way the preferred effect is.
345    fn paste_succeeded_format() -> u16 {
346        // SAFETY: registering the same name twice returns the same id.
347        unsafe { RegisterClipboardFormatW(windows::core::w!("Paste Succeeded")) as u16 }
348    }
349
350    pub fn cut_pasted(was: u32) {
351        // Still the same clipboard? If the user copied something else while the shell was
352        // working, that is theirs and this leaves it alone.
353        if super::sequence() != was {
354            return;
355        }
356        // SAFETY: the data object is reference counted, and the block handed to `SetData`
357        // is released by it.
358        unsafe {
359            let Ok(data) = retrying(|| OleGetClipboard()) else {
360                return;
361            };
362            if let Some(medium) = global_dword(DROPEFFECT_MOVE) {
363                let format = FORMATETC {
364                    cfFormat: paste_succeeded_format(),
365                    ptd: std::ptr::null_mut(),
366                    dwAspect: 1,
367                    lindex: -1,
368                    tymed: TYMED_HGLOBAL.0 as u32,
369                };
370                let _ = data.SetData(&format, &medium, true);
371            }
372            clear();
373        }
374    }
375
376    /// Let go of whatever this process has on the clipboard, and answer the calls that come
377    /// of having had it there.
378    ///
379    /// For tests only, and it earns its place. Each of them takes over the desktop's one
380    /// clipboard, and a test that leaves a live data object on it leaves the *next* test in the
381    /// same process answering calls about it — which is how four clipboard tests that each pass
382    /// alone managed to fail two at a time when run together. Production never has this
383    /// problem: one copy per keystroke, and a window pumping between them.
384    #[cfg(test)]
385    pub fn settle() {
386        clear();
387        // SAFETY: answering calls, which is all this does.
388        unsafe {
389            answering_calls(250);
390        }
391    }
392
393    /// Empty the clipboard. A null data object is the documented way.
394    pub fn clear() {
395        // SAFETY: as patient as every other call here, and for the same reason.
396        unsafe {
397            let _ = retrying(|| OleSetClipboard(None));
398        }
399    }
400
401
402
403    /// Retry a clipboard call while it is only losing a race.
404    ///
405    /// The clipboard is one lock shared by the whole desktop, and on a normal Windows install
406    /// several things take it the instant its contents change — clipboard history first among
407    /// them. Losing that race is ordinary and transient, and the only sensible answer is to
408    /// wait and ask again; Microsoft's own guidance for `OpenClipboard` says the same.
409    ///
410    /// The window was ten attempts over 150 ms, which measurement showed to be too narrow:
411    /// `probe_consecutive_puts` hammering the clipboard produced `CLIPBRD_E_CANT_OPEN` on the
412    /// second put and then on every put after it, pumping or not. That matters more than it
413    /// looks — a *failed* copy leaves the previous contents in place, so the next paste
414    /// quietly pastes the wrong files, which is the shape of a bug that loses somebody's work.
415    /// Thirty attempts over about a second outlasts it, and a second is only ever spent when
416    /// something is genuinely holding on.
417    unsafe fn retrying<T>(
418        mut attempt: impl FnMut() -> windows::core::Result<T>,
419    ) -> windows::core::Result<T> {
420        const TRIES: u32 = 30;
421        let mut last = attempt();
422        for step in 1..TRIES {
423            if last.is_ok() {
424                return last;
425            }
426            // Backing off, capped: quick at first, because most contention is over in a
427            // millisecond or two, and then patient.
428            let wait = (2 + step as u64 * 3).min(60);
429            answering_calls(wait);
430            last = attempt();
431        }
432        last
433    }
434
435    /// Waiting that answers what this apartment owes; the reasoning lives on
436    /// [`crate::shell::answering_calls`], because more than the clipboard depends on it.
437    ///
438    /// Measured here, by `one_copy_after_another_keeps_working`: sleeping between retries
439    /// refused eight to eleven of twelve copies made 200 ms apart -- about how long Windows'
440    /// clipboard history takes to come asking -- and refused nothing at all when they came back
441    /// to back, before it had started. Answering during the wait: nought out of forty-eight.
442    unsafe fn answering_calls(ms: u64) {
443        crate::shell::answering_calls(ms);
444    }
445
446    /// A `DWORD` in a moveable global block, which is what `SetData` wants.
447    unsafe fn global_dword(value: u32) -> Option<STGMEDIUM> {
448        let handle = GlobalAlloc(GMEM_MOVEABLE, 4).ok()?;
449        let locked = GlobalLock(handle);
450        if locked.is_null() {
451            return None;
452        }
453        std::ptr::copy_nonoverlapping(value.to_le_bytes().as_ptr(), locked.cast(), 4);
454        let _ = GlobalUnlock(handle);
455        Some(STGMEDIUM {
456            tymed: TYMED_HGLOBAL.0 as u32,
457            u: windows::Win32::System::Com::STGMEDIUM_0 {
458                hGlobal: handle,
459            },
460            pUnkForRelease: std::mem::ManuallyDrop::new(None),
461        })
462    }
463
464    pub fn has_files() -> bool {
465        use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
466        // SAFETY: a pure query; it opens nothing and allocates nothing.
467        unsafe { IsClipboardFormatAvailable(CF_HDROP as u32).is_ok() }
468    }
469
470    pub fn get() -> Option<Pasteable> {
471        // The whole read is retried, not just getting hold of the object. Getting the object
472        // is a lock; *reading* it is a call into whoever owns it, and that can fail on its own
473        // — measured, when Windows' clipboard history happened to be rendering the same object
474        // at the same moment. Retrying only the first half left a paste reporting "there are
475        // no files on the clipboard" for files that were plainly on it.
476        //
477        // SAFETY: the data object is reference counted, and `read` releases every medium it
478        // takes.
479        unsafe {
480            for step in 0..READ_TRIES {
481                if let Ok(data) = OleGetClipboard() {
482                    if let Some(pasteable) = read(&data) {
483                        return Some(pasteable);
484                    }
485                }
486                // An empty clipboard is an answer, not a race, and waiting a second to say so
487                // would make Ctrl+V feel broken. `IsClipboardFormatAvailable` takes no lock.
488                if !has_files() {
489                    return None;
490                }
491                answering_calls((2 + step * 3).min(60));
492            }
493            None
494        }
495    }
496
497    /// How many times a read is worth trying. Fewer than a write: a paste that cannot read the
498    /// clipboard has a sensible thing to say, whereas a copy that cannot write it has left the
499    /// *previous* contents in place and the next paste would use them.
500    const READ_TRIES: u64 = 12;
501
502    unsafe fn read_hdrop(data: &IDataObject) -> Option<Vec<PathBuf>> {
503        use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
504
505        let format = FORMATETC {
506            cfFormat: CF_HDROP,
507            ptd: std::ptr::null_mut(),
508            dwAspect: 1,
509            lindex: -1,
510            tymed: TYMED_HGLOBAL.0 as u32,
511        };
512        let _ = DATADIR_GET;
513        let medium = data.GetData(&format).ok()?;
514        let handle = medium.u.hGlobal;
515        if handle.is_invalid() {
516            return None;
517        }
518
519        let drop = HDROP(handle.0);
520        // Index `0xFFFF_FFFF` asks how many there are rather than for one of them.
521        let count = DragQueryFileW(drop, u32::MAX, None);
522        let mut items = Vec::with_capacity(count as usize);
523        for index in 0..count {
524            // Ask for the length first: a path can be longer than `MAX_PATH`, and a
525            // fixed buffer would silently truncate one.
526            let len = DragQueryFileW(drop, index, None);
527            if len == 0 {
528                continue;
529            }
530            let mut buffer = vec![0u16; len as usize + 1];
531            let written = DragQueryFileW(drop, index, Some(&mut buffer));
532            if written > 0 {
533                buffer.truncate(written as usize);
534                items.push(PathBuf::from(String::from_utf16_lossy(&buffer)));
535            }
536        }
537
538        // `ReleaseStgMedium` is what frees the block the clipboard handed over.
539        let mut medium = medium;
540        windows::Win32::System::Ole::ReleaseStgMedium(&mut medium);
541        Some(items)
542    }
543
544    unsafe fn read_effect(data: &IDataObject) -> Option<Effect> {
545        let format = FORMATETC {
546            cfFormat: preferred_effect_format(),
547            ptd: std::ptr::null_mut(),
548            dwAspect: 1,
549            lindex: -1,
550            tymed: TYMED_HGLOBAL.0 as u32,
551        };
552        let medium = data.GetData(&format).ok()?;
553        let handle = medium.u.hGlobal;
554        let mut effect = None;
555        if !handle.is_invalid() {
556            let locked = GlobalLock(handle);
557            if !locked.is_null() {
558                let mut bytes = [0u8; 4];
559                std::ptr::copy_nonoverlapping(locked.cast::<u8>(), bytes.as_mut_ptr(), 4);
560                let value = u32::from_le_bytes(bytes);
561                // A source may offer both; a move is the more specific intent, so it
562                // wins. This is what Explorer does with the same pair of bits.
563                effect = Some(if value & DROPEFFECT_MOVE != 0 {
564                    Effect::Move
565                } else {
566                    Effect::Copy
567                });
568                let _ = GlobalUnlock(handle);
569            }
570        }
571        let mut medium = medium;
572        windows::Win32::System::Ole::ReleaseStgMedium(&mut medium);
573        let _ = HANDLE::default();
574        effect
575    }
576
577    /// The interface id lookup the array bind needs, kept honest.
578    #[allow(dead_code)]
579    fn data_object_iid() -> windows::core::GUID {
580        IDataObject::IID
581    }
582}
583
584#[cfg(all(test, windows))]
585mod tests {
586    use super::*;
587
588    /// A round trip through the real clipboard, which is the only test worth having
589    /// here: it is interoperability that matters, and interoperability with a mock is
590    /// not a fact about anything.
591    /// The data object a cut or a copy hands over: the shell's own, with `CF_HDROP` and
592    /// the `Preferred DropEffect` block that tells the target which of the two it was.
593    ///
594    /// Built and read back directly rather than through the clipboard. The clipboard is one
595    /// object shared by every process on the desktop — Windows' own history service reads
596    /// each new item the moment it lands — and a test that goes through it is a test that
597    /// fails one run in four for reasons that have nothing to do with this program. What is
598    /// worth checking is the object, and that needs no lock at all.
599    #[test]
600    #[cfg(windows)]
601    fn the_data_object_carries_the_files_and_the_effect() {
602        let _serialised = crate::shell::serialised();
603        crate::shell::init();
604
605        let mut here = std::env::temp_dir();
606        here.push(format!("yafe-clip-{}", std::process::id()));
607        std::fs::create_dir_all(&here).expect("temp dir");
608        let one = here.join("one.txt");
609        let two = here.join("two.txt");
610        std::fs::write(&one, b"1").expect("write");
611        std::fs::write(&two, b"2").expect("write");
612
613        for effect in [Effect::Copy, Effect::Move] {
614            let data = win::data_object(&[one.clone(), two.clone()], effect)
615                .expect("the shell should build a data object for two real files");
616            let read = win::read(&data).expect("and it should read back as files");
617            assert_eq!(
618                read.effect, effect,
619                "a cut has to come back as a cut, or a paste would copy when it should move"
620            );
621            assert_eq!(read.items.len(), 2);
622            // The shell normalises the case of what it hands back, so compare that way.
623            let names: Vec<String> = read
624                .items
625                .iter()
626                .map(|p| {
627                    p.file_name()
628                        .map(|n| n.to_string_lossy().to_lowercase())
629                        .unwrap_or_default()
630                })
631                .collect();
632            assert!(names.contains(&"one.txt".to_owned()), "{names:?}");
633            assert!(names.contains(&"two.txt".to_owned()), "{names:?}");
634        }
635
636        assert!(
637            win::data_object(&[], Effect::Copy).is_err(),
638            "and nothing is not something to put on a clipboard"
639        );
640        let _ = std::fs::remove_dir_all(&here);
641    }
642
643    /// The same, but through the real clipboard, which is what a paste into Explorer
644    /// actually uses.
645    ///
646    /// Ignored by default: it needs the desktop's one clipboard to stay still for a moment,
647    /// and nothing on a working machine promises that. Run it on purpose:
648    ///
649    /// ```text
650    /// cargo test -- --ignored the_real_clipboard
651    /// ```
652    #[test]
653    #[ignore]
654    #[cfg(windows)]
655    fn the_real_clipboard_round_trips() {
656        let _serialised = crate::shell::serialised();
657        crate::shell::init();
658        win::settle();
659
660        let mut here = std::env::temp_dir();
661        here.push(format!("yafe-clip-real-{}", std::process::id()));
662        std::fs::create_dir_all(&here).expect("temp dir");
663        let one = here.join("one.txt");
664        std::fs::write(&one, b"1").expect("write");
665
666        put(std::slice::from_ref(&one), Effect::Move).expect("put on the clipboard");
667        assert!(has_files(), "the clipboard should be offering files");
668        let read = get().expect("read back off the clipboard");
669        assert_eq!(read.effect, Effect::Move);
670        assert_eq!(read.items.len(), 1);
671
672        clear();
673        let _ = std::fs::remove_dir_all(&here);
674    }
675
676    /// A zip holding one stored `inner.txt`, so the test that needs a non-file shell item
677    /// need no archiver.
678    ///
679    /// Made by `zipfile` and pasted in: a hand-written one had a bad central directory, which
680    /// Windows treats as a plain file rather than a folder -- and the probe that found this
681    /// duly reported that the shell could not resolve a path inside a zip.
682    #[cfg(all(test, windows))]
683    const ZIP_WITH_ONE_ENTRY: &[u8] = &[
684        0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0xb2, 0x8e, 0x03, 0x5d, 0x86, 0xa6, 0x10, 0x36, 0x05, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x69, 0x6e, 0x6e, 0x65, 0x72, 0x2e, 0x74, 0x78, 0x74, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x00, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0xb2, 0x8e, 0x03, 0x5d, 0x86, 0xa6, 0x10, 0x36, 0x05, 0x00, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00, 0x00, 0x00, 0x00, 0x69, 0x6e, 0x6e, 0x65, 0x72, 0x2e, 0x74, 0x78, 0x74, 0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x37, 0x00, 0x00, 0x00, 0x2c, 0x00, 0x00, 0x00, 0x00, 0x00,
685    ];
686
687    /// Copying something that is not a file: an entry inside a `.zip`.
688    ///
689    /// This is the case that made a paste read the shell rather than `CF_HDROP`. Explorer
690    /// offers `Shell IDList Array`, `FileGroupDescriptorW` and `FileContents` for an archive
691    /// entry and no `CF_HDROP` at all, so a paste that only knew `CF_HDROP` read nothing back
692    /// and did nothing at all -- copy a file out of a zip in Explorer, press Ctrl+V here, and
693    /// the answer was silence.
694    ///
695    /// What is asserted is the whole way through: the data object reads back as the item, and
696    /// the name it reads back as is one the shell can resolve again, which is what lets
697    /// `IFileOperation` extract it.
698    #[test]
699    #[cfg(windows)]
700    fn an_entry_inside_a_zip_reads_back_as_something_pasteable() {
701        let _serialised = crate::shell::serialised();
702        crate::shell::init();
703
704        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
705            .join("target")
706            .join("sandbox")
707            .join("zip");
708        let _ = std::fs::remove_dir_all(&root);
709        std::fs::create_dir_all(&root).expect("sandbox");
710        let zip = root.join("bundle.zip");
711        std::fs::write(&zip, ZIP_WITH_ONE_ENTRY).expect("write the zip");
712        let inside = zip.join("inner.txt");
713
714        let data = win::data_object(std::slice::from_ref(&inside), Effect::Copy)
715            .expect("the shell should build a data object for an item inside an archive");
716        let read = win::read(&data).expect(
717            "and a paste should read it back -- if this is None, the read has gone back to \
718             `CF_HDROP`, which an archive entry does not offer",
719        );
720        assert_eq!(read.items.len(), 1, "{:?}", read.items);
721
722        // The name has to be one the shell can resolve, since that is what the copy engine
723        // is handed on the other side.
724        let name = &read.items[0];
725        assert!(
726            name.to_string_lossy().to_lowercase().contains("bundle.zip"),
727            "expected a path through the archive, got {}",
728            name.display()
729        );
730        // SAFETY: a pure lookup; nothing is retained.
731        unsafe {
732            assert!(
733                crate::shell::ops::item(name).is_ok(),
734                "the shell cannot resolve {} back, so a paste of it would fail",
735                name.display()
736            );
737        }
738
739        let _ = std::fs::remove_dir_all(&root);
740    }
741
742    /// Interoperability, across a process boundary, in both directions.
743    ///
744    /// The claim worth checking is not that this program can read its own clipboard -- it is
745    /// that *another* process sees what it puts there, and that it sees what another process
746    /// puts. PowerShell stands in for Explorer: `Set-Clipboard -Path` writes `CF_HDROP` the
747    /// same way a copy in a folder window does, and `Get-Clipboard -Format FileDropList`
748    /// reads it back the same way a paste does.
749    ///
750    /// Ignored by default, because it takes over the desktop's one clipboard. Run it on
751    /// purpose:
752    ///
753    /// ```text
754    /// cargo test -- --ignored --test-threads=1 explorer_and_this_program
755    /// ```
756    #[test]
757    #[ignore = "takes over the real clipboard; run explicitly"]
758    #[cfg(windows)]
759    fn explorer_and_this_program_read_each_other_s_clipboard() {
760        let _serialised = crate::shell::serialised();
761        crate::shell::init();
762        win::settle();
763
764        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
765            .join("target")
766            .join("sandbox")
767            .join("interop");
768        let _ = std::fs::remove_dir_all(&root);
769        std::fs::create_dir_all(&root).expect("sandbox");
770        let one = root.join("one.txt");
771        let two = root.join("two.txt");
772        std::fs::write(&one, b"1").expect("write");
773        std::fs::write(&two, b"2").expect("write");
774
775        /// Run a snippet of PowerShell in its own STA, pumping messages while it runs.
776        ///
777        /// The pumping is not incidental. `OleSetClipboard` does not copy anything: it leaves
778        /// the clipboard holding a reference to the data object *in this process*, and another
779        /// process asking for the bytes is a marshalled call back into this apartment, which
780        /// arrives as a window message. A thread that is not dispatching messages therefore
781        /// hands out nothing at all -- which is exactly what the first version of this test
782        /// measured, and it would have been wrong to conclude from it that interoperability
783        /// was broken. The real window pumps continuously.
784        fn powershell(script: &str) -> String {
785            use windows::Win32::UI::WindowsAndMessaging::{
786                DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
787            };
788
789            let mut child = std::process::Command::new("powershell")
790                .args(["-NoProfile", "-STA", "-Command", script])
791                .stdout(std::process::Stdio::piped())
792                .spawn()
793                .expect("powershell should be on the path");
794
795            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
796            while std::time::Instant::now() < deadline {
797                if child.try_wait().ok().flatten().is_some() {
798                    break;
799                }
800                // SAFETY: a plain pump over this thread's own queue.
801                unsafe {
802                    let mut message = MSG::default();
803                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
804                        let _ = TranslateMessage(&message);
805                        DispatchMessageW(&message);
806                    }
807                }
808                std::thread::sleep(std::time::Duration::from_millis(10));
809            }
810
811            let out = child.wait_with_output().expect("powershell finished");
812            String::from_utf8_lossy(&out.stdout).trim().to_owned()
813        }
814
815        // ---- this program copies, another process pastes ----
816        put(&[one.clone(), two.clone()], Effect::Copy).expect("put on the clipboard");
817        let seen = powershell(
818            "(Get-Clipboard -Format FileDropList | ForEach-Object { $_.Name }) -join ','",
819        );
820        assert!(
821            seen.to_lowercase().contains("one.txt") && seen.to_lowercase().contains("two.txt"),
822            "another process read `{seen}` off the clipboard, not the two files put there"
823        );
824
825        // ---- another process copies, this program pastes ----
826        let script = format!(
827            "Set-Clipboard -Path '{}','{}'",
828            one.display(),
829            two.display()
830        );
831        powershell(&script);
832        let read = get().expect("this program should read a clipboard another process wrote");
833        assert_eq!(read.effect, Effect::Copy, "no preferred effect means copy");
834        let mut names: Vec<String> = read
835            .items
836            .iter()
837            .map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_lowercase())
838            .collect();
839        names.sort();
840        assert_eq!(names, ["one.txt", "two.txt"], "{:?}", read.items);
841
842        clear();
843        let _ = std::fs::remove_dir_all(&root);
844    }
845
846    /// The end of a cut: the clipboard is emptied, and only when it is still the same cut.
847    #[test]
848    #[ignore = "takes over the real clipboard; run explicitly"]
849    #[cfg(windows)]
850    fn a_cut_that_has_been_pasted_empties_the_clipboard() {
851        let _serialised = crate::shell::serialised();
852        crate::shell::init();
853        win::settle();
854
855        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
856            .join("target")
857            .join("sandbox")
858            .join("cut");
859        let _ = std::fs::remove_dir_all(&root);
860        std::fs::create_dir_all(&root).expect("sandbox");
861        let one = root.join("one.txt");
862        let other = root.join("other.txt");
863        std::fs::write(&one, b"1").expect("write");
864        std::fs::write(&other, b"2").expect("write");
865
866        // A clipboard that has changed since the paste began is not this program's to throw
867        // away, however much it looks like the one it was given.
868        put(std::slice::from_ref(&one), Effect::Move).expect("put");
869        let stale = sequence();
870        put(std::slice::from_ref(&other), Effect::Move).expect("put something else");
871        cut_pasted(stale);
872        assert!(
873            has_files(),
874            "the clipboard moved on between the paste and its finish, and this emptied it \
875             anyway -- that is somebody else's data"
876        );
877
878        // The real thing: the same clipboard the paste was given.
879        put(std::slice::from_ref(&one), Effect::Move).expect("put");
880        let ours = sequence();
881        assert!(has_files());
882        cut_pasted(ours);
883        assert!(
884            !has_files(),
885            "a cut that has been pasted has to leave the clipboard empty, or Ctrl+V would \
886             move files that are no longer there"
887        );
888
889        clear();
890        let _ = std::fs::remove_dir_all(&root);
891    }
892
893    /// Puts two files on the clipboard and returns, so the process exits with them on it.
894    ///
895    /// Half of a test: the other half is another process reading the clipboard afterwards.
896    /// Driven from the shell, because what is being checked is what survives *this* program
897    /// closing, and that cannot be checked from inside it:
898    ///
899    /// ```text
900    /// cargo test --release -- --ignored --exact \
901    ///   shell::clipboard::tests::leaves_a_copy_behind_and_exits
902    /// powershell -NoProfile -STA -Command "Get-Clipboard -Format FileDropList"
903    /// ```
904    #[test]
905    #[ignore = "half of a cross-process check; see the note"]
906    #[cfg(windows)]
907    fn leaves_a_copy_behind_and_exits() {
908        let _serialised = crate::shell::serialised();
909        crate::shell::init();
910
911        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
912            .join("target")
913            .join("sandbox")
914            .join("survives");
915        let _ = std::fs::remove_dir_all(&root);
916        std::fs::create_dir_all(&root).expect("sandbox");
917        let one = root.join("survivor.txt");
918        std::fs::write(&one, b"1").expect("write");
919
920        put(std::slice::from_ref(&one), Effect::Copy).expect("put on the clipboard");
921        if std::env::var_os("YAFE_NO_FLUSH").is_none() {
922            crate::shell::flush();
923        }
924        // Deliberately leaves the file: the reader on the other side names it.
925    }
926
927    /// One copy after another has to keep working, at every cadence.
928    ///
929    /// This is the regression test for the least obvious bug in this file. A copy leaves the
930    /// clipboard holding a reference to a data object *here*, so Windows' clipboard history
931    /// comes asking for the bytes a couple of hundred milliseconds later -- and it asks while
932    /// holding the clipboard. A thread that does not answer that call leaves the lock taken and
933    /// the next copy refused, and the `HRESULT` for it says `OpenClipboard failed`, which reads
934    /// like somebody else's fault.
935    ///
936    /// The shape is what gives it away, and it is the shape asserted here: back-to-back copies
937    /// were fine, because the history service had not started yet, and copies 200 ms apart
938    /// failed eight to eleven times in twelve. So a version of this test that only hammered
939    /// would pass against the bug. `answering_calls` is the fix; see the note on it.
940    ///
941    /// Ignored because it takes over the desktop's one clipboard.
942    #[test]
943    #[ignore = "takes over the real clipboard; run explicitly, single-threaded"]
944    #[cfg(windows)]
945    fn one_copy_after_another_keeps_working() {
946        let _serialised = crate::shell::serialised();
947        crate::shell::init();
948        win::settle();
949
950        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
951            .join("target")
952            .join("sandbox")
953            .join("consecutive");
954        let _ = std::fs::remove_dir_all(&root);
955        std::fs::create_dir_all(&root).expect("sandbox");
956        let a = root.join("a.txt");
957        let b = root.join("b.txt");
958        std::fs::write(&a, b"a").expect("write");
959        std::fs::write(&b, b"b").expect("write");
960
961        // 200 ms is the one that mattered, so it is in the list; the others are there because
962        // a fix that only worked at one cadence would not be a fix.
963        for gap in [0u64, 20, 50, 200] {
964            const TIMES: usize = 8;
965            for step in 0..TIMES {
966                let which = if step % 2 == 0 { &a } else { &b };
967                put(std::slice::from_ref(which), Effect::Copy).unwrap_or_else(|why| {
968                    panic!("copy {step} of {TIMES}, {gap} ms apart, was refused: {why}")
969                });
970                let read = get().unwrap_or_else(|| {
971                    panic!("copy {step} of {TIMES}, {gap} ms apart, read back as nothing")
972                });
973                assert_eq!(read.items.len(), 1);
974                std::thread::sleep(std::time::Duration::from_millis(gap));
975            }
976        }
977
978        clear();
979        let _ = std::fs::remove_dir_all(&root);
980    }
981
982    #[test]
983    fn putting_nothing_is_refused_rather_than_clearing() {
984        let _serialised = crate::shell::serialised();
985        assert!(put(&[], Effect::Copy).is_err());
986    }
987}
