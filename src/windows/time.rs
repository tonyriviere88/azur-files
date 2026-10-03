//! The local timezone, as Windows states it: a bias, and two daylight-saving transitions.
//!
//! The Windows half of [`crate::fs::time`]. A second inherent `impl` block rather than a free
//! function, so the call site is `LocalZone::current()` on every platform.

use super::*;

impl LocalZone {
    #[cfg(windows)]
    pub fn current() -> Self {
        use windows_sys::Win32::System::Time::{
            GetTimeZoneInformation, TIME_ZONE_ID_INVALID, TIME_ZONE_INFORMATION,
        };

        let mut info = TIME_ZONE_INFORMATION::default();
        if unsafe { GetTimeZoneInformation(&mut info) } == TIME_ZONE_ID_INVALID {
            return Self::utc();
        }

        // Windows states the bias as "UTC = local + bias", so the offset that
        // takes UTC to local is its negation.
        let std_offset = -(info.Bias + info.StandardBias);
        let dst_offset = -(info.Bias + info.DaylightBias);

        // `wMonth == 0` is how Windows says "this zone does not observe daylight
        // saving". A non-zero `wYear` means an absolute, one-off date rather than
        // a recurring rule; nothing in the wild ships that through this API, and
        // treating it as "no DST" is safer than misreading it as a rule.
        let recurring = info.DaylightDate.wMonth != 0
            && info.StandardDate.wMonth != 0
            && info.DaylightDate.wYear == 0
            && info.StandardDate.wYear == 0;

        let dst = recurring.then(|| {
            (
                Transition::from(&info.DaylightDate),
                Transition::from(&info.StandardDate),
            )
        });

        Self {
            std_offset,
            dst_offset,
            dst,
        }
    }
}
