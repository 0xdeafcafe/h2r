//! The guest structures the light gather reads: datum pools, the tag
//! table, and the visibility lists (four columns and a count). Each read
//! is checked; a structure the port does not recognise is
//! [`Refusal::Layout`].

use super::{Refusal, Result, Writes};
use guest_layout::memory::GuestMemory;

pub(super) const NONE: u32 = u32::MAX;
pub(super) const TAGS: u32 = guest_layout::tags::TAG_ENTRIES_GLOBAL;
pub(super) const OBJECTS: u32 = crate::render_loop::occlusion::guest::OBJECTS;
pub(super) const LIGHTS: u32 = crate::render::light::ARRAY_POINTER;
pub(super) const LIGHT_BYTES: u32 = crate::render::light::RECORD_SIZE as u32;
/// The light's visibility struct (`V`), which `0x001320f0` fills.
pub const LIGHT_VIS: u32 = 0x0054_aa68;
/// The main view's visibility struct.
pub const MAIN_VIS: u32 = 0x0054_7f88;

pub(super) fn at(base: u32, offset: u32) -> Result<u32> {
    base.checked_add(offset).ok_or(Refusal::Layout)
}

/// A pointer word that must point somewhere.
pub(super) fn ptr(m: &dyn GuestMemory, addr: u32) -> Result<u32> {
    match m.u32(addr)? {
        0 | NONE => Err(Refusal::Layout),
        value => Ok(value),
    }
}

/// A tag's data: the entry of `datum` in the tag table must name it.
pub(super) fn tag(m: &dyn GuestMemory, table: u32, datum: u32) -> Result<u32> {
    if datum == NONE {
        return Err(Refusal::Layout);
    }
    let entry = at(table, (datum & 0xffff) * 16)?;
    if m.u32(at(entry, 4)?)? != datum {
        return Err(Refusal::Layout);
    }
    ptr(m, at(entry, 8)?)
}

/// A datum array: its data, its high-water mark and its stride.
#[derive(Clone, Copy)]
pub(super) struct Pool {
    pub data: u32,
    pub end: u32,
    pub stride: u32,
}

impl Pool {
    pub fn read(m: &dyn GuestMemory, header: u32, stride: u32) -> Result<Self> {
        let end = m.u32(at(header, 0x38)?)?;
        if !(1..=0x10000).contains(&stride) || end > 0x10000 || m.u32(at(header, 0x24)?)? != stride {
            return Err(Refusal::Layout);
        }
        Ok(Self { data: ptr(m, at(header, 0x44)?)?, end, stride })
    }

    /// The slot of a live datum, its salt checked.
    pub fn slot(self, m: &dyn GuestMemory, datum: u32) -> Result<u32> {
        let i = datum & 0xffff;
        if i >= self.end || datum == NONE || datum >> 16 == 0 {
            return Err(Refusal::Layout);
        }
        let slot = at(self.data, i * self.stride)?;
        if m.u16(slot)? != (datum >> 16) as u16 {
            return Err(Refusal::Layout);
        }
        Ok(slot)
    }

    /// The object body a slot points at (`+8`).
    pub fn body(self, m: &dyn GuestMemory, datum: u32) -> Result<u32> {
        ptr(m, at(self.slot(m, datum)?, 8)?)
    }
}

pub(super) fn lights(m: &dyn GuestMemory) -> Result<Pool> {
    Pool::read(m, ptr(m, LIGHTS)?, LIGHT_BYTES)
}

pub(super) fn objects(m: &dyn GuestMemory) -> Result<Pool> {
    Pool::read(m, ptr(m, OBJECTS)?, 12)
}

/// One row of a visibility list: its four columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Row {
    pub handle: u32,
    pub flags: u16,
    pub extra: u32,
    pub mask: u16,
}

impl Row {
    pub fn plain(handle: u32) -> Self {
        Self { handle, flags: 0, extra: 0, mask: u16::MAX }
    }
}

/// A visibility list's header: capacity `+0`, count `+4` (i16), the
/// columns' pointers (flags `+8`, handles `+0xc`, extras `+0x10`, masks
/// `+0x14`).
pub(super) struct List {
    header: u32,
    pub capacity: usize,
    pub count: usize,
}

impl List {
    pub fn read(m: &dyn GuestMemory, header: u32) -> Result<Self> {
        let count = m.u16(at(header, 4)?)? as usize;
        let capacity = m.u32(header)? as usize;
        if capacity > 0x8000 || count > capacity || count >= 0x8000 {
            return Err(Refusal::Layout);
        }
        Ok(Self { header, capacity, count })
    }

    pub fn column(&self, m: &dyn GuestMemory, offset: u32) -> Result<u32> {
        ptr(m, at(self.header, offset)?)
    }

    pub fn rows(&self, m: &dyn GuestMemory) -> Result<Vec<Row>> {
        if self.count == 0 {
            return Ok(Vec::new());
        }
        let handles = m.bytes(self.column(m, 0xc)?, self.count * 4)?;
        let flags = m.bytes(self.column(m, 8)?, self.count * 2)?;
        let extras = m.bytes(self.column(m, 0x10)?, self.count * 4)?;
        let masks = m.bytes(self.column(m, 0x14)?, self.count * 2)?;
        let word = |b: &[u8], i: usize| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
        let half = |b: &[u8], i: usize| u16::from_le_bytes(b[2 * i..2 * i + 2].try_into().unwrap());
        Ok((0..self.count)
            .map(|i| Row { handle: word(&handles, i), flags: half(&flags, i), extra: word(&extras, i), mask: half(&masks, i) })
            .collect())
    }

    pub fn has_room(&self, count: usize) -> bool {
        count + 1 < self.capacity
    }

    pub fn publish(&self, m: &dyn GuestMemory, out: &mut Writes, rows: &[Row]) -> Result<()> {
        if !rows.is_empty() {
            out.array(self.column(m, 0xc)?, rows.iter().flat_map(|r| r.handle.to_le_bytes()).collect());
            out.array(self.column(m, 8)?, rows.iter().flat_map(|r| r.flags.to_le_bytes()).collect());
            out.array(self.column(m, 0x10)?, rows.iter().flat_map(|r| r.extra.to_le_bytes()).collect());
            out.array(self.column(m, 0x14)?, rows.iter().flat_map(|r| r.mask.to_le_bytes()).collect());
        }
        out.half(at(self.header, 4)?, rows.len() as u16);
        Ok(())
    }
}

/// A bounding sphere: centre and radius.
#[derive(Clone, Copy)]
pub(super) struct Sphere {
    pub centre: [f32; 3],
    pub radius: f32,
}

impl Sphere {
    pub fn read(m: &dyn GuestMemory, addr: u32) -> Result<Self> {
        Ok(Self { centre: m.vec3(addr)?, radius: m.f32(at(addr, 12)?)? })
    }
}
