//! Havok's array header: data pointer, count, then capacity with bit 31
//! set when the array does not own its data (docs/engine/havok-runtime.md).
//! `grow` is `0x002d91d0` (cdecl: header, element size).

use crate::layout::{ARRAY_CAPACITY, ARRAY_COUNT, ARRAY_DATA, ARRAY_NOT_OWNED};
use crate::view::{Fault, GuestRead};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub data: u32,
    pub count: u32,
    /// The raw word, flag included.
    pub capacity: u32,
}

impl Header {
    pub fn read(m: &impl GuestRead, at: u32) -> Result<Self, Fault> {
        Ok(Self {
            data: m.u32(at + ARRAY_DATA)?,
            count: m.u32(at + ARRAY_COUNT)?,
            capacity: m.u32(at + ARRAY_CAPACITY)?,
        })
    }

    /// The array frees its data when it grows.
    pub fn owned(&self) -> bool {
        self.capacity & ARRAY_NOT_OWNED == 0
    }

    pub fn slots(&self) -> u32 {
        self.capacity & !ARRAY_NOT_OWNED
    }

    /// What `0x002d91d0` does: allocate twice the *count* (one when it is
    /// empty) elements, copy `count` of them, free the old data if owned
    /// (sized by its capacity), then store the new pointer and capacity,
    /// which clears the flag. The allocation and the free go through the
    /// memory system with class `0x12`.
    pub fn grow(&self, element: u32) -> Grow {
        let capacity = if self.count == 0 { 1 } else { self.count.wrapping_mul(2) };
        Grow {
            capacity,
            allocate: capacity.wrapping_mul(element),
            copy: self.count.wrapping_mul(element),
            free: self.owned().then(|| (self.data, self.slots().wrapping_mul(element))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grow {
    pub capacity: u32,
    pub allocate: u32,
    pub copy: u32,
    /// The old (pointer, bytes), freed after the copy.
    pub free: Option<(u32, u32)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Bytes;

    #[test]
    fn grow_doubles_the_count_and_frees_only_owned_data() {
        let mut m = Bytes::default();
        m.put_u32(0x10, 0x5000);
        m.put_u32(0x14, 3);
        m.put_u32(0x18, 4);
        let h = Header::read(&m, 0x10).unwrap();
        assert_eq!(
            h.grow(8),
            Grow { capacity: 6, allocate: 48, copy: 24, free: Some((0x5000, 32)) }
        );
        m.put_u32(0x18, 4 | ARRAY_NOT_OWNED);
        let h = Header::read(&m, 0x10).unwrap();
        assert!(!h.owned());
        assert_eq!(h.grow(8).free, None);
        m.put_u32(0x14, 0);
        assert_eq!(Header::read(&m, 0x10).unwrap().grow(4).capacity, 1);
    }
}
