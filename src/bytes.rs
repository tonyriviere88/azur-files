//! Little-endian integers out of a byte slice, for the readers that parse a file format by hand:
//! [`crate::pe`]'s headers and [`crate::fs::recycle`]'s `$I…` files.
//!
//! Both read files that anything could have written, so a field that runs off the end of the input
//! is `None` rather than a panic or a read of whatever happened to come next.

/// The `u16` at byte `at`, or `None` if `bytes` is too short to hold it.
pub(crate) fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

/// The `u32` at byte `at`, or `None` if `bytes` is too short to hold it.
pub(crate) fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The `u64` at byte `at`, or `None` if `bytes` is too short to hold it.
pub(crate) fn u64_at(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}
