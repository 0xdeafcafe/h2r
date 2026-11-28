//! The object table as a Rust pool (docs/engine/sim-world.md "Store
//! slots", docs/engine/sim-rewrite-objects.md section 4): retail's objects
//! datum array (`datum_new` 0x16b840, `datum_delete` 0x16ba40) held as a
//! [`sim_world::Pool`] of header and body records. The pool is the truth;
//! [`ObjectTable::scatter`] writes it to the guest's arena for the guest
//! readers that remain, and [`PoolMem`] shows it to the typed views
//! (`view::Objects`, `sim-state`) as guest memory, so a reader names no
//! different type.
//!
//! Slot allocation is retail's: `Pool::alloc` scans from `free_scan_start`
//! to the high water, salts come from the array's `next_salt` and the high
//! water walks down past free slots on a delete. The save keeps every
//! free slot in place, so the next handle after a restore is the next
//! handle retail would hand out.

use guest_layout::object::ObjectHeader;
use guest_layout::{DatumArray, Fault, Mem, MemMut, Off};
use sim_world::{Fnv64, Handle, Pool, Reader, Record, RestoreError, StaleHandle, Writer, kind};

/// Slots in retail's objects array; an import takes the guest array's own
/// `maximum_count` instead.
pub const OBJECT_CAPACITY: u32 = 2048;
/// The array's name seed: `"ob"` of `"object"`, as `data_new` stores it
/// (`name[0..2] | 0x8000` is the first salt).
pub const OBJECT_NAME: u16 = 0x6f62;
/// The section kind this table is saved under.
pub const KIND: u16 = kind::OBJECTS;

/// Bytes per header: [`ObjectHeader::SIZE`].
const STRIDE: u32 = ObjectHeader::SIZE;

/// One object: its header (`ObjectHeader`) and its body bytes, which sit
/// in guest memory at `body_addr`. The address is state: the guest's
/// object memory pool places bodies in allocation order (`bf380` moves
/// them), and a header that names a body names that place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ObjectRecord {
    /// Header flags (`guest_layout::object::header`).
    pub flags: u8,
    /// The concrete type byte.
    pub kind: u8,
    pub cluster: u16,
    /// The body's size, as the header stores it.
    pub body_size: u16,
    /// Where the body lives in guest memory; 0 while it has none.
    pub body_addr: u32,
    pub body: Vec<u8>,
}

impl Record for ObjectRecord {
    fn write(&self, w: &mut Writer) {
        w.u8(self.flags);
        w.u8(self.kind);
        w.u16(self.cluster);
        w.u16(self.body_size);
        w.u32(self.body_addr);
        w.u32(self.body.len() as u32);
        w.bytes(&self.body);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, RestoreError> {
        let (flags, kind) = (r.u8()?, r.u8()?);
        let (cluster, body_size, body_addr) = (r.u16()?, r.u16()?, r.u32()?);
        let len = r.u32()? as usize;
        Ok(Self { flags, kind, cluster, body_size, body_addr, body: r.take(len)?.to_vec() })
    }
}

/// Why a guest array could not be read in.
#[derive(Debug, PartialEq, Eq)]
pub enum ImportError {
    Fault(Fault),
    /// The array's own state disagrees with itself.
    Corrupt(&'static str),
}

impl From<Fault> for ImportError {
    fn from(f: Fault) -> Self {
        ImportError::Fault(f)
    }
}

/// The objects pool.
pub struct ObjectTable {
    pool: Pool<ObjectRecord>,
    name: u16,
}

impl Default for ObjectTable {
    fn default() -> Self {
        Self::new(OBJECT_CAPACITY, OBJECT_NAME)
    }
}

impl ObjectTable {
    /// The section version.
    pub const VERSION: u16 = 1;

    /// An empty table of `capacity` slots whose first salt is
    /// `name | 0x8000`.
    pub fn new(capacity: u32, name: u16) -> Self {
        Self { pool: Pool::new(capacity, name), name }
    }

