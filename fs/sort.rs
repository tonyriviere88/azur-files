1//! Ordering a listing.
2//!
3//! Two things make this fast enough to re-run on every column click, even on a
4//! folder of a few hundred thousand entries:
5//!
6//! - **Indices are sorted, not entries.** The order is a `Vec<u32>`, so a swap
7//!   moves four bytes instead of thirty-two, and the `Dir` itself stays immutable
8//!   and shareable between tabs.
9//! - **Keys are already integers.** Size and date sort on the `u64` they were
10//!   enumerated as. Only the name comparison touches text, and only as a
11//!   tie-break for the other three.
12
13use std::cmp::Ordering;
14
15use super::dir::Dir;
16
17/// The four columns of the details view.
18///
19/// The discriminants are spelled out because the view indexes its width array by
20/// them, and a column reordered here would otherwise silently resize the wrong one.
21#[derive(Clone, Copy, PartialEq, Eq, Debug)]
22#[repr(usize)]
23pub enum Column {
24    Name = 0,
25    Size = 1,
26    Type = 2,
27    Modified = 3,
28}
29
30impl Column {
31    pub const ALL: [Column; 4] = [Self::Name, Self::Size, Self::Type, Self::Modified];
32
33    /// Index into a per-column array.
34    #[inline]
35    pub fn index(self) -> usize {
36        self as usize
37    }
38
39    pub fn header(self) -> &'static str {
40        match self {
41            Self::Name => "Name",
42            Self::Size => "Size",
43            Self::Type => "Type",
44            Self::Modified => "Modified",
45        }
46    }
47
48    /// Right-aligned, because the digits should line up.
49    pub fn numeric(self) -> bool {
50        matches!(self, Self::Size)
51    }
52
53    /// Which way a fresh click on this header should sort.
54    ///
55    /// Names want A→Z, but a click on Size or Modified almost always means "what
56    /// is biggest" or "what did I just touch", so those start descending — which
57    /// is Explorer's behaviour too.
58    pub fn starts_ascending(self) -> bool {
59        matches!(self, Self::Name | Self::Type)
60    }
61}
62
63/// Build the display order for a directory.
64///
65/// `order` is reused between calls so a re-sort allocates nothing.
66pub fn build_order(
67    dir: &Dir,
68    order: &mut Vec<u32>,
69    column: Column,
70    ascending: bool,
71    show_hidden: bool,
72    filter: &str,
73) {
74    order.clear();
75    order.reserve(dir.len());
76
77    let needle = Needle::new(filter);
78    let filtering = !needle.is_empty();
79    for i in 0..dir.len() {
80        let entry = &dir.entries[i];
81        if !show_hidden && entry.is_hidden() {
82            continue;
83        }
84        if filtering && !needle.matches(dir.name(i)) {
85            continue;
86        }
87        order.push(i as u32);
88    }
89
90    sort_order(dir, order, column, ascending);
91}
92
93/// Sort an existing order in place, leaving the filter alone.
94pub fn sort_order(dir: &Dir, order: &mut [u32], column: Column, ascending: bool) {
95    // Type sorts on the label the column shows, which is a lookup per row rather than a
96    // field — so the lookups happen once, up front, and become an integer per entry.
97    // Empty for every other column, and never indexed by one. See [`type_ranks`].
98    let ranks = if column == Column::Type {
99        type_ranks(dir, order)
100    } else {
101        Vec::new()
102    };
103
104    // Directories first regardless of column or direction: a folder is a place and
105    // a file is a thing, and mixing them by size makes a listing you have to read
106    // twice. Explorer, File Pilot and every other shell do the same.
107    //
108    // `sort_unstable_by` because the comparator is a total order down to the name,
109    // so stability would only cost time.
110    order.sort_unstable_by(|&a, &b| {
111        let (ea, eb) = (&dir.entries[a as usize], &dir.entries[b as usize]);
112        match eb.is_dir().cmp(&ea.is_dir()) {
113            Ordering::Equal => {}
114            folders_first => return folders_first,
115        }
116
117        let primary = match column {
118            Column::Name => Ordering::Equal,
119            // A directory has no meaningful size, so within the folder block the
120            // Size column may as well fall through to the name.
121            Column::Size if ea.is_dir() => Ordering::Equal,
122            Column::Size => ea.size.cmp(&eb.size),
123            Column::Type => ranks[a as usize].cmp(&ranks[b as usize]),
124            Column::Modified => ea.modified.cmp(&eb.modified),
125        };
126        let primary = if ascending { primary } else { primary.reverse() };
127
128        match primary {
129            Ordering::Equal => {
130                let names = natural_cmp(dir.name(a as usize), dir.name(b as usize));
131                // The tie-break follows the same direction, so reversing the sort
132                // reverses the whole listing rather than scrambling each group.
133                if ascending {
134                    names
135                } else {
136                    names.reverse()
137                }
138            }
139            other => other,
140        }
141    });
142}
143
144/// A rank per entry for the Type column, so that ordering by rank orders by the label
145/// the column actually shows.
146///
147/// Sorting on the extension instead — which is what this did first — produces an order
148/// nobody can verify by looking at it: `.cpp` before `.exe` puts "C++ source" *above*
149/// "Application", and a folder where one extension dominates (a source tree, a photo
150/// directory) comes out looking exactly like a sort by name, because every row ties and
151/// the tie-break is the name. That is the bug this fixes.
152///
153/// Comparing the labels directly would mean resolving one per comparison — `n log n`
154/// lookups. So each *distinct* extension is resolved once, the labels are ranked among
155/// themselves, and the comparator is left comparing two `u32`s. A folder of 60,000 files
156/// has perhaps thirty distinct types, so the added work is thirty lookups and one sort
157/// of thirty strings.
158///
159/// The ranking is *dense* — equal labels get the same rank — which is what puts `.jpg`
160/// beside `.jpeg`: both say "JPEG image", so both rank equal and the name tie-break
161/// interleaves them into one group instead of two adjacent ones.
162fn type_ranks(dir: &Dir, order: &[u32]) -> Vec<u32> {
163    let mut ranks = vec![0u32; dir.len()];
164    // Extension -> which label it resolved to. Keyed by the extension exactly as
165    // stored, so `.TXT` and `.txt` are two keys — they then resolve to the same label
166    // and the dense ranking gives them the same rank anyway.
167    let mut slot_of: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
168    let mut labels: Vec<String> = Vec::new();
169    let mut label = String::new();
170
171    for &index in order {
172        let i = index as usize;
173        // Folders sort ahead of every file whatever the column, so their rank is never
174        // compared against anything and does not need resolving.
175        if dir.entries[i].is_dir() {
176            continue;
177        }
178        let ext = dir.ext(i);
179        ranks[i] = match slot_of.get(ext) {
180            Some(&slot) => slot,
181            None => {
182                label.clear();
183                super::fmt::type_label(ext, false, &mut label);
184                labels.push(label.clone());
185                let slot = (labels.len() - 1) as u32;
186                slot_of.insert(ext, slot);
187                slot
188            }
189        };
190    }
191
192    // Rank the distinct labels among themselves, then rewrite each entry's slot as its
193    // label's rank.
194    let mut by_label: Vec<u32> = (0..labels.len() as u32).collect();
195    by_label.sort_unstable_by(|&a, &b| natural_cmp(&labels[a as usize], &labels[b as usize]));
196    let mut rank_of = vec![0u32; labels.len()];
197    let mut rank = 0u32;
198    for (position, &slot) in by_label.iter().enumerate() {
199        if position > 0 && labels[slot as usize] != labels[by_label[position - 1] as usize] {
200            rank += 1;
201        }
202        rank_of[slot as usize] = rank;
203    }
204    for &index in order {
205        let i = index as usize;
206        if !dir.entries[i].is_dir() {
207            ranks[i] = rank_of[ranks[i] as usize];
208        }
209    }
210    ranks
211}
212
213// ---------------------------------------------------------------------------
214// Natural ordering
215// ---------------------------------------------------------------------------
216
217/// Compare two names the way a person reads them: case-insensitively, and with
218/// runs of digits compared as numbers so `file2` comes before `file10`.
219///
220/// Bytes rather than chars, with an ASCII fast path. Beyond ASCII this falls back
221/// to comparing UTF-8 bytes, which is code-point order — so accented names sort
222/// consistently but not by the locale's collation rules. Doing that properly needs
223/// ICU, and the sort would no longer be free.
224pub fn natural_cmp(a: &str, b: &str) -> Ordering {
225    let (a, b) = (a.as_bytes(), b.as_bytes());
226    let (mut i, mut j) = (0, 0);
227
228    while i < a.len() && j < b.len() {
229        let (ca, cb) = (a[i], b[j]);
230
231        if ca.is_ascii_digit() && cb.is_ascii_digit() {
232            // Skip leading zeros so `007` and `7` compare equal in value, and let
233            // the shorter spelling win only if everything else ties.
234            let (za, ia) = skip_zeros(a, i);
235            let (zb, jb) = skip_zeros(b, j);
236            let (ea, eb) = (digits_end(a, ia), digits_end(b, jb));
237            let (la, lb) = (ea - ia, eb - jb);
238
239            // More digits (after zeros) is a bigger number.
240            match la.cmp(&lb) {
241                Ordering::Equal => match a[ia..ea].cmp(&b[jb..eb]) {
242                    Ordering::Equal => {}
243                    unequal => return unequal,
244                },
245                unequal => return unequal,
246            }
247            // Equal in value: remember the zero padding as a last resort, so
248            // `01` and `1` still have a stable order.
249            if za != zb {
250                return za.cmp(&zb);
251            }
252            i = ea;
253            j = eb;
254            continue;
255        }
256
257        let (la, lb) = (lower(ca), lower(cb));
258        if la != lb {
259            return la.cmp(&lb);
260        }
261        i += 1;
262        j += 1;
263    }
264
265    // One is a prefix of the other, or they differ only in case.
266    match (a.len() - i).cmp(&(b.len() - j)) {
267        Ordering::Equal => a.cmp(b),
268        unequal => unequal,
269    }
270}
271
272#[inline]
273fn lower(byte: u8) -> u8 {
274    // Only ASCII: a byte in a UTF-8 continuation sequence is >= 0x80 and must be
275    // left alone, or two different characters could fold onto each other.
276    if byte.is_ascii_uppercase() {
277        byte + 32
278    } else {
279        byte
280    }
281}
282
283#[inline]
284fn skip_zeros(s: &[u8], mut i: usize) -> (usize, usize) {
285    let start = i;
286    while i + 1 < s.len() && s[i] == b'0' && s[i + 1].is_ascii_digit() {
287        i += 1;
288    }
289    (i - start, i)
290}
291
292#[inline]
293fn digits_end(s: &[u8], mut i: usize) -> usize {
294    while i < s.len() && s[i].is_ascii_digit() {
295        i += 1;
296    }
297    i
298}
299
300// ---------------------------------------------------------------------------
301// Filtering
302// ---------------------------------------------------------------------------
303
304/// A case-insensitive substring test, pre-lowered once instead of per candidate.
305///
306/// An empty needle matches everything and short-circuits, so an unfiltered
307/// listing pays nothing for the feature being there.
308pub struct Needle {
309    lowered: Vec<u8>,
310}
311
312impl Needle {
313    pub fn new(filter: &str) -> Self {
314        Self {
315            lowered: filter.trim().bytes().map(lower).collect(),
316        }
317    }
318
319    pub fn is_empty(&self) -> bool {
320        self.lowered.is_empty()
321    }
322
323    pub fn matches(&self, name: &str) -> bool {
324        if self.lowered.is_empty() {
325            return true;
326        }
327        let name = name.as_bytes();
328        if name.len() < self.lowered.len() {
329            return false;
330        }
331        let first = self.lowered[0];
332        // Naive search, but over a name of at most 255 bytes with a one-byte
333        // prefix check — the memchr-shaped win is not worth a dependency here.
334        (0..=name.len() - self.lowered.len()).any(|start| {
335            lower(name[start]) == first
336                && name[start..start + self.lowered.len()]
337                    .iter()
338                    .zip(&self.lowered)
339                    .all(|(a, b)| lower(*a) == *b)
340        })
341    }
342}
343
344#[cfg(test)]
345mod tests {
346    use super::*;
347    use crate::fs::dir::{DirBuilder, FLAG_DIR};
348
349    /// What one keystroke in the filter box costs on a listing big enough to notice.
350    ///
351    /// ```text
352    /// cargo test --release -- --ignored --nocapture filter_speed
353    /// ```
354    ///
355    /// This is the measurement behind [`crate::pane::FILTER_DELAY`]: a filter is re-applied from
356    /// scratch on every change, so if one pass is slow then *typing* is slow, and no amount of
357    /// making the pass faster fixes a listing where every keystroke costs a pass. The tree is
358    /// whatever `YAFE_FLATTEN_ROOT` names, flattened — which is the biggest listing this program
359    /// can produce and the case the delay exists for.
360    #[test]
361    #[ignore = "walks a large tree; run explicitly"]
362    fn filter_speed() {
363        let root = std::env::var("YAFE_FLATTEN_ROOT")
364            .map(std::path::PathBuf::from)
365            .unwrap_or_else(|_| {
366                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target")
367            });
368        if !root.is_dir() {
369            println!("no {}; skipping", root.display());
370            return;
371        }
372        let dir = crate::fs::scan::scan_deep(
373            &root,
374            crate::fs::scan::FLATTEN_BUDGET,
375            std::time::Duration::from_secs(600),
376        );
377        println!("{} entries from {}", dir.len(), root.display());
378
379        let mut order = Vec::new();
380        for filter in ["", "e", "ex", "exe", "microsoft"] {
381            // Twice, and the second is the one to read: the first pass warms the caches the
382            // listing's own arena needs, which a real second keystroke would find warm too.
383            let mut took = std::time::Duration::ZERO;
384            for _ in 0..2 {
385                let started = std::time::Instant::now();
386                build_order(&dir, &mut order, Column::Type, true, false, filter);
387                took = started.elapsed();
388            }
389            println!(
390                "  filter {:>12}: {:>7} rows in {:>7.2} ms",
391                format!("{filter:?}"),
392                order.len(),
393                took.as_secs_f64() * 1000.0
394            );
395        }
396    }
397
398    fn sorted(mut names: Vec<&str>) -> Vec<&str> {
399        names.sort_by(|a, b| natural_cmp(a, b));
400        names
401    }
402
403    /// A listing of the given names, folders marked by a trailing `/`.
404    fn listing(names: &[&str]) -> Dir {
405        let mut builder = DirBuilder::new(r"C:\x");
406        for name in names {
407            match name.strip_suffix('/') {
408                Some(folder) => builder.push(folder, 0, 0, FLAG_DIR),
409                None => builder.push(name, 1, 0, 0),
410            }
411        }
412        builder.finish(0)
413    }
414
415    fn by(dir: &Dir, column: Column, ascending: bool) -> Vec<String> {
416        let mut order = Vec::new();
417        build_order(dir, &mut order, column, ascending, true, "");
418        order
419            .iter()
420            .map(|&i| dir.name(i as usize).to_owned())
421            .collect()
422    }
423
424    #[test]
425    fn type_sorts_by_the_label_the_column_shows() {
426        // Not by extension: `cpp` < `exe`, but "Application" < "C++ source". Sorting on
427        // the extension put the column visibly out of order, which is what this is for.
428        let dir = listing(&["b.cpp", "a.exe"]);
429        assert_eq!(by(&dir, Column::Type, true), ["a.exe", "b.cpp"]);
430        assert_eq!(by(&dir, Column::Type, false), ["b.cpp", "a.exe"]);
431    }
432
433    #[test]
434    fn two_extensions_with_one_label_form_one_group() {
435        // Both are "JPEG image", so they rank equal and the name tie-break interleaves
436        // them — one group in the column rather than two that happen to be adjacent.
437        let dir = listing(&["z.jpg", "a.jpeg", "m.jpg"]);
438        assert_eq!(by(&dir, Column::Type, true), ["a.jpeg", "m.jpg", "z.jpg"]);
439    }
440
441    #[test]
442    fn a_type_sort_still_orders_by_name_inside_a_type() {
443        let dir = listing(&["b.txt", "a.txt", "c.exe"]);
444        assert_eq!(by(&dir, Column::Type, true), ["c.exe", "a.txt", "b.txt"]);
445    }
446
447    #[test]
448    fn folders_lead_every_column_and_ignore_its_key() {
449        // A folder has no type of its own, and none of the labels may reorder it out of
450        // the block at the top.
451        let dir = listing(&["zebra/", "a.exe", "alpha/"]);
452        assert_eq!(
453            by(&dir, Column::Type, true),
454            ["alpha", "zebra", "a.exe"],
455            "folders first, in name order"
456        );
457        assert_eq!(
458            by(&dir, Column::Type, false)[0],
459            "zebra",
460            "still folders first when reversed"
461        );
462    }
463
464    #[test]
465    fn digits_compare_as_numbers() {
466        assert_eq!(
467            sorted(vec!["file10.txt", "file2.txt", "file1.txt"]),
468            vec!["file1.txt", "file2.txt", "file10.txt"]
469        );
470        assert_eq!(
471            sorted(vec!["scan_100", "scan_9", "scan_20"]),
472            vec!["scan_9", "scan_20", "scan_100"]
473        );
474    }
475
476    #[test]
477    fn case_is_ignored_but_still_breaks_ties() {
478        assert_eq!(natural_cmp("Alpha", "alpha"), natural_cmp("Alpha", "alpha"));
479        assert_eq!(natural_cmp("beta", "Alpha"), Ordering::Greater);
480        // A pure case difference has to be *some* consistent order, not Equal, or
481        // an unstable sort could swap two rows between frames.
482        assert_ne!(natural_cmp("Alpha", "alpha"), Ordering::Equal);
483    }
484
485    #[test]
486    fn leading_zeros_do_not_change_the_value() {
487        assert_eq!(
488            sorted(vec!["v007", "v10", "v7", "v0008"]),
489            vec!["v7", "v007", "v0008", "v10"]
490        );
491    }
492
493    #[test]
494    fn prefixes_come_first() {
495        assert_eq!(
496            sorted(vec!["report.txt", "report", "reports"]),
497            vec!["report", "report.txt", "reports"]
498        );
499    }
500
501    #[test]
502    fn ordering_is_a_total_order() {
503        // An unstable sort with an inconsistent comparator can loop or panic, so
504        // check antisymmetry across a set built to collide.
505        let names = [
506            "a", "A", "a1", "a01", "a2", "a10", "b", "", "1", "01", "2", "z9z", "z10z",
507        ];
508        for x in names {
509            for y in names {
510                assert_eq!(
511                    natural_cmp(x, y).reverse(),
512                    natural_cmp(y, x),
513                    "`{x}` vs `{y}`"
514                );
515            }
516        }
517    }
518
519    #[test]
520    fn filter_is_case_insensitive_substring() {
521        let needle = Needle::new("  ReadMe ");
522        assert!(needle.matches("README.md"));
523        assert!(needle.matches("some-readme-file"));
524        assert!(!needle.matches("read.me"));
525
526        assert!(Needle::new("").matches("anything"));
527        assert!(Needle::new("   ").is_empty());
528    }
529}
