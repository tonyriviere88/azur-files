1//! The directory model.
2//!
3//! A directory is two allocations: one string holding every name end to end, and
4//! one `Vec<Entry>` of 32-byte records pointing into it. That shape is the reason
5//! this explorer can hold a 200,000-entry folder without the scan showing up as a
6//! pause:
7//!
8//! - **One allocation for the names.** A `Vec<PathBuf>` would be one heap block
9//!   per entry, and the allocator, not the disk, would be the bottleneck.
10//! - **A fixed-size record.** 32 bytes means eight entries per cache line, so a
11//!   sort or a filter pass streams instead of chasing pointers.
12//! - **Raw values, not strings.** Sizes stay `u64` and times stay `FILETIME`, so
13//!   sorting compares integers and formatting happens only for the ~40 rows that
14//!   are actually on screen.
15//!
16//! A `Dir` is immutable once built, which is what lets it be an `Arc` shared by
17//! every tab looking at the same folder while each keeps its own sort order.
18
19use std::path::{Path, PathBuf};
20
21/// This entry is a directory.
22pub const FLAG_DIR: u16 = 1 << 0;
23/// `FILE_ATTRIBUTE_HIDDEN`.
24pub const FLAG_HIDDEN: u16 = 1 << 1;
25/// `FILE_ATTRIBUTE_SYSTEM`.
26pub const FLAG_SYSTEM: u16 = 1 << 2;
27/// A reparse point: symlink, junction or a cloud placeholder.
28pub const FLAG_LINK: u16 = 1 << 3;
29/// `FILE_ATTRIBUTE_READONLY`.
30pub const FLAG_READONLY: u16 = 1 << 4;
31
32/// One name in a [`Dir`], plus everything the details view and the sort need.
33///
34/// Laid out largest field first so the record is exactly 32 bytes with no
35/// interior padding, and the name is a slice of the directory's one string
36/// rather than an allocation of its own.
37#[derive(Clone, Copy, Debug)]
38pub struct Entry {
39    /// Bytes, from the find data. Meaningless for a directory, which is why the
40    /// details view leaves that cell blank rather than printing `0 B`.
41    pub size: u64,
42    /// Raw `FILETIME`: 100-nanosecond ticks since 1601-01-01 UTC. Kept raw so a
43    /// sort by date is an integer compare and no timezone maths happens for a row
44    /// nobody is looking at.
45    pub modified: u64,
46    /// Byte offset of the name in [`Dir::names`].
47    name_off: u32,
48    /// Byte length of the name. A path component is at most 255 UTF-16 units, so
49    /// this cannot overflow.
50    name_len: u16,
51    /// Byte offset *within the name* of the extension, past the dot. `0` means
52    /// there is none — an offset of 0 is impossible for a real extension, since
53    /// a leading dot makes a dotfile, not an extension.
54    ext_off: u16,
55    /// The `FLAG_*` set.
56    pub flags: u16,
57}
58
59impl Entry {
60    #[inline]
61    pub fn is_dir(&self) -> bool {
62        self.flags & FLAG_DIR != 0
63    }
64
65    #[inline]
66    pub fn is_hidden(&self) -> bool {
67        self.flags & (FLAG_HIDDEN | FLAG_SYSTEM) != 0
68    }
69
70    #[inline]
71    pub fn is_link(&self) -> bool {
72        self.flags & FLAG_LINK != 0
73    }
74}
75
76/// A scanned directory: immutable, shared, and the unit the cache stores.
77pub struct Dir {
78    /// The folder this is a listing of. Empty means the synthetic "This PC".
79    pub path: PathBuf,
80    /// Every name, end to end, unseparated. Only [`Dir::name`] indexes it.
81    names: String,
82    pub entries: Vec<Entry>,
83    /// Where an entry leads, for synthetic listings whose rows are not children
84    /// of `path` — the drives under "This PC". Empty for a real directory, where
85    /// [`Dir::target`] joins the name onto the path instead.
86    targets: Vec<PathBuf>,
87    pub dir_count: u32,
88    pub file_count: u32,
89    /// Summed size of the files (not the directories, whose `size` is noise).
90    pub total_size: u64,
91    /// Why the listing is empty, if it is empty because something went wrong.
92    pub error: Option<String>,
93    /// The read stopped at a limit rather than at the end of what is there.
94    ///
95    /// Only ever true for a flattened listing — [`crate::fs::scan::scan_deep`] — which
96    /// is the one read in this program that has no natural end. The status line says so,
97    /// because a listing that is missing rows and does not admit it is the one kind of
98    /// wrong answer a file manager must not give.
99    pub truncated: bool,
100    /// How long the scan took. Shown in the status bar, which is the only honest
101    /// way to claim the word "fast".
102    pub scan_micros: u64,
103}
104
105impl Dir {
106    /// The name of entry `i`.
107    #[inline]
108    pub fn name(&self, i: usize) -> &str {
109        let e = &self.entries[i];
110        let start = e.name_off as usize;
111        &self.names[start..start + e.name_len as usize]
112    }
113
114    /// The file's own name, without the folders in front of it.
115    ///
116    /// The same thing as [`Dir::name`] for an ordinary listing, where a name is one
117    /// path component. In a **flattened** listing — [`crate::fs::scan::scan_deep`] —
118    /// a name is the path relative to the folder being flattened, because that is
119    /// what a row has to show and what sorting and filtering have to see. This is
120    /// the part of it that is the file: what a rename edits, and what a row is
121    /// revealed by.
122    #[inline]
123    pub fn leaf(&self, i: usize) -> &str {
124        leaf_of(self.name(i))
125    }
126
127    /// The folders in front of entry `i`'s own name — where it is, relative to the
128    /// folder being listed.
129    ///
130    /// `""` in an ordinary listing, where a name is one component and every row is in
131    /// the folder on show. In a **flattened** one it is what the Name column shows after
132    /// the name, dimmed: `filelist.rs` is `ui`, and `heads` is `logs\refs`.
133    #[inline]
134    pub fn within(&self, i: usize) -> &str {
135        let name = self.name(i);
136        name[..name.len() - self.leaf(i).len()].trim_end_matches(['\\', '/'])
137    }
138
139    /// The extension of entry `i`, lowercase-insensitive as stored, without the
140    /// dot. `""` when there is none.
141    #[inline]
142    pub fn ext(&self, i: usize) -> &str {
143        let e = &self.entries[i];
144        if e.ext_off == 0 {
145            return "";
146        }
147        let start = e.name_off as usize + e.ext_off as usize;
148        let end = e.name_off as usize + e.name_len as usize;
149        &self.names[start..end]
150    }
151
152    /// Where opening entry `i` goes.
153    pub fn target(&self, i: usize) -> PathBuf {
154        if let Some(explicit) = self.targets.get(i) {
155            return explicit.clone();
156        }
157        self.path.join(self.name(i))
158    }
159
160    /// The target of entry `i` when it is *not* a child of this folder.
161    ///
162    /// `Some` only for a synthetic listing — the drives under This PC — and that is exactly
163    /// when a row's icon has to be asked about by path rather than by type: a volume's icon
164    /// is its own, and joining `Windows (C:)` onto an empty path names nothing at all, which
165    /// is why those rows drew the generic folder.
166    pub fn explicit_target(&self, i: usize) -> Option<&Path> {
167        self.targets
168            .get(i)
169            .filter(|target| !target.as_os_str().is_empty())
170            .map(PathBuf::as_path)
171    }
172
173    #[inline]
174    pub fn len(&self) -> usize {
175        self.entries.len()
176    }
177
178    #[inline]
179    pub fn is_empty(&self) -> bool {
180        self.entries.is_empty()
181    }
182
183    /// A directory that could not be read. Still a `Dir`, so the UI has one code
184    /// path: a failed listing is an empty one that knows why.
185    pub fn failed(path: impl Into<PathBuf>, error: impl Into<String>) -> Self {
186        Self {
187            path: path.into(),
188            names: String::new(),
189            entries: Vec::new(),
190            targets: Vec::new(),
191            dir_count: 0,
192            file_count: 0,
193            total_size: 0,
194            error: Some(error.into()),
195            truncated: false,
196            scan_micros: 0,
197        }
198    }
199}
200
201/// Accumulates a [`Dir`] one entry at a time.
202///
203/// The scanner pushes into this in find-order and never looks back, so both
204/// vectors grow forwards only and the name string is written once.
205pub struct DirBuilder {
206    path: PathBuf,
207    names: String,
208    entries: Vec<Entry>,
209    targets: Vec<PathBuf>,
210    dir_count: u32,
211    file_count: u32,
212    total_size: u64,
213}
214
215impl DirBuilder {
216    pub fn new(path: impl Into<PathBuf>) -> Self {
217        Self {
218            path: path.into(),
219            // A folder of a few hundred files is the common case and fits in one
220            // grow from here; a folder of 200k reaches its final size in ~12
221            // doublings, which is not where the time goes.
222            names: String::with_capacity(8 * 1024),
223            entries: Vec::with_capacity(256),
224            targets: Vec::new(),
225            dir_count: 0,
226            file_count: 0,
227            total_size: 0,
228        }
229    }
230
231    /// Reserve for a known entry count, when the caller has one.
232    pub fn reserve(&mut self, entries: usize) {
233        self.entries.reserve(entries);
234        self.names.reserve(entries * 16);
235    }
236
237    pub fn push(&mut self, name: &str, size: u64, modified: u64, flags: u16) {
238        let name_off = self.names.len() as u32;
239        self.names.push_str(name);
240        self.finish_entry(name_off, size, modified, flags);
241    }
242
243    /// How many entries are in the builder so far, for a caller working to a budget.
244    #[inline]
245    pub fn len(&self) -> usize {
246        self.entries.len()
247    }
248
249    /// Push a name that arrived as UTF-16, transcoding straight into the arena.
250    ///
251    /// The obvious `String::from_utf16_lossy` would be one heap allocation per
252    /// entry, thrown away immediately after being copied in — which for a folder
253    /// of 200,000 files is 200,000 malloc/free pairs, and more time than the
254    /// directory read itself. This writes the bytes where they are going.
255    #[cfg(windows)]
256    pub fn push_wide(&mut self, wide: &[u16], size: u64, modified: u64, flags: u16) {
257        let name_off = self.names.len() as u32;
258        {
259            // SAFETY: every byte pushed below is valid UTF-8. The fast path
260            // pushes single units already known to be < 0x80, and the tail goes
261            // through `char::encode_utf8`. The borrow ends with this block, so
262            // the string is a `String` again before anything else reads it.
263            let bytes = unsafe { self.names.as_mut_vec() };
264            bytes.reserve(wide.len());
265            // Nearly every file name on a Windows volume is ASCII, and the ones
266            // that are not are usually ASCII up to the first accent.
267            let mut i = 0;
268            while i < wide.len() && wide[i] < 0x80 {
269                bytes.push(wide[i] as u8);
270                i += 1;
271            }
272            if i < wide.len() {
273                let mut buf = [0u8; 4];
274                for c in char::decode_utf16(wide[i..].iter().copied()) {
275                    let c = c.unwrap_or(char::REPLACEMENT_CHARACTER);
276                    bytes.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
277                }
278            }
279        }
280        self.finish_entry(name_off, size, modified, flags);
281    }
282
283    /// Close the record for a name already appended at `name_off`.
284    fn finish_entry(&mut self, name_off: u32, size: u64, modified: u64, flags: u16) {
285        let name = &self.names[name_off as usize..];
286        let name_len = name.len() as u16;
287
288        let is_dir = flags & FLAG_DIR != 0;
289        // A directory's dots are part of its name, not an extension: `v1.2` is a
290        // folder, not a file of type `2`.
291        let ext_off = if is_dir {
292            0
293        } else {
294            // The extension is the last dot of the last *component*. A name here is
295            // usually one component and then the two are the same thing — but a
296            // flattened listing stores relative paths (see [`Dir::leaf`]), and
297            // `v1.2\README` has a dot in it that belongs to the folder.
298            let leaf = leaf_of(name);
299            // `rfind` rather than `split`: `archive.tar.gz` is a `gz`, which is
300            // what the shell thinks too.
301            match leaf.rfind('.') {
302                // A leading dot makes `.gitignore` a name, not an extension.
303                Some(0) | None => 0,
304                Some(dot) => (name.len() - leaf.len() + dot + 1) as u16,
305            }
306        };
307
308        if is_dir {
309            self.dir_count += 1;
310        } else {
311            self.file_count += 1;
312            self.total_size += size;
313        }
314
315        self.entries.push(Entry {
316            size,
317            modified,
318            name_off,
319            name_len,
320            ext_off,
321            flags,
322        });
323    }
324
325    /// Push an entry that leads somewhere other than `path/name` — a drive row
326    /// under "This PC".
327    pub fn push_link(&mut self, name: &str, target: PathBuf, size: u64, flags: u16) {
328        // `targets` is indexed in lockstep with `entries`, so a builder that
329        // mixes plain and linked entries has to pad it.
330        while self.targets.len() < self.entries.len() {
331            self.targets.push(PathBuf::new());
332        }
333        self.push(name, size, 0, flags);
334        self.targets.push(target);
335    }
336
337    /// Hand the arenas over as a finished listing, sized to what they hold.
338    ///
339    /// The reservations in [`DirBuilder::new`] and [`DirBuilder::reserve`] are exactly right
340    /// for *building* — a folder of a few hundred files fills without a single reallocation —
341    /// and exactly wrong for *holding*, because the listing then goes into a cache that keeps
342    /// scores of them. A 2-entry folder was keeping the 8KB of names and 256 records it was
343    /// given to start with: measured, 96 cached folders came to 1.6MB while holding about 300
344    /// entries between them. Growth by doubling means a large folder can be carrying an arena
345    /// up to twice the size of its contents, too.
346    ///
347    /// The cost is one reallocation and copy per folder read, on the worker thread that did
348    /// the reading, against a syscall per entry that has already happened.
349    pub fn finish(mut self, scan_micros: u64) -> Dir {
350        self.names.shrink_to_fit();
351        self.entries.shrink_to_fit();
352        self.targets.shrink_to_fit();
353        Dir {
354            path: self.path,
355            names: self.names,
356            entries: self.entries,
357            targets: self.targets,
358            dir_count: self.dir_count,
359            file_count: self.file_count,
360            total_size: self.total_size,
361            error: None,
362            truncated: false,
363            scan_micros,
364        }
365    }
366}
367
368/// The last component of a stored name. Free for the ordinary case, which has no
369/// separator in it at all.
370#[inline]
371fn leaf_of(name: &str) -> &str {
372    match name.rfind(['\\', '/']) {
373        Some(at) => &name[at + 1..],
374        None => name,
375    }
376}
377
378/// The display name of a folder, for a tab title or a breadcrumb segment.
379///
380/// A drive root has no file name of its own, so `C:\` would come back empty from
381/// [`Path::file_name`]; this gives `C:` instead. An empty path is "This PC".
382pub fn display_name(path: &Path) -> String {
383    if path.as_os_str().is_empty() {
384        return "This PC".to_owned();
385    }
386    if let Some(name) = path.file_name() {
387        return name.to_string_lossy().into_owned();
388    }
389    // A root: `C:\`, or `\\server\share`.
390    let text = path.to_string_lossy();
391    let trimmed = text.trim_end_matches(['\\', '/']);
392    if trimmed.is_empty() {
393        text.into_owned()
394    } else {
395        trimmed.to_owned()
396    }
397}
398
399#[cfg(test)]
400mod tests {
401    use super::*;
402
403    /// A name in a flattened listing is a relative path, and both the extension and the
404    /// leaf are properties of its *last component*.
405    ///
406    /// `v1.2\README` is the case that says why: a folder is allowed dots, so an extension
407    /// read off the whole name would make a file with none a `2\README`, and the Type
408    /// column, the type icon and the sort would all follow it there.
409    #[test]
410    fn an_extension_belongs_to_the_last_component_of_a_name() {
411        let mut builder = DirBuilder::new("D:\\root");
412        for (name, flags) in [
413            ("plain.txt", 0),
414            ("archive.tar.gz", 0),
415            (".gitignore", 0),
416            ("sub\\deep.rs", 0),
417            ("v1.2\\README", 0),
418            ("v1.2", FLAG_DIR),
419            ("sub\\nested", FLAG_DIR),
420        ] {
421            builder.push(name, 0, 0, flags);
422        }
423        let dir = builder.finish(0);
424        let ext_of = |want: &str| {
425            let i = (0..dir.len())
426                .find(|&i| dir.name(i) == want)
427                .expect("pushed above");
428            (dir.ext(i), dir.leaf(i))
429        };
430
431        assert_eq!(ext_of("plain.txt"), ("txt", "plain.txt"));
432        assert_eq!(ext_of("archive.tar.gz"), ("gz", "archive.tar.gz"));
433        assert_eq!(ext_of(".gitignore"), ("", ".gitignore"));
434        assert_eq!(ext_of("sub\\deep.rs"), ("rs", "deep.rs"));
435        assert_eq!(
436            ext_of("v1.2\\README"),
437            ("", "README"),
438            "the dot belongs to the folder, not to the file"
439        );
440        // A directory's dots are part of its name either way.
441        assert_eq!(ext_of("v1.2"), ("", "v1.2"));
442        assert_eq!(ext_of("sub\\nested"), ("", "nested"));
443    }
444}
