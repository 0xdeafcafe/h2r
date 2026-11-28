//! The layer draw's open shader (docs/graphics/retire-d3d-remainders.md,
//! "The open shader"): a layer draw whose items are all native but whose
//! shader-draw cache holds an open shader returns once its restore list
//! is written.
//!
//! The body's `0x00016b10` calls `0x00018900` when the cache's `+0x24`
//! (an implementation) is set: for each of the implementation's restore
//! entries (the `u16` at `+0x11c`: first `& 0x1ff`, count `>> 9`; five
//! bytes each in the cached descriptor's `+0x14` table, a state byte and
//! a value) it writes the state as `SetRenderState` does inline
//! ([`Write`]). Then the rest of the body's writes follow
//! (`frame_step::layer::writes`).
//!
//! The host writes the list through the device as the guest's calls
//! would ([`crate::native_layers::Host::restore`], [`replay`]); the
//! guest's `0x00018900` never runs. A list the host does not take panics,
//! `open shader: <why>` (`host`, `unread`, the host's own cause).
//!
//! Counted in `render.scene` as `open_shader_<outcome>`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use guest_layout::memory::{GuestMemory, MemFault};
use engine::render_loop::states::{HEADERS, MASKS};

use crate::frame_step::layer::{SETUP, SETUP_OPEN};

/// `0x00018900`: stdcall (context, restore range), `ret 8`.
pub const RESTORE: u32 = 0x0001_8900;
/// The cache's pass descriptor.
pub const SETUP_DESCRIPTOR: u32 = 0x20;
/// The implementation's restore range.
pub const IMPLEMENTATION_RESTORE: u32 = 0x11c;
/// The descriptor's render-state table.
pub const DESCRIPTOR_STATES: u32 = 0x14;
/// Bytes a table entry takes.
pub const ENTRY: u32 = 5;
/// The setters of states `0x88` and up, by state.
pub const SETTERS: u32 = 0x0040_38d8;

/// One restore entry as `0x00018900` applies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// Below `0x5c`: `SetRenderState_Simple` (`0x003f5cf0`, ECX the push
    /// header from `0x0043e6f0`, EDX the value), then the shadow.
    Simple { state: u8, header: u32, value: u32 },
    /// Below `0x88`: the state's mask (`0x0043e4d0`) into the dirty word,
    /// then the shadow.
    Deferred { state: u8, mask: u32, value: u32 },
    /// `0x88` and up: the state's stdcall setter (`0x004038d8`).
    Setter { state: u8, setter: u32, value: u32 },
}

impl Write {
    pub fn state(self) -> u8 {
        match self {
            Write::Simple { state, .. } | Write::Deferred { state, .. } | Write::Setter { state, .. } => state,
        }
    }

    pub fn value(self) -> u32 {
        match self {
            Write::Simple { value, .. } | Write::Deferred { value, .. } | Write::Setter { value, .. } => value,
        }
    }
}

/// The cache's open shader and its restore list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restore {
    /// The implementation (`[0x00485b48 + 0x24]`).
    pub implementation: u32,
    pub writes: Vec<Write>,
}

/// The open shader's restore list as `0x00018900` reads it; `None` when
/// the cache holds none.
pub fn read(mem: &dyn GuestMemory) -> Result<Option<Restore>, MemFault> {
    let implementation = mem.u32(SETUP + SETUP_OPEN)?;
    if implementation == 0 {
        return Ok(None);
    }
    let range = u32::from(mem.u16(implementation.wrapping_add(IMPLEMENTATION_RESTORE))?);
    let descriptor = mem.u32(SETUP + SETUP_DESCRIPTOR)?;
    let table = mem.u32(descriptor.wrapping_add(DESCRIPTOR_STATES))?;
    let mut at = table.wrapping_add(ENTRY * (range & 0x1ff));
    let mut writes = Vec::with_capacity((range >> 9) as usize);
    for _ in 0..range >> 9 {
        let state = mem.u8(at)?;
        let value = mem.u32(at.wrapping_add(1))?;
        let s = u32::from(state);
        writes.push(if s < 0x5c {
            Write::Simple { state, header: mem.u32(HEADERS + 4 * s)?, value }
        } else if s < 0x88 {
            Write::Deferred { state, mask: mem.u32(MASKS + 4 * s)?, value }
        } else {
            Write::Setter { state, setter: mem.u32(SETTERS + 4 * s)?, value }
        });
        at = at.wrapping_add(ENTRY);
    }
    Ok(Some(Restore { implementation, writes }))
}

