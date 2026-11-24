//! Byte, half and word reads at any alignment over [`GuestRead`], which
//! reads aligned words.

use crate::GuestRead;

pub(super) fn u8_at(mem: &dyn GuestRead, addr: u32) -> Option<u8> {
    let word = mem.read_u32(addr & !3)?;
    Some((word >> ((addr & 3) * 8)) as u8)
}

pub(super) fn u16_at(mem: &dyn GuestRead, addr: u32) -> Option<u16> {
    if addr & 1 == 0 {
        let word = mem.read_u32(addr & !3)?;
        return Some((word >> ((addr & 2) * 8)) as u16);
    }
    Some(u16::from(u8_at(mem, addr)?) | (u16::from(u8_at(mem, addr.checked_add(1)?)?) << 8))
}

pub(super) fn u32_at(mem: &dyn GuestRead, addr: u32) -> Option<u32> {
    if addr & 3 == 0 {
        return mem.read_u32(addr);
    }
    let lo = u32::from(u16_at(mem, addr)?);
    let hi = u32::from(u16_at(mem, addr.checked_add(2)?)?);
    Some(lo | (hi << 16))
}

pub(super) fn f32_at(mem: &dyn GuestRead, addr: u32) -> Option<f32> {
    u32_at(mem, addr).map(f32::from_bits)
}

pub(super) fn f64_at(mem: &dyn GuestRead, addr: u32) -> Option<f64> {
    let lo = u64::from(u32_at(mem, addr)?);
    let hi = u64::from(u32_at(mem, addr.checked_add(4)?)?);
    Some(f64::from_bits(lo | (hi << 32)))
}

pub(super) fn vec3_at(mem: &dyn GuestRead, addr: u32) -> Option<[f32; 3]> {
    Some([
        f32_at(mem, addr)?,
        f32_at(mem, addr.checked_add(4)?)?,
        f32_at(mem, addr.checked_add(8)?)?,
    ])
}

/// A non-null pointer.
pub(super) fn ptr_at(mem: &dyn GuestRead, addr: u32) -> Option<u32> {
    u32_at(mem, addr).filter(|&p| p != 0)
}

/// `base + offset`, or `None` on overflow.
pub(super) fn at(base: u32, offset: u32) -> Option<u32> {
    base.checked_add(offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;

    #[test]
    fn unaligned_halves_and_words_span_the_words_they_touch() {
        let mut m = Mem::default();
        m.w(0x100, 0x4433_2211);
        m.w(0x104, 0x8877_6655);
        assert_eq!(u8_at(&m, 0x103), Some(0x44));
        assert_eq!(u16_at(&m, 0x102), Some(0x4433));
        assert_eq!(u16_at(&m, 0x103), Some(0x5544));
        assert_eq!(u32_at(&m, 0x102), Some(0x6655_4433));
        assert_eq!(u32_at(&m, 0x106), None, "the second word is not mapped");
        assert_eq!(ptr_at(&m, 0x200), None);
    }
}
