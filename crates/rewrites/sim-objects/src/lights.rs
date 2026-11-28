//! The lights datum array as a Rust pool (docs/engine/sim-world.md "Store
//! slots"; contracts: docs/engine/object-update.md, "Slice 5 bodies"), and
//! the tick's light walk `c1400` and the expiry `c3260` / unlink `c3220`
//! over it. The global `[4e030c]` points at retail's array of `0x110`-byte
//! elements; this table holds the same elements, [`LightTable::scatter`]
//! writes them back for the guest readers that remain, and
//! [`LightTable::import`] reads them in.
//!
//! The walk's callees that are not light data stay behind [`LightHost`]:
//! the record update `c0c80`, the link `c2d00` and the list removal
//! `1cae40`. They read and write elements, so each is handed the table.
//!
//! | Function | Rust |
//! |---|---|
//! | `c1400` | [`walk`] |
//! | `c3260` | [`expire`] |
//! | `c3220` | [`unlink`] |
//! | `16ba40` on the array | [`LightTable::delete`] |

use guest_layout::{DatumArray, Fault, Mem, MemMut, Off};
use sim_world::{Fnv64, Handle, Pool, Reader, Record, RestoreError, StaleHandle, Writer, kind};

use crate::table::{ImportError, ObjectTable};

/// Slots in retail's lights array.
pub const LIGHT_CAPACITY: u32 = 0x80;
/// Bytes per element.
pub const STRIDE: u32 = 0x110;
/// The section kind this table is saved under.
pub const KIND: u16 = kind::LIGHTS;
/// The "none" handle.
const NONE: u32 = u32::MAX;

/// Element offsets (`lights/mod.rs` in the engine: flags `+02`).
pub mod field {
    pub const FLAGS: usize = 0x02;
    pub const DEFINITION: usize = 0x04;
    /// The list record `1cae40` and `1cac60` keep.
    pub const LIST: usize = 0x10;
    pub const CREATED: usize = 0x14;
    pub const OWNER: usize = 0x4c;
    pub const PARENT: usize = 0x58;
}

/// Flag bits at [`field::FLAGS`].
pub mod flag {
    pub const ACTIVE: u8 = 2;
    pub const LINKED: u8 = 8;
    pub const TIMED: u8 = 0x10;
}

/// An object header's POST flag (`guest_layout::object::header`, bit 2).
const OBJECT_POST: u8 = 4;
/// The bit of a marker parent's body flags (`+4`) that says a light is
/// still attached to it.
const PARENT_LIGHT: u32 = 0x40;
/// Offset of the lifetime in a light definition (seconds, `f32`).
pub const DEFINITION_LIFETIME: u32 = 0xb0;

/// One light element, `0x110` bytes with the salt word (`+0`) left to the
/// pool: bytes `0..2` are zero here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightRecord {
    pub bytes: Vec<u8>,
}

impl Default for LightRecord {
    fn default() -> Self {
        Self { bytes: vec![0; STRIDE as usize] }
    }
}

