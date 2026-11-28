//! `0x3314b0`: the system time the sound pause records
//! (docs/engine/sound-instances.md, "Entries").
//!
//! The guest reads the kernel's `KeTickCount` through its import slot; the
//! kernel keeps that word current in guest memory, so the read is the call.

use crate::memory::{GuestMemory, MemFault};

/// `0x411690`: the import slot holding `KeTickCount`'s address.
pub const TICK_COUNT_IMPORT: u32 = 0x0041_1690;

/// `0x3314b0() -> eax`: `[[0x411690]]`, the kernel's tick count in ms.
pub fn system_time(m: &dyn GuestMemory) -> Result<u32, MemFault> {
    m.u32(m.u32(TICK_COUNT_IMPORT)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FakeGuest;

    #[test]
    fn the_system_time_is_the_word_the_import_slot_names() {
        let mut g = FakeGuest::new();
        g.put_u32(TICK_COUNT_IMPORT, 0x0080_0010).put_u32(0x0080_0010, 123_456);
        assert_eq!(system_time(&g), Ok(123_456));
    }

    #[test]
    fn an_unmapped_tick_count_faults_with_its_address() {
        let mut g = FakeGuest::new();
        g.put_u32(TICK_COUNT_IMPORT, 0x0080_0010);
        assert_eq!(system_time(&g).unwrap_err().addr, 0x0080_0010);
    }
}
