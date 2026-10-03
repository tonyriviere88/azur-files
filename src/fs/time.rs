//! `FILETIME` to a local date, without a syscall per row.
//!
//! The obvious way to fill the Modified column is `FileTimeToSystemTime` +
//! `SystemTimeToTzSpecificLocalTime`, which is two kernel transitions per cell.
//! That is fine for the forty rows on screen and ruinous for the sort, the
//! column-width measurement, or any future grouping pass — so the timezone rules
//! are read **once** into [`LocalZone`] and every conversion after that is
//! integer arithmetic.
//!
//! The civil-calendar maths is Howard Hinnant's `days_from_civil` /
//! `civil_from_days`, which is exact for any year and branch-free apart from the
//! leap-year test.
//!
//! # The one inaccuracy
//!
//! Windows keeps a *dynamic* timezone database: one set of daylight-saving rules
//! per year, so a timestamp from before a country last moved its transition dates
//! converts correctly. [`GetTimeZoneInformation`] hands over only the rules in
//! force now, and this applies them to every year. A file last written under an
//! older rule can therefore read an hour out. Explorer's own tooltip has the same
//! quirk, and the alternative is a syscall per cell.

/// 1601-01-01 to 1970-01-01, in `FILETIME` ticks.
///
/// Only the portable scanner needs it — the Windows one is handed `FILETIME`s
/// already — but it is the number this whole module is built around, so it is
/// stated here rather than buried in a `cfg` block.
#[allow(dead_code)]
pub const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;

/// Seconds between the `FILETIME` epoch and the Unix epoch.
const FILETIME_TO_UNIX_SECS: i64 = 11_644_473_600;

/// The write time out of a [`std::fs::Metadata`], as a raw `FILETIME`.
///
/// Nothing else in this program needs one: the scanner is handed `FILETIME`s by the enumeration
/// itself, which is the whole point of [`crate::fs::scan`], and asking `metadata()` per row is the
/// 202× mistake that module is built to avoid. This exists for [`crate::archive`], which stats one
/// file — the archive — to key its cache on, and wants the timestamp in the same units as
/// everything else here.
///
/// `0` when the platform will not say, which is [`LocalZone::convert`]'s "no date".
pub fn filetime_of(found: &std::fs::Metadata) -> u64 {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        found.last_write_time()
    }
    #[cfg(not(windows))]
    {
        found
            .modified()
            .ok()
            .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| {
                (since.as_secs() + FILETIME_TO_UNIX_SECS as u64).saturating_mul(10_000_000)
                    + since.subsec_nanos() as u64 / 100
            })
            .unwrap_or(0)
    }
}

#[cfg(windows)]
#[path = "../windows/time.rs"]
mod win;

/// A broken-down local date and time. No timezone: it is already local.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DateTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

/// One daylight-saving transition, as Windows states it: "the `week`th `dow` of
/// `month`, at `hour:minute`", where week 5 means the last one.
#[derive(Clone, Copy, Debug)]
struct Transition {
    month: u32,
    /// 0 = Sunday, as `SYSTEMTIME::wDayOfWeek` counts.
    dow: u32,
    /// 1..=5, where 5 is "the last one in the month".
    week: u32,
    hour: u32,
    minute: u32,
}

/// The machine's timezone, resolved once.
#[derive(Clone, Debug)]
pub struct LocalZone {
    /// Minutes to add to UTC outside daylight saving.
    std_offset: i32,
    /// Minutes to add to UTC during daylight saving.
    dst_offset: i32,
    /// When daylight saving starts and ends, if this zone has any.
    dst: Option<(Transition, Transition)>,
}

impl LocalZone {
    /// UTC — the fallback when the platform will not say, and what the non-Windows
    /// build uses.
    pub const fn utc() -> Self {
        Self {
            std_offset: 0,
            dst_offset: 0,
            dst: None,
        }
    }

    /// Read the current zone from the operating system.

    #[cfg(not(windows))]
    pub fn current() -> Self {
        // A portable local-time lookup means parsing the TZif database, which is
        // a dependency this program does not otherwise need. The Windows build is
        // the one that has to be right.
        Self::utc()
    }

