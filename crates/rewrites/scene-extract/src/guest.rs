//! How extraction reads guest memory: implemented by the feature over the
//! runtime's memory, by a word map in tests ([`crate::testing::Mem`]).

/// Reads guest memory.
pub trait GuestRead {
    fn read_u32(&self, addr: u32) -> Option<u32>;

    fn read_f32(&self, addr: u32) -> Option<f32> {
        self.read_u32(addr).map(f32::from_bits)
    }
}

/// Writes guest memory (the skip's mask bits only, until slice 4
/// replaces the masks by whole calls).
pub trait GuestWrite: GuestRead {
    fn write_u32(&mut self, addr: u32, value: u32) -> Option<()>;
}
