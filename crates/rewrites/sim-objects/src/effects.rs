//! The effects datum array as a Rust pool (docs/engine/sim-world.md "Store
//! slots"). The global `[4ea93c]` points at retail's array of `0x190`-byte
//! elements (`docs/engine/effects-parts.md`); this table holds the same
//! elements, [`EffectTable::scatter`] writes them back for the guest
//! readers that remain, and [`EffectTable::import`] reads them in. The
//! element's fields stay bytes until the effect update `1792b0` is ported
//! over [`EffectRecord`] (flags `+02`, clock `+60`, event end `+64`,
//! lifetime `+fc`).

use guest_layout::{DatumArray, Fault, Mem, MemMut, Off};
use sim_world::{Fnv64, Handle, Pool, Reader, Record, RestoreError, StaleHandle, Writer, kind};

use crate::table::ImportError;

/// Slots in retail's effects array.
pub const EFFECT_CAPACITY: u32 = 0x100;
/// Bytes per element.
pub const STRIDE: u32 = 0x190;
/// The section kind this table is saved under.
pub const KIND: u16 = kind::EFFECTS;

/// One effect element, `0x190` bytes with the salt word (`+0`) left to the
/// pool: bytes `0..2` are zero here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectRecord {
    pub bytes: Vec<u8>,
}

impl Default for EffectRecord {
    fn default() -> Self {
        Self { bytes: vec![0; STRIDE as usize] }
    }
}

impl EffectRecord {
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
}