    pub fn capacity(&self) -> u32 {
        self.pool.capacity()
    }
    /// Live objects (`active_count`).
    pub fn len(&self) -> u32 {
        self.pool.len()
    }
    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }
    /// One past the highest live slot.
    pub fn high_water(&self) -> u32 {
        self.pool.high_water()
    }
    pub fn free_scan_start(&self) -> u32 {
        self.pool.free_scan_start()
    }
    pub fn next_salt(&self) -> u16 {
        self.pool.next_salt()
    }

    /// `datum_new`: the first free slot at or after the scan start, else
    /// the high water. `None` when the array is full.
    pub fn alloc(&mut self, record: ObjectRecord) -> Option<Handle> {
        self.pool.alloc(record)
    }

    /// `datum_delete`, refusing a handle whose salt is not the slot's.
    pub fn delete(&mut self, h: Handle) -> Result<ObjectRecord, StaleHandle> {
        self.pool.free(h)
    }

    pub fn get(&self, h: Handle) -> Option<&ObjectRecord> {
        self.pool.get(h)
    }

    /// Borrowing mutably clears the cached section hash.
    pub fn get_mut(&mut self, h: Handle) -> Option<&mut ObjectRecord> {
        self.pool.get_mut(h)
    }

    /// The handle of the live object in `slot`.
    pub fn handle_at(&self, slot: u16) -> Option<Handle> {
        self.pool.handle_at(slot)
    }

    /// Live objects in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (Handle, &ObjectRecord)> {
        self.pool.iter()
    }

    /// Replace `h`'s header flags (the walks' ACTIVE, AWAKE, POST, NEW and
    /// DELETE bits). `false` for a stale handle.
    pub fn set_flags(&mut self, h: Handle, flags: u8) -> bool {
        self.pool.get_mut(h).map(|r| r.flags = flags).is_some()
    }

    /// The canonical section bytes: the pool's, so free slots keep their
    /// place.
    pub fn encode(&self) -> Vec<u8> {
        self.pool.encode()
    }

    pub fn section_hash(&self) -> u64 {
        self.pool.section_hash()
    }

    /// Fnv64 of one object's saved record, for a per-object compare.
    pub fn record_hash(r: &ObjectRecord) -> u64 {
        let mut w = Writer::new();
        r.write(&mut w);
        Fnv64::of(&w.finish())
    }

    /// A saved section checked and decoded, this table's capacity and name
    /// being the build's.
    pub fn decode(&self, version: u16, body: &[u8]) -> Result<Self, RestoreError> {
        Ok(Self {
            pool: Pool::decode(KIND, self.pool.capacity(), self.name, version, body)?,
            name: self.name,
        })
    }

    /// The table the guest's objects array holds now (`global::OBJECTS`):
    /// the allocation state (`next_salt`, `free_scan_start`), every live
    /// slot's header and its body bytes. A slot is live when its bitmap bit
    /// and its salt agree. `None` while no map is loaded.
    pub fn import<M: Mem + ?Sized>(m: &M) -> Result<Option<Self>, ImportError> {
        let Some(addr) = guest_layout::global::OBJECTS.pointer(m)? else {
            return Ok(None);
        };
        let array = DatumArray::at(addr);
        let capacity = array.maximum_count(m)?.max(0) as u32;
        let high = array.high_water(m)?.max(0) as u32;
        let scan = Off::<i32>::new(0x34).read(m, addr)?.max(0) as u32;
        let next_salt = Off::<u16>::new(0x40).read(m, addr)?;
        let (data, bitmap) = (array.data(m)?, array.bitmap(m)?);
        let stride = array.element_size(m)? as u32;
        if stride != STRIDE {
            return Err(ImportError::Corrupt("element size is not a header's"));
        }
        let mut parts = Vec::with_capacity(high as usize);
        for slot in 0..high {
            let header = ObjectHeader::at(data.wrapping_add(slot * stride));
            let word = Off::<u32>::new(0).read(m, bitmap.wrapping_add((slot >> 5) * 4))?;
            let salt = header.salt(m)?;
            if salt == 0 || word & (1 << (slot & 31)) == 0 {
                parts.push((0, None));
                continue;
            }
            let (body_addr, body_size) = (header.body(m)?, header.body_size(m)?);
            let mut body = vec![0u8; if body_addr == 0 { 0 } else { usize::from(body_size) }];
            m.read_bytes(body_addr, &mut body)?;
            parts.push((
                salt,
                Some(ObjectRecord {
                    flags: header.flags(m)?,
                    kind: header.kind(m)?,
                    cluster: header.cluster(m)?,
                    body_size,
                    body_addr,
                    body,
                }),
            ));
        }
        let pool = Pool::from_parts(capacity, next_salt, scan, parts).map_err(ImportError::Corrupt)?;
        Ok(Some(Self { pool, name: OBJECT_NAME }))
    }

