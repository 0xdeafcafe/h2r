//! Little-endian field reads. Short input reads as zero: callers check a
//! struct's length once, against its layout's size.

pub(crate) fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}

pub(crate) fn u16_at(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
        .unwrap_or(0)
}

pub(crate) fn i16_at(b: &[u8], at: usize) -> i16 {
    u16_at(b, at) as i16
}

pub(crate) fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(b, at))
}

pub(crate) fn vec3(b: &[u8], at: usize) -> [f32; 3] {
    std::array::from_fn(|i| f32_at(b, at + 4 * i))
}

/// A NUL-terminated string in a fixed field (lossy).
pub(crate) fn fixed_str(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}
