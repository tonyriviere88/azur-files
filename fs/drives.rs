1//! Volumes: what is mounted, how full it is, and the synthetic "This PC" listing.
2//!
3//! # The 22-second trap
4//!
5//! Listing drive letters is free. *Describing* one is not, and the difference is
6//! not small: on this machine, the first `GetVolumeInformationW` against a mapped
7//! network drive that is not currently reachable takes **22 seconds** while SMB
8//! tries to reconnect underneath it. A file manager that asks for volume labels on
9//! its startup path is a file manager whose window does not appear for 22 seconds.
10//!
11//! So the two halves are separate:
12//!
13//! - [`list_letters`] is `GetLogicalDrives` + `GetDriveTypeW`. Both answer from the
14//!   local mount table with no I/O, in microseconds. This is what the sidebar shows
15//!   immediately.
16//! - [`describe`] is the part that can block — the label and the free space. It runs
17//!   on a worker (see [`crate::loader::Volumes`]), one thread per volume so a
18//!   stalled share delays only its own row, and the answer is merged in when it
19//!   arrives.
20//!
21//! Every thread that calls [`describe`] must have called
22//! [`super::scan::silence_device_dialogs`] first, or an empty card reader raises
23//! "Please insert a disk into drive E:" from inside the syscall.
24
25use std::path::PathBuf;
26use std::sync::Mutex;
27use std::time::Instant;
28
29use super::dir::{Dir, DirBuilder, FLAG_DIR};
30
31/// What kind of thing a volume is, which decides its icon.
32#[derive(Clone, Copy, PartialEq, Eq, Debug)]
33pub enum DriveKind {
34    Fixed,
35    Removable,
36    Optical,
37    Network,
38    RamDisk,
39    Unknown,
40}
41
42impl DriveKind {
43    /// What Explorer calls a volume with no label of its own.
44    pub fn default_label(self) -> &'static str {
45        match self {
46            Self::Fixed => "Local Disk",
47            Self::Removable => "Removable Disk",
48            Self::Optical => "DVD Drive",
49            Self::Network => "Network Drive",
50            Self::RamDisk => "RAM Disk",
51            Self::Unknown => "Disk",
52        }
53    }
54}
55
56/// One mounted volume.
57#[derive(Clone, Debug)]
58pub struct Drive {
59    /// `C:\` — where clicking it navigates.
60    pub path: PathBuf,
61    /// `C:` — the row's identity, and the key descriptions are merged on.
62    pub letter: String,
63    /// The volume label, or the drive kind's default name until one is known.
64    pub label: String,
65    pub kind: DriveKind,
66    /// Capacity and free space in bytes. Both zero until described.
67    pub total: u64,
68    pub free: u64,
69    /// Whether [`describe`] has run against this volume yet.
70    pub described: bool,
71}
72
73impl Drive {
74    /// How full, in `0..=1`. `None` when the volume has no measurement — either
75    /// because nothing is in it, or because it has not been described yet.
76    pub fn used_fraction(&self) -> Option<f32> {
77        (self.total > 0).then(|| {
78            let used = self.total.saturating_sub(self.free);
79            (used as f64 / self.total as f64) as f32
80        })
81    }
82}
83
84/// Every mounted volume, by letter and kind only. Microseconds, no I/O.
85#[cfg(windows)]
86pub fn list_letters() -> Vec<Drive> {
87    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
88
89    // The DRIVE_* constants live behind a `windows-sys` feature that carries nothing
90    // else this program wants, and they are stable numbers.
91    const DRIVE_REMOVABLE: u32 = 2;
92    const DRIVE_FIXED: u32 = 3;
93    const DRIVE_REMOTE: u32 = 4;
94    const DRIVE_CDROM: u32 = 5;
95    const DRIVE_RAMDISK: u32 = 6;
96
97    let mask = unsafe { GetLogicalDrives() };
98    let mut drives = Vec::new();
99
100    for bit in 0..26u32 {
101        if mask & (1 << bit) == 0 {
102            continue;
103        }
104        let letter = (b'A' + bit as u8) as char;
105        let root = root_of(letter);
106        let kind = match unsafe { GetDriveTypeW(root.as_ptr()) } {
107            DRIVE_FIXED => DriveKind::Fixed,
108            DRIVE_REMOVABLE => DriveKind::Removable,
109            DRIVE_CDROM => DriveKind::Optical,
110            DRIVE_REMOTE => DriveKind::Network,
111            DRIVE_RAMDISK => DriveKind::RamDisk,
112            _ => DriveKind::Unknown,
113        };
114        drives.push(Drive {
115            path: PathBuf::from(format!("{letter}:\\")),
116            letter: format!("{letter}:"),
117            label: kind.default_label().to_owned(),
118            kind,
119            total: 0,
120            free: 0,
121            described: false,
122        });
123    }
124    drives
125}
126
127/// Fill in a volume's label and free space.
128///
129/// **May block for tens of seconds.** Never call this on the UI thread.
130#[cfg(windows)]
131pub fn describe(drive: &mut Drive) {
132    use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetVolumeInformationW};
133
134    let letter = drive.letter.chars().next().unwrap_or('C');
135    let root = root_of(letter);
136
137    let mut label_buf = [0u16; 128];
138    let ok = unsafe {
139        GetVolumeInformationW(
140            root.as_ptr(),
141            label_buf.as_mut_ptr(),
142            label_buf.len() as u32,
143            std::ptr::null_mut(),
144            std::ptr::null_mut(),
145            std::ptr::null_mut(),
146            std::ptr::null_mut(),
147            0,
148        )
149    } != 0;
150
151    if ok {
152        let text = wide_to_string(&label_buf);
153        if !text.is_empty() {
154            drive.label = text;
155        }
156        let (mut total, mut free, mut available) = (0u64, 0u64, 0u64);
157        let measured =
158            unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut available, &mut total, &mut free) }
159                != 0;
160        if measured {
161            drive.total = total;
162            // The quota-aware figure is the one that matters to whoever is looking,
163            // and it is never larger than the volume's own free count.
164            drive.free = free.min(available);
165        }
166    }
167    // Failure leaves the kind's default label and no bar, which is the honest
168    // rendering of "there is a slot here and nothing in it".
169    drive.described = true;
170    remember(drive.clone());
171}
172
173/// `C:\` as a null-terminated wide string, which every volume call wants.
174#[cfg(windows)]
175fn root_of(letter: char) -> [u16; 4] {
176    [letter as u16, b':' as u16, b'\\' as u16, 0]
177}
178
179/// Mount points, for the platforms that have no drive letters.
180#[cfg(not(windows))]
181pub fn list_letters() -> Vec<Drive> {
182    vec![Drive {
183        path: PathBuf::from("/"),
184        letter: "/".to_owned(),
185        label: "Filesystem".to_owned(),
186        kind: DriveKind::Fixed,
187        total: 0,
188        free: 0,
189        described: false,
190    }]
191}
192
193#[cfg(not(windows))]
194pub fn describe(drive: &mut Drive) {
195    drive.described = true;
196    remember(drive.clone());
197}
198
199// ---------------------------------------------------------------------------
200// What has been learned so far
201// ---------------------------------------------------------------------------
202
203/// Descriptions already paid for, keyed by letter.
204///
205/// A process-wide cache rather than state threaded through the application, because
206/// the set of volumes on the machine *is* process-wide — and because [`this_pc`] runs
207/// on a scanner worker that has no way to reach the sidebar's copy. Nothing here
208/// blocks: it is only ever read, or written by a probe that has already paid the
209/// cost.
210static KNOWN: Mutex<Vec<Drive>> = Mutex::new(Vec::new());
211
212/// Record what a probe found.
213pub fn remember(drive: Drive) {
214    let Ok(mut known) = KNOWN.lock() else { return };
215    match known.iter_mut().find(|d| d.letter == drive.letter) {
216        Some(existing) => *existing = drive,
217        None => known.push(drive),
218    }
219}
220
221/// The described volumes, if any probe has finished.
222pub fn known() -> Vec<Drive> {
223    KNOWN.lock().map(|k| k.clone()).unwrap_or_default()
224}
225
226/// Drop everything learned, so a refresh actually re-reads.
227pub fn forget_all() {
228    if let Ok(mut known) = KNOWN.lock() {
229        known.clear();
230    }
231}
232
233/// The drive list as a [`Dir`], so "This PC" is a listing like any other and the
234/// details view needs no special case for it.
235///
236/// Letters come from the mount table and labels from whatever has already been
237/// described, so this never blocks — a volume nobody has probed yet appears under
238/// its kind's name and gains its real one once the probe lands.
239///
240/// The rows carry explicit targets, because a drive's display name (`Windows (C:)`)
241/// is not a child of the empty path the way a file name is a child of its folder.
242pub fn this_pc(started: Instant) -> Dir {
243    let described = known();
244    let drives: Vec<Drive> = list_letters()
245        .into_iter()
246        .map(|drive| {
247            described
248                .iter()
249                .find(|d| d.letter == drive.letter)
250                .cloned()
251                .unwrap_or(drive)
252        })
253        .collect();
254
255    let mut builder = DirBuilder::new(PathBuf::new());
256    builder.reserve(drives.len());
257    for drive in &drives {
258        builder.push_link(
259            &format!("{} ({})", drive.label, drive.letter),
260            drive.path.clone(),
261            drive.total,
262            FLAG_DIR,
263        );
264    }
265    builder.finish(started.elapsed().as_micros() as u64)
266}
267
268/// A null-terminated wide buffer as a `String`.
269#[cfg(windows)]
270fn wide_to_string(buf: &[u16]) -> String {
271    let len = buf.iter().position(|&u| u == 0).unwrap_or(buf.len());
272    String::from_utf16_lossy(&buf[..len])
273}
274
275#[cfg(test)]
276mod tests {
277    use super::*;
278
279    /// The whole point of the split: this is on the startup path and must not do
280    /// I/O. A generous bound, since a loaded machine is still nowhere near a network
281    /// timeout.
282    #[test]
283    fn listing_letters_does_no_io() {
284        let started = Instant::now();
285        let drives = list_letters();
286        let elapsed = started.elapsed();
287        assert!(
288            elapsed < std::time::Duration::from_millis(150),
289            "list_letters took {elapsed:?} -- it is on the startup path and a \
290             blocking volume query has crept back into it"
291        );
292        for drive in &drives {
293            assert!(!drive.described);
294            assert_eq!(drive.total, 0, "an undescribed volume has no measurement");
295        }
296    }
297
298    #[test]
299    fn this_pc_never_blocks_either() {
300        let started = Instant::now();
301        let dir = this_pc(Instant::now());
302        assert!(
303            started.elapsed() < std::time::Duration::from_millis(150),
304            "This PC is a listing like any other and cannot wait on a share"
305        );
306        assert_eq!(dir.len(), list_letters().len());
307        for i in 0..dir.len() {
308            assert!(dir.entries[i].is_dir());
309            assert!(
310                !dir.target(i).as_os_str().is_empty(),
311                "every drive row has to lead somewhere"
312            );
313        }
314    }
315
316    #[test]
317    fn descriptions_merge_by_letter() {
318        let sample = |label: &str, total, free| Drive {
319            path: PathBuf::from("Z:\\"),
320            letter: "Z:".to_owned(),
321            label: label.to_owned(),
322            kind: DriveKind::Fixed,
323            total,
324            free,
325            described: true,
326        };
327        remember(sample("First", 100, 40));
328        remember(sample("Second", 200, 50));
329
330        let known = known();
331        let z: Vec<&Drive> = known.iter().filter(|d| d.letter == "Z:").collect();
332        assert_eq!(z.len(), 1, "a letter is described once, not appended to");
333        assert_eq!(z[0].label, "Second");
334        assert_eq!(z[0].used_fraction(), Some(0.75));
335    }
336
337    #[test]
338    fn an_undescribed_volume_has_no_gauge() {
339        let drives = list_letters();
340        if let Some(drive) = drives.first() {
341            assert_eq!(
342                drive.used_fraction(),
343                None,
344                "a bar drawn before the measurement lands would be a made-up number"
345            );
346        }
347    }
348}