impl LightRecord {
    pub fn u8(&self, at: usize) -> u8 {
        self.bytes[at]
    }
    pub fn u16(&self, at: usize) -> u16 {
        u16::from_le_bytes(self.bytes[at..at + 2].try_into().expect("two bytes"))
    }
    pub fn u32(&self, at: usize) -> u32 {
        u32::from_le_bytes(self.bytes[at..at + 4].try_into().expect("four bytes"))
    }
    pub fn set_u32(&mut self, at: usize, v: u32) {
        self.bytes[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    pub fn flags(&self) -> u8 {
        self.bytes[field::FLAGS]
    }
    pub fn set_flags(&mut self, v: u8) {
        self.bytes[field::FLAGS] = v;
    }
}

impl Record for LightRecord {
    fn write(&self, w: &mut Writer) {
        w.bytes(&self.bytes[2..]);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, RestoreError> {
        let mut bytes = vec![0u8; 2];
        bytes.extend_from_slice(r.take(STRIDE as usize - 2)?);
        Ok(Self { bytes })
    }
}

/// The lights pool.
pub struct LightTable {
    pool: Pool<LightRecord>,
    name: u16,
}

impl Default for LightTable {
    fn default() -> Self {
        Self::new(LIGHT_CAPACITY, 0)
    }
}

impl LightTable {
    pub const VERSION: u16 = 1;

    /// An empty table of `capacity` slots whose first salt is
    /// `name | 0x8000`. A table read from the guest takes `name` from the
    /// array header's name string (the first two bytes, as
    /// `data_initialize` ORs them into `next_salt`).
    pub fn new(capacity: u32, name: u16) -> Self {
        Self { pool: Pool::new(capacity, name), name }
    }

    pub fn capacity(&self) -> u32 {
        self.pool.capacity()
    }
    pub fn len(&self) -> u32 {
        self.pool.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }
    pub fn high_water(&self) -> u32 {
        self.pool.high_water()
    }
    pub fn free_scan_start(&self) -> u32 {
        self.pool.free_scan_start()
    }
    pub fn next_salt(&self) -> u16 {
        self.pool.next_salt()
    }

    /// `datum_new`.
    pub fn alloc(&mut self, record: LightRecord) -> Option<Handle> {
        self.pool.alloc(record)
    }
    /// `datum_delete` (`16ba40`).
    pub fn delete(&mut self, h: Handle) -> Result<LightRecord, StaleHandle> {
        self.pool.free(h)
    }
    pub fn get(&self, h: Handle) -> Option<&LightRecord> {
        self.pool.get(h)
    }
    pub fn get_mut(&mut self, h: Handle) -> Option<&mut LightRecord> {
        self.pool.get_mut(h)
    }
    pub fn handle_at(&self, slot: u16) -> Option<Handle> {
        self.pool.handle_at(slot)
    }
    pub fn iter(&self) -> impl Iterator<Item = (Handle, &LightRecord)> {
        self.pool.iter()
    }

    /// The first live light at or after slot `from` (`16bc00` and the
    /// walk's inline bitmap scan), reading the high water as it stands.
    pub fn next_live(&self, from: u32) -> Option<Handle> {
        (from..self.high_water()).find_map(|slot| self.handle_at(slot as u16))
    }

    pub fn encode(&self) -> Vec<u8> {
        self.pool.encode()
    }
    pub fn section_hash(&self) -> u64 {
        self.pool.section_hash()
    }
    pub fn record_hash(r: &LightRecord) -> u64 {
        let mut w = Writer::new();
        r.write(&mut w);
        Fnv64::of(&w.finish())
    }
    pub fn decode(&self, version: u16, body: &[u8]) -> Result<Self, RestoreError> {
        Ok(Self {
            pool: Pool::decode(KIND, self.pool.capacity(), self.name, version, body)?,
            name: self.name,
        })
    }

    /// The table the guest's lights array holds now (`global::LIGHTS`),
    /// with its allocation state. `None` while no map is loaded.
    pub fn import<M: Mem + ?Sized>(m: &M) -> Result<Option<Self>, ImportError> {
        let Some(addr) = guest_layout::global::LIGHTS.pointer(m)? else {
            return Ok(None);
        };
        let array = DatumArray::at(addr);
        let capacity = array.maximum_count(m)?.max(0) as u32;
        let high = array.high_water(m)?.max(0) as u32;
        let scan = Off::<i32>::new(0x34).read(m, addr)?.max(0) as u32;
        let next_salt = Off::<u16>::new(0x40).read(m, addr)?;
        let name = Off::<u16>::new(0).read(m, addr)?;
        let (data, bitmap) = (array.data(m)?, array.bitmap(m)?);
        if array.element_size(m)? as u32 != STRIDE {
            return Err(ImportError::Corrupt("element size is not a light's"));
        }
        let mut parts = Vec::with_capacity(high as usize);
        for slot in 0..high {
            let at = data.wrapping_add(slot * STRIDE);
            let word = Off::<u32>::new(0).read(m, bitmap.wrapping_add((slot >> 5) * 4))?;
            let salt = Off::<u16>::new(0).read(m, at)?;
            if salt == 0 || word & (1 << (slot & 31)) == 0 {
                parts.push((0, None));
                continue;
            }
            let mut bytes = vec![0u8; STRIDE as usize];
            m.read_bytes(at, &mut bytes)?;
            bytes[..2].fill(0);
            parts.push((salt, Some(LightRecord { bytes })));
        }
        let pool = Pool::from_parts(capacity, next_salt, scan, parts).map_err(ImportError::Corrupt)?;
        Ok(Some(Self { pool, name }))
    }

    /// Write the pool to the guest's lights array: the allocation state,
    /// every element and bitmap word up to the larger of the old and new
    /// high water (a freed slot's salt and bit clear). The array's storage
    /// (`data`, `bitmap`, `maximum_count`) is the guest's and stays.
    pub fn scatter<M: MemMut + ?Sized>(&self, m: &mut M) -> Result<(), Fault> {
        let Some(addr) = guest_layout::global::LIGHTS.pointer(&*m)? else {
            return Ok(());
        };
        let array = DatumArray::at(addr);
        let (data, bitmap) = (array.data(&*m)?, array.bitmap(&*m)?);
        let span = (array.high_water(&*m)?.max(0) as u32).max(self.high_water());
        let mut elements = vec![0u8; (span * STRIDE) as usize];
        let mut words = vec![0u32; span.div_ceil(32) as usize];
        for (h, r) in self.pool.iter() {
            let at = (u32::from(h.index()) * STRIDE) as usize;
            elements[at..at + STRIDE as usize].copy_from_slice(&r.bytes);
            elements[at..at + 2].copy_from_slice(&h.salt().to_le_bytes());
            words[usize::from(h.index()) >> 5] |= 1 << (h.index() & 31);
        }
        m.write_bytes(data, &elements)?;
        m.write_bytes(bitmap, &words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>())?;
        let put = |m: &mut M, at: u32, v: &[u8]| m.write_bytes(addr.wrapping_add(at), v);
        put(m, 0x34, &(self.free_scan_start() as i32).to_le_bytes())?;
        put(m, 0x38, &(self.high_water() as i32).to_le_bytes())?;
        put(m, 0x3c, &(self.len() as i32).to_le_bytes())?;
        put(m, 0x40, &self.next_salt().to_le_bytes())
    }
}

/// The walk's callees that are not light data, and the reads it makes of
/// the rest of the game. Each light callee gets the table.
pub trait LightHost {
    /// The record update `c0c80(handle)`.
    fn record_update(&mut self, lights: &mut LightTable, light: Handle);
    /// The link `c2d00(handle)`.
    fn link(&mut self, lights: &mut LightTable, light: Handle);
    /// The list removal `1cae40(handle, element +10)`.
    fn list_remove(&mut self, light: Handle, record: u32);
    /// The light definition's lifetime word (`f32` bits at `+b0`) for the
    /// tag datum at element `+04`.
    fn lifetime(&self, definition: u32) -> u32;
}

/// The game clock a walk reads once at entry: game time `+8` (the tick)
/// and `+2` (ticks per second, a signed 16 bit word).
#[derive(Clone, Copy, Debug)]
pub struct Clock {
    pub tick: i32,
    pub ticks_per_second: i16,
}

/// `c3220 (light: eax)`: an active light is removed from the lists, then
/// loses its linked bit.
pub fn unlink(lights: &mut LightTable, host: &mut dyn LightHost, light: Handle) {
    let Some(r) = lights.get_mut(light) else { return };
    if r.flags() & flag::ACTIVE == 0 {
        return;
    }
    let record = r.u32(field::LIST);
    host.list_remove(light, record);
    let r = lights.get_mut(light).expect("a host callee frees no light");
    r.set_flags(r.flags() & !flag::LINKED);
}

/// `c3260 (light, notify)`: a linked, active light leaves the lists first.
/// With a marker parent and `notify`, the parent's body flags (`+4`) bit 6
/// is set when another live light has the same parent and cleared when
/// none has. Then the light is deleted.
pub fn expire(
    lights: &mut LightTable,
    objects: &mut ObjectTable,
    host: &mut dyn LightHost,
    light: Handle,
    notify: bool,
) {
    let Some(r) = lights.get(light) else { return };
    let (flags, record, parent) = (r.flags(), r.u32(field::LIST), r.u32(field::PARENT));
    if flags & flag::LINKED != 0 && flags & flag::ACTIVE != 0 {
        host.list_remove(light, record);
        let r = lights.get_mut(light).expect("a host callee frees no light");
        r.set_flags(r.flags() & !flag::LINKED);
    }
    if parent != NONE && notify {
        let shared = lights
            .iter()
            .any(|(h, r)| h != light && r.u32(field::PARENT) == parent);
        let slot = (parent & 0xffff) as u16;
        let object = objects
            .handle_at(slot)
            .unwrap_or_else(|| unimplemented!("lights: expire: marker parent {parent:#x} is no live object"));
        let body = &mut objects.get_mut(object).expect("live").body;
        if body.len() < 8 {
            unimplemented!("lights: expire: marker parent {parent:#x} has no body flags");
        }
        let flags = u32::from_le_bytes(body[4..8].try_into().expect("four bytes"));
        let flags = if shared { flags | PARENT_LIGHT } else { flags & !PARENT_LIGHT };
        body[4..8].copy_from_slice(&flags.to_le_bytes());
    }
    lights.delete(light).expect("a host callee frees no light");
}

/// `c1400`: every live light, in slot order, the next slot read from the
/// table as it stands after each light (so a light a callee created or
/// deleted is seen). Per light:
///
/// 1. the record update;
/// 2. a linked light whose owner, else marker parent, is a live object
///    with POST set: an active one leaves the lists (and loses the linked
///    bit), then the light is linked again;
/// 3. a timed light older than ticks-per-second times its definition's
///    lifetime expires with `notify` set.
///
/// The age and the limit are `f32` (`cvtsi2ss`, `mulss`, `comiss`); an
/// unordered compare does not expire.
pub fn walk(
    lights: &mut LightTable,
    objects: &mut ObjectTable,
    host: &mut dyn LightHost,
    clock: Clock,
) {
    let mut next = lights.next_live(0);
    while let Some(light) = next {
        let slot = u32::from(light.index());
        let definition = lights.get(light).expect("live").u32(field::DEFINITION);
        host.record_update(lights, light);
        // The callee may have deleted the light; retail reads the element
        // in place, a freed element keeping its bytes. The Rust pool has
        // none, so a light the update deleted ends its own turn.
        if let Some(r) = lights.get(light) {
            let (flags, owner, parent) = (r.flags(), r.u32(field::OWNER), r.u32(field::PARENT));
            if flags & flag::LINKED != 0 {
                let anchor = if owner != NONE { owner } else { parent };
                if anchor != NONE && is_post(&*objects, anchor) {
                    if flags & flag::ACTIVE != 0 {
                        let record = r.u32(field::LIST);
                        host.list_remove(light, record);
                        if let Some(r) = lights.get_mut(light) {
                            r.set_flags(r.flags() & !flag::LINKED);
                        }
                    }
                    host.link(lights, light);
                }
            }
            if let Some(r) = lights.get(light) {
                if r.flags() & flag::TIMED != 0 {
                    let age = clock.tick.wrapping_sub(r.u32(field::CREATED) as i32) as f32;
                    let life = f32::from_bits(host.lifetime(definition));
                    let limit = f32::from(clock.ticks_per_second) * life;
                    if age > limit {
                        expire(lights, objects, host, light, true);
                    }
                }
            }
        }
        next = lights.next_live(slot + 1);
    }
}

fn is_post(objects: &ObjectTable, handle: u32) -> bool {
    objects
        .handle_at((handle & 0xffff) as u16)
        .and_then(|h| objects.get(h))
        .is_some_and(|r| r.flags & OBJECT_POST != 0)
}

pub mod record;

#[cfg(test)]
mod tests;