static COUNTS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// `open_shader_<outcome>`.
pub fn count(outcome: &str, n: u64) {
    *COUNTS.lock().unwrap_or_else(|e| e.into_inner()).entry(format!("open_shader_{outcome}")).or_default() += n;
}

pub fn counters() -> Vec<(String, u64)> {
    COUNTS.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(k, &v)| (k.clone(), v)).collect()
}

/// `0x00018900` on `restore`: its writes through `replay` (the host's,
/// [`crate::native_layers::Host::restore`]), counted `replayed`. `Err`
/// names why the host did not take the list.
pub fn replay(
    mem: &mut dyn GuestMemory,
    restore: &Restore,
    replay: impl FnOnce(&mut dyn GuestMemory, &[Write]) -> Result<(), &'static str>,
) -> Result<(), &'static str> {
    replay(mem, &restore.writes)?;
    count("replayed", 1);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;
    use engine::render_loop::states::DIRTY;

    const DESCRIPTOR: u32 = 0x0045_1000;
    const TABLE: u32 = 0x0045_2000;
    const IMPLEMENTATION: u32 = 0x0045_3000;

    /// An open shader whose restore list is entries 2..5 of the table: a
    /// simple state, a deferred one and a setter's, the simple one twice.
    fn open() -> FlatMemory {
        let mut m = FlatMemory::new(0x0040_0000, 0x10_0000);
        m.set_u32(SETUP + SETUP_OPEN, IMPLEMENTATION).unwrap();
        m.set_u32(SETUP + SETUP_DESCRIPTOR, DESCRIPTOR).unwrap();
        m.set_u32(DESCRIPTOR + DESCRIPTOR_STATES, TABLE).unwrap();
        m.set_u16(IMPLEMENTATION + IMPLEMENTATION_RESTORE, 4 << 9 | 2).unwrap();
        for (n, (state, value)) in [(0u8, 9), (0, 9), (0x10, 1), (0x60, 7), (0x8f, 3), (0x10, 5)].into_iter().enumerate() {
            m.set_u8(TABLE + ENTRY * n as u32, state).unwrap();
            m.set_u32(TABLE + ENTRY * n as u32 + 1, value).unwrap();
        }
        m.set_u32(HEADERS + 4 * 0x10, 0x0004_0300).unwrap();
        m.set_u32(MASKS + 4 * 0x60, 0x20).unwrap();
        m.set_u32(SETTERS + 4 * 0x8f, 0x003f_54f0).unwrap();
        m.set_u32(DIRTY, 0x101).unwrap();
        m
    }

    #[test]
    fn the_restore_list_is_read_and_classed_as_the_original_applies_it() {
        let m = open();
        let r = read(&m).unwrap().unwrap();
        assert_eq!(r.implementation, IMPLEMENTATION);
        assert_eq!(r.writes, [
            Write::Simple { state: 0x10, header: 0x0004_0300, value: 1 },
            Write::Deferred { state: 0x60, mask: 0x20, value: 7 },
            Write::Setter { state: 0x8f, setter: 0x003f_54f0, value: 3 },
            Write::Simple { state: 0x10, header: 0x0004_0300, value: 5 },
        ]);
        let mut closed = open();
        closed.set_u32(SETUP + SETUP_OPEN, 0).unwrap();
        assert_eq!(read(&closed).unwrap(), None, "no open shader");
    }

    #[test]
    fn the_replay_takes_the_whole_list_or_names_why_not() {
        let mut m = open();
        let r = read(&m).unwrap().unwrap();
        let mut seen = 0;
        replay(&mut m, &r, |_, writes| {
            seen = writes.len();
            Ok(())
        })
        .unwrap();
        assert_eq!(seen, 4);
        assert_eq!(replay(&mut m, &r, |_, _| Err("setter")), Err("setter"));
        let c: BTreeMap<String, u64> = counters().into_iter().collect();
        assert!(c["open_shader_replayed"] >= 1);
    }
}