    /// The array's header fields, as retail stores them: `maximum_count`,
    /// element size, `free_scan_start`, `high_water`, `active_count`,
    /// `next_salt`, then the data and bitmap pointers given.
    fn array_image(&self, data: u32, bitmap: u32) -> [u8; 0x4c] {
        let mut a = [0u8; 0x4c];
        let put = |a: &mut [u8; 0x4c], at: usize, v: &[u8]| a[at..at + v.len()].copy_from_slice(v);
        put(&mut a, 0x20, &(self.capacity() as i32).to_le_bytes());
        put(&mut a, 0x24, &(STRIDE as i32).to_le_bytes());
        a[0x29] = 1;
        put(&mut a, 0x2c, &0x6440_7440u32.to_le_bytes());
        put(&mut a, 0x34, &(self.free_scan_start() as i32).to_le_bytes());
        put(&mut a, 0x38, &(self.high_water() as i32).to_le_bytes());
        put(&mut a, 0x3c, &(self.len() as i32).to_le_bytes());
        put(&mut a, 0x40, &self.next_salt().to_le_bytes());
        put(&mut a, 0x44, &data.to_le_bytes());
        put(&mut a, 0x48, &bitmap.to_le_bytes());
        a
    }

    /// `slots` headers, free slots zero.
    fn headers_image(&self, slots: u32) -> Vec<u8> {
        let mut out = vec![0u8; (slots * STRIDE) as usize];
        for (h, r) in self.pool.iter() {
            let at = (u32::from(h.index()) * STRIDE) as usize;
            if at + STRIDE as usize > out.len() {
                continue;
            }
            out[at..at + 2].copy_from_slice(&h.salt().to_le_bytes());
            out[at + 2] = r.flags;
            out[at + 3] = r.kind;
            out[at + 4..at + 6].copy_from_slice(&r.cluster.to_le_bytes());
            out[at + 6..at + 8].copy_from_slice(&r.body_size.to_le_bytes());
            out[at + 8..at + 12].copy_from_slice(&r.body_addr.to_le_bytes());
        }
        out
    }

