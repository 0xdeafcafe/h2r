//! Little-endian field readers and writers over a byte slice, the one copy
//! the tag decoders and the Halo 1 tools share.
//!
//! Every function indexes the slice directly, so a read or write that runs
//! past the end panics. Callers bound-check a record's size once, up front.

/// The byte at `at`.
pub fn u8_at(b: &[u8], at: usize) -> u8 {
    b[at]
}

/// The little-endian `u16` at `at`.
pub fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// The little-endian `i16` at `at`.
pub fn i16_at(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

/// The little-endian `u32` at `at`.
pub fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The little-endian `i32` at `at`.
pub fn i32_at(b: &[u8], at: usize) -> i32 {
    i32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// The little-endian `f32` at `at`.
pub fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(b, at))
}

/// Writes `v` little-endian at `at`.
pub fn put_u16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// Writes `v` little-endian at `at`.
pub fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Writes `v` little-endian at `at`.
pub fn put_f32(b: &mut [u8], at: usize, v: f32) {
    put_u32(b, at, v.to_bits());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readers_are_little_endian() {
        let b = [0x01, 0x02, 0x03, 0x84, 0x05];
        assert_eq!(u8_at(&b, 4), 5);
        assert_eq!(u16_at(&b, 0), 0x0201);
        assert_eq!(i16_at(&b, 2), 0x8403u16 as i16);
        assert_eq!(u32_at(&b, 0), 0x8403_0201);
        assert_eq!(i32_at(&b, 0), 0x8403_0201u32 as i32);
        assert_eq!(f32_at(&1.5f32.to_le_bytes(), 0), 1.5);
    }

    #[test]
    fn writers_round_trip_through_the_readers() {
        let mut b = [0u8; 12];
        put_u16(&mut b, 0, 0xBEEF);
        put_u32(&mut b, 2, 0xDEAD_BEEF);
        put_f32(&mut b, 6, -2.25);
        assert_eq!(u16_at(&b, 0), 0xBEEF);
        assert_eq!(u32_at(&b, 2), 0xDEAD_BEEF);
        assert_eq!(f32_at(&b, 6), -2.25);
        assert_eq!(&b[10..], &[0, 0]);
    }

    #[test]
    #[should_panic]
    fn u8_past_the_end_panics() {
        u8_at(&[1], 1);
    }

    #[test]
    #[should_panic]
    fn u16_past_the_end_panics() {
        u16_at(&[1, 2, 3], 2);
    }

    #[test]
    #[should_panic]
    fn i16_past_the_end_panics() {
        i16_at(&[1, 2, 3], 2);
    }

    #[test]
    #[should_panic]
    fn u32_past_the_end_panics() {
        u32_at(&[1, 2, 3, 4, 5], 2);
    }

    #[test]
    #[should_panic]
    fn i32_past_the_end_panics() {
        i32_at(&[1, 2, 3, 4, 5], 2);
    }

    #[test]
    #[should_panic]
    fn f32_past_the_end_panics() {
        f32_at(&[1, 2, 3, 4, 5], 2);
    }

    #[test]
    #[should_panic]
    fn writers_past_the_end_panic() {
        put_u32(&mut [0; 5], 2, 1);
    }
}