impl Record for EffectRecord {
    fn write(&self, w: &mut Writer) {
        w.bytes(&self.bytes[2..]);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, RestoreError> {
        let mut bytes = vec![0u8; 2];
        bytes.extend_from_slice(r.take(STRIDE as usize - 2)?);
        Ok(Self { bytes })
    }
}

/// The effects pool.
pub struct EffectTable {
    pool: Pool<EffectRecord>,
    name: u16,
}

impl Default for EffectTable {
    fn default() -> Self {
        Self::new(EFFECT_CAPACITY, 0)
    }
}

impl EffectTable {
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
    pub fn alloc(&mut self, record: EffectRecord) -> Option<Handle> {
        self.pool.alloc(record)
    }
    /// `datum_delete` (`16ba40`).
    pub fn delete(&mut self, h: Handle) -> Result<EffectRecord, StaleHandle> {
        self.pool.free(h)
    }
    pub fn get(&self, h: Handle) -> Option<&EffectRecord> {
        self.pool.get(h)
    }
    pub fn get_mut(&mut self, h: Handle) -> Option<&mut EffectRecord> {
        self.pool.get_mut(h)
    }
    pub fn handle_at(&self, slot: u16) -> Option<Handle> {
        self.pool.handle_at(slot)
    }
    pub fn iter(&self) -> impl Iterator<Item = (Handle, &EffectRecord)> {
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
    pub fn record_hash(r: &EffectRecord) -> u64 {
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

    /// The table the guest's effects array holds now (`global::LIGHTS`),
    /// with its allocation state. `None` while no map is loaded.
    pub fn import<M: Mem + ?Sized>(m: &M) -> Result<Option<Self>, ImportError> {
        let Some(addr) = pointer(m)? else {
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
            return Err(ImportError::Corrupt("element size is not a effect's"));
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
            parts.push((salt, Some(EffectRecord { bytes })));
        }
        let pool = Pool::from_parts(capacity, next_salt, scan, parts).map_err(ImportError::Corrupt)?;
        Ok(Some(Self { pool, name }))
    }

    /// Write the pool to the guest's effects array: the allocation state,
    /// every element and bitmap word up to the larger of the old and new
    /// high water (a freed slot's salt and bit clear). The array's storage
    /// (`data`, `bitmap`, `maximum_count`) is the guest's and stays.
    pub fn scatter<M: MemMut + ?Sized>(&self, m: &mut M) -> Result<(), Fault> {
        let Some(addr) = pointer(&*m)? else {
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

/// The effects array's pointer.
pub const ARRAY: u32 = 0x004e_a93c;

fn pointer<M: Mem + ?Sized>(m: &M) -> Result<Option<u32>, Fault> {
    let at = Off::<u32>::new(0).read(m, ARRAY)?;
    Ok((at != 0).then_some(at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::FlatRam;

    const BASE: u32 = 0x1000_0000;
    const HEADER: u32 = BASE + 0x1000;
    const DATA: u32 = BASE + 0x2000;
    const BITMAP: u32 = BASE + 0x1800;

    /// A guest array of 3 slots: slot 0 and 2 live, slot 1 free.
    fn guest() -> FlatRam {
        let mut ram = FlatRam::new(BASE, 0x10_0000);
        let w = |ram: &mut FlatRam, at: u32, v: &[u8]| ram.write_bytes(at, v).unwrap();
        w(&mut ram, ARRAY, &HEADER.to_le_bytes());
        w(&mut ram, HEADER, &0x6566u16.to_le_bytes());
        w(&mut ram, HEADER + 0x20, &(EFFECT_CAPACITY as i32).to_le_bytes());
        w(&mut ram, HEADER + 0x24, &(STRIDE as i32).to_le_bytes());
        w(&mut ram, HEADER + 0x34, &1i32.to_le_bytes());
        w(&mut ram, HEADER + 0x38, &3i32.to_le_bytes());
        w(&mut ram, HEADER + 0x3c, &2i32.to_le_bytes());
        w(&mut ram, HEADER + 0x40, &0xe572u16.to_le_bytes());
        w(&mut ram, HEADER + 0x44, &DATA.to_le_bytes());
        w(&mut ram, HEADER + 0x48, &BITMAP.to_le_bytes());
        w(&mut ram, BITMAP, &0b101u32.to_le_bytes());
        for (slot, salt) in [(0u32, 0xe566u16), (2, 0xe568)] {
            let at = DATA + slot * STRIDE;
            w(&mut ram, at, &salt.to_le_bytes());
            w(&mut ram, at + 0xfc, &u32::MAX.to_le_bytes());
        }
        ram
    }

    #[test]
    fn import_reads_the_array_and_scatter_writes_it_back_bit_for_bit() {
        let mut ram = guest();
        let t = EffectTable::import(&ram).unwrap().unwrap();
        assert_eq!((t.len(), t.high_water(), t.free_scan_start(), t.next_salt()), (2, 3, 1, 0xe572));
        assert_eq!(t.handle_at(2).unwrap().raw(), 0xe568_0002);
        assert_eq!(t.get(t.handle_at(0).unwrap()).unwrap().u32(0xfc), u32::MAX);
        let mut before = vec![0u8; 3 * STRIDE as usize];
        ram.read_bytes(DATA, &mut before).unwrap();
        t.scatter(&mut ram).unwrap();
        let mut after = vec![0u8; 3 * STRIDE as usize];
        ram.read_bytes(DATA, &mut after).unwrap();
        assert_eq!(before, after);
        assert_eq!(EffectTable::import(&ram).unwrap().unwrap().section_hash(), t.section_hash());
    }

    #[test]
    fn no_array_pointer_is_no_table_and_a_delete_scatters_as_free() {
        let empty = FlatRam::new(BASE, 0x1000);
        assert!(EffectTable::import(&empty).unwrap().is_none());
        let mut ram = guest();
        let mut t = EffectTable::import(&ram).unwrap().unwrap();
        let h = t.handle_at(2).unwrap();
        t.delete(h).unwrap();
        t.scatter(&mut ram).unwrap();
        let re = EffectTable::import(&ram).unwrap().unwrap();
        assert_eq!((re.len(), re.high_water()), (1, 1));
    }

    #[test]
    fn the_section_round_trips() {
        let t = EffectTable::import(&guest()).unwrap().unwrap();
        let back = t.decode(EffectTable::VERSION, &t.encode()).unwrap();
        assert_eq!(back.section_hash(), t.section_hash());
    }
}
