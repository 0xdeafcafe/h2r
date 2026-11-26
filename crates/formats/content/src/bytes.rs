//! The little-endian reader the package list decoders share. It indexes
//! the slice, so a read past the end panics; callers check a record's size
//! first.

pub(crate) fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
