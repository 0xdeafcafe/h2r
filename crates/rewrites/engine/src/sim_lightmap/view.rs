//! What the sampler reads (docs/engine/sim-render-split.md, "6. Render
//! scratch and stamps", the effect tint): guest memory for the tags and
//! the simulation, and the lightmap store for every byte the guest reads
//! out of its geometry and texture caches.
//!
//! The store's bytes sit behind token addresses ([`WINDOW`] up), laid out
//! as the guest's caches lay them out once resident: a section's resource
//! pointers name tokens, as `0x00223c70` would have pointed them into the
//! cache. So the port reads `[section + 4]` and follows it as the guest
//! does, and only the residency calls change:
//!
//! | Guest | Here |
//! |---|---|
//! | `0x0012de70` (flags 3) on a block info, then its runtime section `[info + [info+0x1c] + 4]` | [`View::section`] |
//! | the texture cache's get of a bitmap image (`0x000d15e0`, `0x00012360`, `0x0001cfb0`, `0x000d1680`, `0x000d1630`, all through `0x0012ccf0`) and the pixels behind it | [`View::pixels`] |
//! | the PRT table `0x004b89b0` (`[0x005093e8]` entries) the render rebuilds each view (`0x00022850`) | [`View::prt_entries`]: none, as on a frame no view drew |
//!
//! A write through a [`View`] faults: the sampler writes only its output,
//! which the caller stores.
use super::store::Store;
use crate::sim_native::Memory;
use runtime::GuestFault;

/// The first token. Below the clip store's window (`0xe0000000`) and above
/// the Live HLE window, which stops at `0xa1000000`; the store is refused
/// when a guest mapping overlaps it.
pub const WINDOW: u32 = 0xd000_0000;
/// Bytes the window spans: the most the store holds.
pub const WINDOW_LEN: u32 = 0x1000_0000;

pub fn is_token(addr: u32) -> bool {
    addr.wrapping_sub(WINDOW) < WINDOW_LEN
}

/// Guest memory with the store laid over the token window.
pub struct View<'a, M: Memory> {
    pub guest: &'a M,
    pub store: &'a Store,
}

impl<'a, M: Memory> View<'a, M> {
    pub fn new(guest: &'a M, store: &'a Store) -> Self {
        Self { guest, store }
    }

    /// `0x0012de70(info, 3)` and the runtime section pointer it makes
    /// valid, `[info + i16 [info+0x1c] + 4]`:
    ///
    /// - a block the store holds: the token of its fixed-up section;
    /// - an in-tag section (no raw block, `+0` -1, with the loaded flag
    ///   `+0x1e` set, as the lightmap's own sections are on the disc,
    ///   docs/formats/structure-bsp.md): the guest's pointer, which
    ///   points into the tag and so is always resident (`0x0012de7b`
    ///   returns true at once for it);
    /// - otherwise `None`, the guest's false for a block it cannot have.
    pub fn section(&self, info: u32) -> Result<Option<u32>, GuestFault> {
        if let Some(token) = self.store.section(info) {
            return Ok(Some(token));
        }
        let raw = self.guest.u32(info)?;
        if raw == u32::MAX {
            if self.guest.u8(info.wrapping_add(0x1e))? != 0 {
                let offset = self.guest.u16(info.wrapping_add(0x1c))? as i16 as i32 as u32;
                return Ok(Some(self.guest.u32(info.wrapping_add(offset).wrapping_add(4))?));
            }
            return Ok(None);
        }
        // A block the structure's load did not list (a model's section):
        // read from the map file now, the bytes the cache would load.
        super::late_block(self.guest, self.store, info)
    }

    /// The pixels of the bitmap image record at `image` (a `bitm` image,
    /// as the texture cache's get takes it in EAX): the token of its
    /// highest level's bytes, `None` when the store holds none for it.
    pub fn pixels(&self, image: u32) -> Result<Option<u32>, GuestFault> {
        if let Some(token) = self.store.pixels(image) {
            return Ok(Some(token));
        }
        // An image the structure's load did not list (a shader's base
        // map): its level 0 from the map file now.
        super::late_pixels(self.guest, self.store, image)
    }

    /// `[0x005093e8]`, the PRT table's entry count, as the sampler reads
    /// it at `0x00024490` and `0x000241c0`: always 0. The table holds the
    /// instances the last view drew; the simulation reads it as a frame
    /// that drew none, the answer of a headless or skipped frame.
    pub fn prt_entries(&self) -> u32 {
        0
    }
}

fn refused(addr: u32, len: usize) -> GuestFault {
    GuestFault {
        addr,
        len,
        access: runtime::guest::Access::Write,
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

impl<M: Memory> Memory for View<'_, M> {
    fn read_bytes(&self, addr: u32, dst: &mut [u8], alignment: u32) -> Result<(), GuestFault> {
        if is_token(addr) {
            return self.store.read(addr, dst);
        }
        self.guest.read_bytes(addr, dst, alignment)
    }

    fn write_bytes(&mut self, addr: u32, src: &[u8], _alignment: u32) -> Result<(), GuestFault> {
        Err(refused(addr, src.len()))
    }
}