    /// Convert a raw `FILETIME` to local civil time.
    ///
    /// `None` for the zero timestamp, which is what a filesystem reports when it
    /// has no idea — and which would otherwise print as 1601.
    pub fn convert(&self, filetime: u64) -> Option<DateTime> {
        if filetime == 0 {
            return None;
        }
        let utc = (filetime / 10_000_000) as i64 - FILETIME_TO_UNIX_SECS;
        let offset = self.offset_at(utc);
        Some(civil(utc + offset as i64 * 60))
    }

    /// Local civil time back to a raw `FILETIME` — the inverse of [`LocalZone::convert`].
    ///
    /// Here for exactly one input: **a zip entry's timestamp is a DOS timestamp**, which is local
    /// wall-clock time with no zone recorded anywhere in the format. Every other date in this
    /// program arrives as a `FILETIME` already — the enumeration hands one over, a tar stores Unix
    /// seconds UTC, a `.7z` stores a `FILETIME` outright — and [`crate::fs::dir::Entry::modified`]
    /// is a `FILETIME`, so a zip's dates have to come the other way through the same rules or they
    /// arrive an offset out. Which is visible: an hour or two wrong on every row of every zip, in
    /// the one column where being nearly right looks like being right.
    ///
    /// # Its inaccuracy is the module's, plus one hour a year
    ///
    /// The zone offset depends on the instant, and the instant is what is being worked out, so
    /// this resolves the offset from standard local time exactly as [`LocalZone::offset_at`]
    /// already does for the forward direction. That is exact except inside the hour a transition
    /// moves, where a local wall-clock time is either ambiguous (it happens twice) or does not
    /// exist at all — which is a property of wall-clock time and not of this function. The
    /// ambiguous hour resolves to daylight time and the missing one to standard.
    ///
    /// `0` — [`LocalZone::convert`]'s "no date", which draws as a dash — for anything at or before
    /// the `FILETIME` epoch, which no zip can legitimately carry: a DOS timestamp cannot express a
    /// year before 1980.
    pub fn filetime_from_local(&self, at: DateTime) -> u64 {
        let local = days_from_civil(at.year, at.month, at.day) * 86_400
            + at.hour as i64 * 3_600
            + at.minute as i64 * 60
            + at.second as i64;
        // Standard time to pick the year and the daylight window, which is the same approximation
        // `offset_at` makes about its own argument.
        let offset = self.offset_at(local - self.std_offset as i64 * 60);
        let utc = local - offset as i64 * 60;
        if utc <= -FILETIME_TO_UNIX_SECS {
            return 0;
        }
        ((utc + FILETIME_TO_UNIX_SECS) as u64).saturating_mul(10_000_000)
    }

    /// Which of the two offsets is in force at a UTC instant.
    fn offset_at(&self, utc_secs: i64) -> i32 {
        let Some((start, end)) = self.dst else {
            return self.std_offset;
        };
        // Both transitions are stated in local time. Resolving them needs a year,
        // and the year needs a conversion — so this uses standard local time to
        // pick the year and to compare, which is exact except within the hour of
        // a transition itself.
        let local = utc_secs + self.std_offset as i64 * 60;
        let year = civil(local).year;
        let dst_starts = start.instant(year);
        let dst_ends = end.instant(year);

        let in_daylight = if dst_starts <= dst_ends {
            // Northern hemisphere: the daylight window sits inside the year.
            local >= dst_starts && local < dst_ends
        } else {
            // Southern: it straddles New Year.
            local >= dst_starts || local < dst_ends
        };
        if in_daylight {
            self.dst_offset
        } else {
            self.std_offset
        }
    }
}

#[cfg(windows)]
impl From<&windows_sys::Win32::Foundation::SYSTEMTIME> for Transition {
    fn from(st: &windows_sys::Win32::Foundation::SYSTEMTIME) -> Self {
        Self {
            month: st.wMonth as u32,
            dow: st.wDayOfWeek as u32,
            week: st.wDay as u32,
            hour: st.wHour as u32,
            minute: st.wMinute as u32,
        }
    }
}

