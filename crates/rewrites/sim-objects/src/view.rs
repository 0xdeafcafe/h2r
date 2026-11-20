//! Typed views over the objects pool (docs/engine/sim-rewrite-objects.md
//! section 4): the pool's header, a live slot, a handle resolved to its
//! header and body, and the child links.
//!
//! Every view is addresses only and reads guest memory when asked. An
//! [`ObjRef`] is never kept across a guest call or a tick: callbacks
//! create, delete and relink objects, and the pool moves bodies when it
//! compacts. Resolve the handle again instead.

use guest_layout::object::header;
use guest_layout::{DatumArray, Datum, Fault, Mem, Object, ObjectHeader, ObjectType};

/// The objects pool as it is now: where its headers and bitmap are, and
/// the header stride. Read again per segment; the high-water mark is read
/// at each use, as the walks re-read it after every visited slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Objects {
    pub array: DatumArray,
    pub data: u32,
    pub bitmap: u32,
    pub stride: u32,
}

/// A live object: its handle, its header and its body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjRef {
    pub handle: Datum,
    pub header: ObjectHeader,
    pub body: Object,
}

impl Objects {
    /// The pool the objects global points at, `None` while it is null (no
    /// map loaded).
    pub fn read<M: Mem + ?Sized>(m: &M) -> Result<Option<Objects>, Fault> {
        let Some(addr) = guest_layout::global::OBJECTS.pointer(m)? else {
            return Ok(None);
        };
        Objects::at(m, DatumArray::at(addr)).map(Some)
    }

    /// The pool whose header is `array`.
    pub fn at<M: Mem + ?Sized>(m: &M, array: DatumArray) -> Result<Objects, Fault> {
        Ok(Objects {
            array,
            data: array.data(m)?,
            bitmap: array.bitmap(m)?,
            stride: array.element_size(m)? as u32,
        })
    }

    /// One past the highest slot in use, now.
    pub fn high_water<M: Mem + ?Sized>(&self, m: &M) -> Result<u32, Fault> {
        Ok(self.array.high_water(m)?.max(0) as u32)
    }

    /// The header of `slot`, live or not.
    pub fn header(&self, slot: u16) -> ObjectHeader {
        ObjectHeader::at(self.data.wrapping_add(u32::from(slot).wrapping_mul(self.stride)))
    }

    /// Whether `slot`'s bit is set in the bitmap.
    pub fn is_live<M: Mem + ?Sized>(&self, m: &M, slot: u16) -> Result<bool, Fault> {
        let word = self.bitmap_word(m, u32::from(slot) >> 5)?;
        Ok(word & (1 << (slot & 31)) != 0)
    }

    fn bitmap_word<M: Mem + ?Sized>(&self, m: &M, word: u32) -> Result<u32, Fault> {
        guest_layout::Off::<u32>::new(0).read(m, self.bitmap.wrapping_add(word.wrapping_mul(4)))
    }

    /// The first live slot at or after `from` and below the high-water
    /// mark. The bitmap is read one word at a time, and a word only once
    /// the previous one held no live slot at or after `from`, as the walks
    /// read it.
    pub fn next_live<M: Mem + ?Sized>(&self, m: &M, from: u32) -> Result<Option<u16>, Fault> {
        let high = self.high_water(m)?.min(1 << 16);
        let mut at = from;
        while at < high {
            let word = self.bitmap_word(m, at >> 5)? & (u32::MAX << (at & 31));
            let base = at & !31;
            if word != 0 {
                let slot = base + word.trailing_zeros();
                return Ok((slot < high).then_some(slot as u16));
            }
            at = base + 32;
        }
        Ok(None)
    }

    /// The live object `handle` names: a slot below the high-water mark,
    /// live, whose header salt is the handle's. `None` for a stale or
    /// absent handle.
    pub fn resolve<M: Mem + ?Sized>(&self, m: &M, handle: Datum) -> Result<Option<ObjRef>, Fault> {
        if handle.is_none() || u32::from(handle.index()) >= self.high_water(m)? {
            return Ok(None);
        }
        if !self.is_live(m, handle.index())? {
            return Ok(None);
        }
        let header = self.header(handle.index());
        let salt = header.salt(m)?;
        if salt == 0 || salt != handle.salt() {
            return Ok(None);
        }
        Ok(Some(ObjRef {
            handle,
            header,
            body: Object::at(header.body(m)?),
        }))
    }

