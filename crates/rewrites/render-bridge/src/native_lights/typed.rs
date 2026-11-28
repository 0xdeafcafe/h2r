//! The read-only mirror the cut light frame plans its groups over
//! ([`super::scene_frame::groups`]): the shadow setup's stores laid over
//! guest memory for the read, nothing written.

use engine::memory::{GuestMemory, MemFault};
use engine::render::light_gather::Writes;

/// Record the same stores the guest-memory writers make, for reads only.
pub(super) struct Mirror<'a> {
    pub(super) mem: &'a dyn GuestMemory,
    pub(super) writes: Writes,
}

impl GuestMemory for Mirror<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        buf.copy_from_slice(&self.writes.read(self.mem, addr, buf.len())?);
        Ok(())
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        self.writes.array(addr, data.to_vec());
        Ok(())
    }
}