impl Transition {
    /// The instant this rule fires in `year`, as local seconds since the Unix
    /// epoch.
    fn instant(&self, year: i32) -> i64 {
        let day = self.day_of_month(year);
        days_from_civil(year, self.month, day) * 86_400
            + self.hour as i64 * 3600
            + self.minute as i64 * 60
    }

    /// Resolve "the `week`th `dow` of `month`" to a day of the month.
    fn day_of_month(&self, year: i32) -> u32 {
        let first = weekday(days_from_civil(year, self.month, 1));
        // The first `dow` on or after the 1st.
        let first_match = 1 + (self.dow + 7 - first) % 7;
        let mut day = first_match + (self.week.saturating_sub(1)) * 7;
        // Week 5 means "the last one", and a fifth occurrence often does not
        // exist — step back until the day is real.
        let last = days_in_month(year, self.month);
        while day > last {
            day -= 7;
        }
        day
    }
}

/// Days since 1970-01-01 for a civil date. Hinnant's algorithm: shifts the year
/// to start in March so the leap day lands at the end and needs no special case.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year } as i64;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // 0..=399
    let m = month as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day as i64 - 1; // 0..=365
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // 0..=146096
    era * 146_097 + doe - 719_468
}

/// The inverse, plus the time of day, from seconds since the Unix epoch.
fn civil(unix_secs: i64) -> DateTime {
    // Floor division, so dates before 1970 do not round towards zero and land a
    // day out.
    let days = unix_secs.div_euclid(86_400);
    let secs = unix_secs.rem_euclid(86_400);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // 0..=146096
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // 0..=399
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..=365
    let mp = (5 * doy + 2) / 153; // 0..=11, March-based
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;

    DateTime {
        year: (y + i64::from(month <= 2)) as i32,
        month,
        day,
        hour: (secs / 3600) as u32,
        minute: (secs / 60 % 60) as u32,
        second: (secs % 60) as u32,
    }
}

/// Day of the week for a day count since 1970-01-01, `0` = Sunday.
///
/// 1970-01-01 was a Thursday, which is why the `+ 4`.
fn weekday(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 30,
    }
}

fn is_leap(year: i32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_round_trips() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        let dt = civil(0);
        assert_eq!((dt.year, dt.month, dt.day), (1970, 1, 1));
    }

    #[test]
    fn known_dates() {
        // 2000-02-29 was a Tuesday, and a leap day only a 400-rule gets right.
        let days = days_from_civil(2000, 2, 29);
        let dt = civil(days * 86_400 + 13 * 3600 + 45 * 60 + 7);
        assert_eq!((dt.year, dt.month, dt.day), (2000, 2, 29));
        assert_eq!((dt.hour, dt.minute, dt.second), (13, 45, 7));
        assert_eq!(weekday(days), 2, "Tuesday");
    }

    #[test]
    fn weekdays_are_right() {
        // 1970-01-01 Thursday, then one of each following day.
        for (offset, expected) in [(0, 4), (1, 5), (2, 6), (3, 0), (4, 1), (5, 2), (6, 3)] {
            assert_eq!(weekday(offset), expected, "day {offset} after the epoch");
        }
    }

    #[test]
    fn filetime_epoch_matches() {
        let dt = LocalZone::utc().convert(UNIX_EPOCH_FILETIME).unwrap();
        assert_eq!((dt.year, dt.month, dt.day, dt.hour), (1970, 1, 1, 0));
    }

    #[test]
    fn zero_filetime_is_unknown() {
        assert!(LocalZone::utc().convert(0).is_none());
    }

    #[test]
    fn last_sunday_of_march_2026() {
        // The EU moves to summer time on the last Sunday of March: 2026-03-29.
        let rule = Transition {
            month: 3,
            dow: 0,
            week: 5,
            hour: 1,
            minute: 0,
        };
        assert_eq!(rule.day_of_month(2026), 29);
    }

    #[test]
    fn second_sunday_of_march_2026() {
        // The US rule, for the same year: 2026-03-08.
        let rule = Transition {
            month: 3,
            dow: 0,
            week: 2,
            hour: 2,
            minute: 0,
        };
        assert_eq!(rule.day_of_month(2026), 8);
    }
}
