//! Guest memory as a model leaves it: the live bytes under the writes the
//! model has made, so a model reads its own writes as the guest would
//! (`0x00174420` reads a parent's `+0xc` after writing it) and never
//! writes the guest.

use std::collections::BTreeMap;

use guest_layout::memory::{GuestMemory, MemFault};

pub struct Overlay<'a> {
    base: &'a dyn GuestMemory,
    writes: BTreeMap<u32, u8>,
}

impl<'a> Overlay<'a> {
    pub fn new(base: &'a dyn GuestMemory) -> Self {
        Self {
            base,
            writes: BTreeMap::new(),
        }
    }

    /// Every byte written, by address.
    pub fn writes(&self) -> &BTreeMap<u32, u8> {
        &self.writes
    }

    /// `bytes`, read at `addr` before the model ran, with the model's
    /// writes in `addr..addr + bytes.len()` applied.
    pub fn apply(&self, addr: u32, bytes: &mut [u8]) {
        let Some(end) = addr.checked_add(bytes.len() as u32) else {
            return;
        };
        for (&a, &b) in self.writes.range(addr..end) {
            bytes[(a - addr) as usize] = b;
        }
    }
}

impl GuestMemory for Overlay<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.base.read(addr, buf)?;
        self.apply(addr, buf);
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        if addr.checked_add(data.len() as u32).is_none() {
            return Err(MemFault {
                addr,
                len: data.len() as u32,
                write: true,
            });
        }
        // The guest's store faults where its read would: check first.
        let mut probe = vec![0; data.len()];
        self.base.read(addr, &mut probe)?;
        for (i, &b) in data.iter().enumerate() {
            self.writes.insert(addr + i as u32, b);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn reads_see_the_models_writes_and_the_guest_is_untouched() {
        let mut m = FlatMemory::new(0x1000, 0x100);
        m.set_u32(0x1010, 0x1122_3344).unwrap();
        let before = m.bytes.clone();
        let mut o = Overlay::new(&m);
        o.set_u8(0x1011, 0xff).unwrap();
        assert_eq!(o.u32(0x1010).unwrap(), 0x1122_ff44);
        assert!(o.set_u32(0x2000, 1).is_err(), "a store off the map faults");
        let mut bytes = before[0x10..0x14].to_vec();
        o.apply(0x1010, &mut bytes);
        assert_eq!(bytes, [0x44, 0xff, 0x22, 0x11]);
        assert_eq!(m.bytes, before);
    }
}
