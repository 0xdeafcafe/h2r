//! The dynamic lights' gather as typed data (docs/graphics/native-light-gather.md).
//!
//! - [`visible`]: one row of the visible list ([`VisibleLight`]), the
//!   candidate tests, and the list `0x00031910` builds.
//! - [`history`]: the per-view history ([`History`]), its merge
//!   (`0x0004a7c0`) and its ranking and fade (`0x0004ac30`).
//! - [`geometry`]: the light's geometry flags (`0x00031660`).
//! - [`extra_caster`]: the extra caster (`0x00031c20`).
//! - [`volume`]: the posed light volume (`0x000c17f0`, `0x000c19f0`,
//!   `0x000c1a80`).
//! - [`frame`]: the frame body of `0x00031c80`, which makes the steps
//!   in the guest's order and calls the guest for the steps not ported.
//!   A light's cluster and placement records are
//!   [`crate::render_loop::world_walk`] in the light's kind.
//! - [`items`]: a light's two item lists and the first-person table
//!   (`0x00040e90` in the light's kind, `0x00044ac0`).
//! - [`passes`]: one light's passes (`0x00031170`), which calls the guest
//!   for its lists, constants, projection and each layer's draws.
//! - [`first_person`]: the first-person lights (`0x000404b0`) and each
//!   slot's record replay (`0x000402b0`), with the same layer blocks.
//! - [`first_person_items`]: the first-person draw's items (`0x0003fdf0`'s
//!   record loop and `0x00041040` in kind 0) into list 0 and the sorted
//!   list.
//! - [`object_shadows`]: the ranked object rows and their mask casters.
//!
//! The leaves are functions of guest memory and typed arguments. They
//! write nothing: each returns the guest's writes as [`Writes`], in the
//! guest's order, for the caller to publish.

pub mod extra_caster;
pub mod first_person;
pub mod first_person_effects;
pub mod first_person_items;
pub mod frame;
pub mod geometry;
mod guest;
pub mod history;
pub mod items;
pub mod object_shadows;
pub mod passes;
pub mod typed;
pub mod typed_tail;
pub mod visible;
pub mod volume;

pub use guest::{LIGHT_VIS, MAIN_VIS};
pub use history::{History, HistoryRow};
pub use visible::VisibleLight;

use guest_layout::memory::{GuestMemory, MemFault};

/// Why a leaf did not answer. The caller leaves the step to its guest
/// function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A read faulted.
    Memory,
    /// The guest's structures are not what the port reads.
    Layout,
    /// A case the port does not take (a float state, a view, a range).
    Unsupported,
    /// A resource is not resident; the guest's function requests it.
    Residency,
}

impl From<MemFault> for Refusal {
    fn from(_: MemFault) -> Self {
        Self::Memory
    }
}

pub type Result<T, E = Refusal> = std::result::Result<T, E>;

/// One guest write: its address and bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    pub addr: u32,
    pub bytes: Vec<u8>,
}

/// The writes a leaf makes, in the guest's order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Writes(Vec<Write>);

impl Writes {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn byte(&mut self, addr: u32, value: u8) {
        self.array(addr, vec![value]);
    }

    pub fn half(&mut self, addr: u32, value: u16) {
        self.array(addr, value.to_le_bytes().to_vec());
    }

    pub fn word(&mut self, addr: u32, value: u32) {
        self.array(addr, value.to_le_bytes().to_vec());
    }

    /// Bytes at `addr`; an empty run is no write.
    pub fn array(&mut self, addr: u32, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.0.push(Write { addr, bytes });
        }
    }

    pub fn extend(&mut self, other: Writes) {
        self.0.extend(other.0);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, Write> {
        self.0.iter()
    }

    /// Makes the writes, in order.
    pub fn apply(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        for w in &self.0 {
            m.write(w.addr, &w.bytes)?;
        }
        Ok(())
    }

    /// The value these writes leave at `addr` (`len` bytes), over `m`:
    /// later writes win, as when they are applied.
    pub fn read(&self, m: &dyn GuestMemory, addr: u32, len: usize) -> Result<Vec<u8>, MemFault> {
        let mut out = m.bytes(addr, len)?;
        for w in &self.0 {
            for (i, &b) in w.bytes.iter().enumerate() {
                let at = w.addr.wrapping_add(i as u32).wrapping_sub(addr) as usize;
                if at < len {
                    out[at] = b;
                }
            }
        }
        Ok(out)
    }
}

impl IntoIterator for Writes {
    type Item = Write;
    type IntoIter = std::vec::IntoIter<Write>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}
