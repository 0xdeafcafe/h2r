//! Synthetic object pools in guest form, for this crate's tests and the
//! readers built on it (`sim-state`). No game data: slots, salts, flags,
//! links and definition tags are whatever a test puts there.

use guest_layout::object::header;
use guest_layout::{DatumArray, Datum, FlatRam, MemMut, Object, ObjectDefinition, ObjectHeader, ObjectType, TagEntry, TagIndex};

/// Where the synthetic memory starts: below every global the views read.
pub const BASE: u32 = 0x004e_0000;
/// The pool's datum-array header.
pub const POOL: u32 = 0x0050_0000;
pub const BITMAP: u32 = 0x0050_0100;
pub const DATA: u32 = 0x0050_1000;
/// Bodies, one per slot, [`BODY_SIZE`] apart.
pub const BODIES: u32 = 0x0051_0000;
pub const BODY_SIZE: u32 = 0x400;
/// The tag index, its entries and definition data.
pub const TAG_INDEX: u32 = 0x0053_ff00;
pub const TAG_ENTRIES: u32 = 0x0054_0000;
pub const TAG_DATA: u32 = 0x0055_0000;
pub const TAG_SIZE: u32 = 0x100;
const LEN: usize = 0x0008_0000;
/// The most slots a synthetic pool has.
pub const CAPACITY: u16 = 128;
/// The most definition tags.
pub const TAGS: u16 = 64;

/// A pool of up to [`CAPACITY`] objects and [`TAGS`] definition tags.
pub struct Pool {
    pub ram: FlatRam,
}

impl Default for Pool {
    fn default() -> Self {
        Pool::new()
    }
}

impl Pool {
    /// An empty pool, the objects global pointing at it, and an empty tag
    /// index.
    pub fn new() -> Pool {
        let mut ram = FlatRam::new(BASE, LEN);
        let w = |ram: &mut FlatRam, a: u32, v: u32| ram.write_bytes(a, &v.to_le_bytes()).unwrap();
        w(&mut ram, guest_layout::global::OBJECTS.base, POOL);
        w(&mut ram, guest_layout::global::TAG_ENTRIES.base, TAG_ENTRIES);
        w(&mut ram, guest_layout::global::TAG_INDEX.base, TAG_INDEX);
        let a = DatumArray::at(POOL);
        a.set(&mut ram, DatumArray::MAXIMUM_COUNT, i32::from(CAPACITY)).unwrap();
        a.set(&mut ram, DatumArray::ELEMENT_SIZE, ObjectHeader::SIZE as i32).unwrap();
        a.set(&mut ram, DatumArray::DATA, DATA).unwrap();
        a.set(&mut ram, DatumArray::BITMAP, BITMAP).unwrap();
        let mut pool = Pool { ram };
        pool.set_tag_count(i32::from(TAGS));
        pool
    }

    fn array(&self) -> DatumArray {
        DatumArray::at(POOL)
    }

    pub fn header(&self, slot: u16) -> ObjectHeader {
        ObjectHeader::at(DATA + u32::from(slot) * ObjectHeader::SIZE)
    }

    pub fn body(&self, slot: u16) -> Object {
        Object::at(BODIES + u32::from(slot) * BODY_SIZE)
    }

    pub fn set_high_water(&mut self, high: i32) {
        self.array().set(&mut self.ram, DatumArray::HIGH_WATER, high).unwrap();
    }

    fn set_bit(&mut self, slot: u16, on: bool) {
        let a = BITMAP + 4 * (u32::from(slot) >> 5);
        let mut b = [0; 4];
        guest_layout::Mem::read_bytes(&self.ram, a, &mut b).unwrap();
        let mut word = u32::from_le_bytes(b);
        let bit = 1 << (slot & 31);
        word = if on { word | bit } else { word & !bit };
        self.ram.write_bytes(a, &word.to_le_bytes()).unwrap();
    }