    /// The occupancy bitmap over `slots` slots, in 32-bit words.
    fn bitmap_image(&self, slots: u32) -> Vec<u8> {
        let mut words = vec![0u32; slots.div_ceil(32) as usize];
        for (h, _) in self.pool.iter() {
            if let Some(w) = words.get_mut(usize::from(h.index()) >> 5) {
                *w |= 1 << (h.index() & 31);
            }
        }
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// Write the pool to the guest's objects array and bodies: the array's
    /// allocation state, every header and bitmap word up to the larger of
    /// the old and new high water (so a freed slot's salt and bit clear),
    /// and each live body at its address. The array's own storage
    /// (`data`, `bitmap`, `maximum_count`) is the guest's and stays.
    pub fn scatter<M: MemMut + ?Sized>(&self, m: &mut M) -> Result<(), Fault> {
        let Some(addr) = guest_layout::global::OBJECTS.pointer(&*m)? else {
            return Ok(());
        };
        let array = DatumArray::at(addr);
        let (data, bitmap) = (array.data(&*m)?, array.bitmap(&*m)?);
        let old = array.high_water(&*m)?.max(0) as u32;
        let span = old.max(self.high_water());
        m.write_bytes(data, &self.headers_image(span))?;
        m.write_bytes(bitmap, &self.bitmap_image(span))?;
        let image = self.array_image(data, bitmap);
        for at in [0x34usize, 0x38, 0x3c, 0x40] {
            let len = if at == 0x40 { 2 } else { 4 };
            m.write_bytes(addr.wrapping_add(at as u32), &image[at..at + len])?;
        }
        for (_, r) in self.pool.iter() {
            if r.body_addr != 0 {
                m.write_bytes(r.body_addr, &r.body)?;
            }
        }
        Ok(())
    }
}

/// Where the table's array header appears in [`PoolMem`]: addresses past
/// the guest's 64 MB, so they never collide with a body.
pub const VIRT_ARRAY: u32 = 0x7f00_0000;
const VIRT_BITMAP: u32 = 0x7f08_0000;
const VIRT_DATA: u32 = 0x7f10_0000;

/// The table seen as guest memory: the objects global, the array header,
/// the headers, the bitmap and the bodies (at their real addresses), and
/// nothing else. `view::Objects` and `sim-state` read it as they read the
/// arena. It is a snapshot of the table's shape taken when built, so build
/// one per read burst and keep none across a tick.
pub struct PoolMem<'a> {
    table: &'a ObjectTable,
    array: [u8; 0x4c],
    headers: Vec<u8>,
    bitmap: Vec<u8>,
    /// `(body_addr, end, slot handle)` sorted by address.
    bodies: Vec<(u32, u32, Handle)>,
}

impl<'a> PoolMem<'a> {
    pub fn new(table: &'a ObjectTable) -> Self {
        let high = table.high_water();
        let mut bodies: Vec<_> = table
            .iter()
            .filter(|(_, r)| r.body_addr != 0 && !r.body.is_empty())
            .map(|(h, r)| (r.body_addr, r.body_addr.wrapping_add(r.body.len() as u32), h))
            .collect();
        bodies.sort_by_key(|b| b.0);
        Self {
            table,
            array: table.array_image(VIRT_DATA, VIRT_BITMAP),
            headers: table.headers_image(high),
            bitmap: table.bitmap_image(high),
            bodies,
        }
    }

    fn region(&self, addr: u32, len: usize) -> Option<&[u8]> {
        fn within(addr: u32, len: usize, base: u32, bytes: &[u8]) -> Option<&[u8]> {
            let at = addr.checked_sub(base)? as usize;
            bytes.get(at..at.checked_add(len)?)
        }
        within(addr, len, VIRT_ARRAY, &self.array)
            .or_else(|| within(addr, len, VIRT_BITMAP, &self.bitmap))
            .or_else(|| within(addr, len, VIRT_DATA, &self.headers))
    }
}

impl Mem for PoolMem<'_> {
    fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> Result<(), Fault> {
        let fault = Fault { addr, len: buf.len() as u32, write: false };
        if addr == guest_layout::global::OBJECTS.base && buf.len() == 4 {
            buf.copy_from_slice(&VIRT_ARRAY.to_le_bytes());
            return Ok(());
        }
        if let Some(bytes) = self.region(addr, buf.len()) {
            buf.copy_from_slice(bytes);
            return Ok(());
        }
        let i = self.bodies.partition_point(|b| b.0 <= addr).checked_sub(1).ok_or(fault)?;
        let (start, end, h) = self.bodies[i];
        let at = (addr - start) as usize;
        let body = &self.table.get(h).ok_or(fault)?.body;
        if addr >= end || at + buf.len() > body.len() {
            return Err(fault);
        }
        buf.copy_from_slice(&body[at..at + buf.len()]);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