    /// The live object in `slot`, with its salted handle.
    pub fn slot<M: Mem + ?Sized>(&self, m: &M, slot: u16) -> Result<Option<ObjRef>, Fault> {
        if !self.is_live(m, slot)? {
            return Ok(None);
        }
        let header = self.header(slot);
        let salt = header.salt(m)?;
        if salt == 0 {
            return Ok(None);
        }
        Ok(Some(ObjRef {
            handle: Datum(u32::from(salt) << 16 | u32::from(slot)),
            header,
            body: Object::at(header.body(m)?),
        }))
    }

    /// Every live object in slot order. The high-water mark and the
    /// bitmap are read as the iteration goes, so it sees the pool as it
    /// is at each step.
    pub fn iter<'a, M: Mem + ?Sized>(&self, m: &'a M) -> Slots<'a, M> {
        Slots { pool: *self, m, next: 0, done: false }
    }

    /// `object`'s children: its first child, then each child's next
    /// sibling. Stops at a handle that does not resolve, and after as many
    /// children as the pool has slots, so a corrupt cycle ends.
    pub fn children<'a, M: Mem + ?Sized>(&self, m: &'a M, object: ObjRef) -> Children<'a, M> {
        Children {
            pool: *self,
            m,
            next: Some(Child::First(object.body)),
            left: 1 << 16,
        }
    }
}

/// [`Objects::iter`].
pub struct Slots<'a, M: ?Sized> {
    pool: Objects,
    m: &'a M,
    next: u32,
    done: bool,
}

impl<M: Mem + ?Sized> Iterator for Slots<'_, M> {
    type Item = Result<ObjRef, Fault>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.done {
            let slot = match self.pool.next_live(self.m, self.next) {
                Ok(Some(slot)) => slot,
                Ok(None) => {
                    self.done = true;
                    return None;
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            };
            self.next = u32::from(slot) + 1;
            match self.pool.slot(self.m, slot) {
                Ok(Some(o)) => return Some(Ok(o)),
                Ok(None) => continue,
                Err(e) => {
                    self.done = true;
                    return Some(Err(e));
                }
            }
        }
        None
    }
}

enum Child {
    First(Object),
    Next(Object),
}

/// [`Objects::children`].
pub struct Children<'a, M: ?Sized> {
    pool: Objects,
    m: &'a M,
    next: Option<Child>,
    left: u32,
}

impl<M: Mem + ?Sized> Iterator for Children<'_, M> {
    type Item = Result<ObjRef, Fault>;

    fn next(&mut self) -> Option<Self::Item> {
        let link = self.next.take()?;
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        let handle = match link {
            Child::First(parent) => parent.first_child(self.m),
            Child::Next(sibling) => sibling.next_sibling(self.m),
        };
        let child = match handle.and_then(|h| self.pool.resolve(self.m, h)) {
            Ok(Some(child)) => child,
            Ok(None) => return None,
            Err(e) => return Some(Err(e)),
        };
        self.next = Some(Child::Next(child.body));
        Some(Ok(child))
    }
}

impl ObjRef {
    /// The header flags ([`header`]).
    pub fn flags<M: Mem + ?Sized>(&self, m: &M) -> Result<u8, Fault> {
        self.header.flags(m)
    }

    /// The type, `None` for a value outside the table.
    pub fn kind<M: Mem + ?Sized>(&self, m: &M) -> Result<Option<ObjectType>, Fault> {
        self.header.object_type(m)
    }

    /// ACTIVE and AWAKE: what the pre-update walks select (with NEW clear).
    pub fn is_awake<M: Mem + ?Sized>(&self, m: &M) -> Result<bool, Fault> {
        let f = self.flags(m)?;
        Ok(f & (header::ACTIVE | header::AWAKE) == header::ACTIVE | header::AWAKE)
    }
}

#[cfg(test)]
mod tests;