    /// A live object in `slot`: salt, type and header flags as given, a
    /// body with no links, no definition, unit scale. Raises the
    /// high-water mark past it.
    pub fn add(&mut self, slot: u16, salt: u16, kind: ObjectType, flags: u8) -> Datum {
        assert!(slot < CAPACITY && salt != 0);
        let h = self.header(slot);
        let b = self.body(slot);
        let r = &mut self.ram;
        h.set(r, ObjectHeader::SALT, salt).unwrap();
        h.set(r, ObjectHeader::FLAGS, flags).unwrap();
        h.set(r, ObjectHeader::KIND, kind as u8).unwrap();
        h.set(r, ObjectHeader::BODY, BODIES + u32::from(slot) * BODY_SIZE).unwrap();
        for link in [Object::NEXT_SIBLING, Object::FIRST_CHILD, Object::PARENT, Object::DEFINITION] {
            b.set(r, link, Datum::NONE).unwrap();
        }
        b.set(r, Object::SCALE, 1.0).unwrap();
        b.set(r, Object::NODES_OFFSET, -1).unwrap();
        self.set_bit(slot, true);
        let high = self.array().high_water(&self.ram).unwrap();
        if i32::from(slot) >= high {
            self.set_high_water(i32::from(slot) + 1);
        }
        Datum(u32::from(salt) << 16 | u32::from(slot))
    }

    /// Clear `slot`'s bit and salt, as a delete does (the high-water mark
    /// stays).
    pub fn free(&mut self, slot: u16) {
        self.set_bit(slot, false);
        self.header(slot).set(&mut self.ram, ObjectHeader::SALT, 0).unwrap();
    }

    /// Attach `child` under `parent` as its last child, and mark it CHILD.
    pub fn attach(&mut self, child: Datum, parent: Datum) {
        let r = &mut self.ram;
        let body = |d: Datum| Object::at(BODIES + u32::from(d.index()) * BODY_SIZE);
        body(child).set(r, Object::PARENT, parent).unwrap();
        let mut last = None;
        let mut next = body(parent).first_child(&*r).unwrap();
        while !next.is_none() {
            last = Some(next);
            next = body(next).next_sibling(&*r).unwrap();
        }
        match last {
            None => body(parent).set(r, Object::FIRST_CHILD, child).unwrap(),
            Some(l) => body(l).set(r, Object::NEXT_SIBLING, child).unwrap(),
        }
        let h = self.header(child.index());
        let f = h.flags(&self.ram).unwrap();
        h.set(&mut self.ram, ObjectHeader::FLAGS, f | header::CHILD).unwrap();
    }

    /// The tag index's slot count.
    pub fn set_tag_count(&mut self, count: i32) {
        TagIndex::at(TAG_INDEX).set(&mut self.ram, TagIndex::TAG_COUNT, count).unwrap();
    }

    /// A definition tag in slot `index` with salt `salt`: an entry whose
    /// datum is the tag's and whose data is a zeroed block with the given
    /// bounds. Returns the tag datum and the data's view.
    pub fn tag(&mut self, index: u16, salt: u16, radius: f32, offset: [f32; 3]) -> (Datum, ObjectDefinition) {
        assert!(index < TAGS);
        let datum = Datum(u32::from(salt) << 16 | u32::from(index));
        let entry = TagEntry::at(TAG_ENTRIES + TagEntry::SIZE * u32::from(index));
        let data = TAG_DATA + TAG_SIZE * u32::from(index);
        entry.set(&mut self.ram, TagEntry::DATUM, datum).unwrap();
        entry.set(&mut self.ram, TagEntry::ADDRESS, data).unwrap();
        let d = ObjectDefinition::at(data);
        d.set(&mut self.ram, ObjectDefinition::BOUNDING_RADIUS, radius).unwrap();
        d.set(&mut self.ram, ObjectDefinition::BOUNDING_OFFSET, offset).unwrap();
        (datum, d)
    }
}
