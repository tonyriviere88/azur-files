1//! `FILETIME` to a local date, without a syscall per row.
2//!
3//! The obvious way to fill the Modified column is `FileTimeToSystemTime` +
4//! `SystemTimeToTzSpecificLocalTime`, which is two kernel transitions per cell.
5//! That is fine for the forty rows on screen and ruinous for the sort, the
6//! column-width measurement, or any future grouping pass — so the timezone rules
7//! are read **once** into [`LocalZone`] and every conversion after that is
8//! integer arithmetic.
9//!
10//! The civil-calendar maths is Howard Hinnant's `days_from_civil` /
11//! `civil_from_days`, which is exact for any year and branch-free apart from the
12//! leap-year test.
13//!
14//! # The one inaccuracy
15//!
16//! Windows keeps a *dynamic* timezone database: one set of daylight-saving rules
17//! per year, so a timestamp from before a country last moved its transition dates
18//! converts correctly. [`GetTimeZoneInformation`] hands over only the rules in
19//! force now, and this applies them to every year. A file last written under an
20//! older rule can therefore read an hour out. Explorer's own tooltip has the same
21//! quirk, and the alternative is a syscall per cell.
22
23/// 1601-01-01 to 1970-01-01, in `FILETIME` ticks.
24///
25/// Only the portable scanner needs it — the Windows one is handed `FILETIME`s
26/// already — but it is the number this whole module is built around, so it is
27/// stated here rather than buried in a `cfg` block.
28#[allow(dead_code)]
29pub const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;
30
31/// Seconds between the `FILETIME` epoch and the Unix epoch.
32const FILETIME_TO_UNIX_SECS: i64 = 11_644_473_600;
33
34/// A broken-down local date and time. No timezone: it is already local.
35#[derive(Clone, Copy, PartialEq, Eq, Debug)]
36pub struct DateTime {
37    pub year: i32,
38    pub month: u32,
39    pub day: u32,
40    pub hour: u32,
41    pub minute: u32,
42    pub second: u32,
43}
44
45/// One daylight-saving transition, as Windows states it: "the `week`th `dow` of
46/// `month`, at `hour:minute`", where week 5 means the last one.
47#[derive(Clone, Copy, Debug)]
48struct Transition {
49    month: u32,
50    /// 0 = Sunday, as `SYSTEMTIME::wDayOfWeek` counts.
51    dow: u32,
52    /// 1..=5, where 5 is "the last one in the month".
53    week: u32,
54    hour: u32,
55    minute: u32,
56}
57
58/// The machine's timezone, resolved once.
59#[derive(Clone, Debug)]
60pub struct LocalZone {
61    /// Minutes to add to UTC outside daylight saving.
62    std_offset: i32,
63    /// Minutes to add to UTC during daylight saving.
64    dst_offset: i32,
65    /// When daylight saving starts and ends, if this zone has any.
66    dst: Option<(Transition, Transition)>,
67}
68
69impl LocalZone {
70    /// UTC — the fallback when the platform will not say, and what the non-Windows
71    /// build uses.
72    pub const fn utc() -> Self {
73        Self {
74            std_offset: 0,
75            dst_offset: 0,
76            dst: None,
77        }
78    }
79
80    /// Read the current zone from the operating system.
81    #[cfg(windows)]
82    pub fn current() -> Self {
83        use windows_sys::Win32::System::Time::{
84            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
85        };
86
87        let mut info = TIME_ZONE_INFORMATION::default();
88        if unsafe { GetTimeZoneInformation(&mut info) } == TIME_ZONE_ID_INVALID {
89            return Self::utc();
90        }
91
92        // Windows states the bias as "UTC = local + bias", so the offset that
93        // takes UTC to local is its negation.
94        let std_offset = -(info.Bias + info.StandardBias);
95        let dst_offset = -(info.Bias + info.DaylightBias);
96
97        // `wMonth == 0` is how Windows says "this zone does not observe daylight
98        // saving". A non-zero `wYear` means an absolute, one-off date rather than
99        // a recurring rule; nothing in the wild ships that through this API, and
100        // treating it as "no DST" is safer than misreading it as a rule.
101        let recurring = info.DaylightDate.wMonth != 0
102            && info.StandardDate.wMonth != 0
103            && info.DaylightDate.wYear == 0
104            && info.StandardDate.wYear == 0;
105
106        let dst = recurring.then(|| {
107            (
108                Transition::from(&info.DaylightDate),
109                Transition::from(&info.StandardDate),
110            )
111        });
112
113        Self {
114            std_offset,
115            dst_offset,
116            dst,
117        }
118    }
119
120    #[cfg(not(windows))]
121    pub fn current() -> Self {
122        // A portable local-time lookup means parsing the TZif database, which is
123        // a dependency this program does not otherwise need. The Windows build is
124        // the one that has to be right.
125        Self::utc()
126    }
127
128    /// Convert a raw `FILETIME` to local civil time.
129    ///
130    /// `None` for the zero timestamp, which is what a filesystem reports when it
131    /// has no idea — and which would otherwise print as 1601.
132    pub fn convert(&self, filetime: u64) -> Option<DateTime> {
133        if filetime == 0 {
134            return None;
135        }
136        let utc = (filetime / 10_000_000) as i64 - FILETIME_TO_UNIX_SECS;
137        let offset = self.offset_at(utc);
138        Some(civil(utc + offset as i64 * 60))
139    }
140
141    /// Which of the two offsets is in force at a UTC instant.
142    fn offset_at(&self, utc_secs: i64) -> i32 {
143        let Some((start, end)) = self.dst else {
144            return self.std_offset;
145        };
146        // Both transitions are stated in local time. Resolving them needs a year,
147        // and the year needs a conversion — so this uses standard local time to
148        // pick the year and to compare, which is exact except within the hour of
149        // a transition itself.
150        let local = utc_secs + self.std_offset as i64 * 60;
151        let year = civil(local).year;
152        let dst_starts = start.instant(year);
153        let dst_ends = end.instant(year);
154
155        let in_daylight = if dst_starts <= dst_ends {
156            // Northern hemisphere: the daylight window sits inside the year.
157            local >= dst_starts && local < dst_ends
158        } else {
159            // Southern: it straddles New Year.
160            local >= dst_starts || local < dst_ends
161        };
162        if in_daylight {
163            self.dst_offset
164        } else {
165            self.std_offset
166        }
167    }
168}
169
170#[cfg(windows)]
171impl From<&windows_sys::Win32::Foundation::SYSTEMTIME> for Transition {
172    fn from(st: &windows_sys::Win32::Foundation::SYSTEMTIME) -> Self {
173        Self {
174            month: st.wMonth as u32,
175            dow: st.wDayOfWeek as u32,
176            week: st.wDay as u32,
177            hour: st.wHour as u32,
178            minute: st.wMinute as u32,
179        }
180    }
181}
182
183impl Transition {
184    /// The instant this rule fires in `year`, as local seconds since the Unix
185    /// epoch.
186    fn instant(&self, year: i32) -> i64 {
187        let day = self.day_of_month(year);
188        days_from_civil(year, self.month, day) * 86_400
189            + self.hour as i64 * 3600
190            + self.minute as i64 * 60
191    }
192
193    /// Resolve "the `week`th `dow` of `month`" to a day of the month.
194    fn day_of_month(&self, year: i32) -> u32 {
195        let first = weekday(days_from_civil(year, self.month, 1));
196        // The first `dow` on or after the 1st.
197        let first_match = 1 + (self.dow + 7 - first) % 7;
198        let mut day = first_match + (self.week.saturating_sub(1)) * 7;
199        // Week 5 means "the last one", and a fifth occurrence often does not
200        // exist — step back until the day is real.
201        let last = days_in_month(year, self.month);
202        while day > last {
203            day -= 7;
204        }
205        day
206    }
207}
208
209/// Days since 1970-01-01 for a civil date. Hinnant's algorithm: shifts the year
210/// to start in March so the leap day lands at the end and needs no special case.
211fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
212    let y = if month <= 2 { year - 1 } else { year } as i64;
213    let era = if y >= 0 { y } else { y - 399 } / 400;
214    let yoe = y - era * 400; // 0..=399
215    let m = month as i64;
216    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1; // 0..=365
217    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // 0..=146096
218    era * 146_097 + doe - 719_468
219}
220
221/// The inverse, plus the time of day, from seconds since the Unix epoch.
222fn civil(unix_secs: i64) -> DateTime {
223    // Floor division, so dates before 1970 do not round towards zero and land a
224    // day out.
225    let days = unix_secs.div_euclid(86_400);
226    let secs = unix_secs.rem_euclid(86_400);
227
228    let z = days + 719_468;
229    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
230    let doe = z - era * 146_097; // 0..=146096
231    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // 0..=399
232    let y = yoe + era * 400;
233    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..=365
234    let mp = (5 * doy + 2) / 153; // 0..=11, March-based
235    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
236    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
237
238    DateTime {
239        year: (y + i64::from(month <= 2)) as i32,
240        month,
241        day,
242        hour: (secs / 3600) as u32,
243        minute: (secs / 60 % 60) as u32,
244        second: (secs % 60) as u32,
245    }
246}
247
248/// Day of the week for a day count since 1970-01-01, `0` = Sunday.
249///
250/// 1970-01-01 was a Thursday, which is why the `+ 4`.
251fn weekday(days: i64) -> u32 {
252    (days + 4).rem_euclid(7) as u32
253}
254
255fn days_in_month(year: i32, month: u32) -> u32 {
256    match month {
257        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
258        4 | 6 | 9 | 11 => 30,
259        2 if is_leap(year) => 29,
260        2 => 28,
261        _ => 30,
262    }
263}
264
265fn is_leap(year: i32) -> bool {
266    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
267}
268
269#[cfg(test)]
270mod tests {
271    use super::*;
272
273    #[test]
274    fn epoch_round_trips() {
275        assert_eq!(days_from_civil(1970, 1, 1), 0);
276        let dt = civil(0);
277        assert_eq!((dt.year, dt.month, dt.day), (1970, 1, 1));
278    }
279
280    #[test]
281    fn known_dates() {
282        // 2000-02-29 was a Tuesday, and a leap day only a 400-rule gets right.
283        let days = days_from_civil(2000, 2, 29);
284        let dt = civil(days * 86_400 + 13 * 3600 + 45 * 60 + 7);
285        assert_eq!((dt.year, dt.month, dt.day), (2000, 2, 29));
286        assert_eq!((dt.hour, dt.minute, dt.second), (13, 45, 7));
287        assert_eq!(weekday(days), 2, "Tuesday");
288    }
289
290    #[test]
291    fn weekdays_are_right() {
292        // 1970-01-01 Thursday, then one of each following day.
293        for (offset, expected) in [(0, 4), (1, 5), (2, 6), (3, 0), (4, 1), (5, 2), (6, 3)] {
294            assert_eq!(weekday(offset), expected, "day {offset} after the epoch");
295        }
296    }
297
298    #[test]
299    fn filetime_epoch_matches() {
300        let dt = LocalZone::utc().convert(UNIX_EPOCH_FILETIME).unwrap();
301        assert_eq!((dt.year, dt.month, dt.day, dt.hour), (1970, 1, 1, 0));
302    }
303
304    #[test]
305    fn zero_filetime_is_unknown() {
306        assert!(LocalZone::utc().convert(0).is_none());
307    }
308
309    #[test]
310    fn last_sunday_of_march_2026() {
311        // The EU moves to summer time on the last Sunday of March: 2026-03-29.
312        let rule = Transition {
313            month: 3,
314            dow: 0,
315            week: 5,
316            hour: 1,
317            minute: 0,
318        };
319        assert_eq!(rule.day_of_month(2026), 29);
320    }
321
322    #[test]
323    fn second_sunday_of_march_2026() {
324        // The US rule, for the same year: 2026-03-08.
325        let rule = Transition {
326            month: 3,
327            dow: 0,
328            week: 2,
329            hour: 2,
330            minute: 0,
331        };
332        assert_eq!(rule.day_of_month(2026), 8);
333    }
334}
