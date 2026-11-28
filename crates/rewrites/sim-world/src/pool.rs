//! `Pool<T>`: retail's datum array (docs/engine/datum_new.md) as a typed
//! slot pool. Handles are `salt << 16 | index`; a slot is live when its
//! salt is non-zero; allocation scans from `free_scan_start`, then takes
//! `high_water`, exactly as `datum_new` does, so handles and iteration order
//! match retail's.

use std::cell::Cell;

use crate::hash::Fnv64;
use crate::save::{Reader, RestoreError, Writer};

/// A record a pool stores. `write` and `read` carry simulation state only;
/// derived state is rebuilt by `read` and never written.
pub trait Record: Sized {
    fn write(&self, w: &mut Writer);
    fn read(r: &mut Reader<'_>) -> Result<Self, RestoreError>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Handle(u32);

impl Handle {
    /// `0xffffffff`, retail's "none".
    pub const NONE: Handle = Handle(u32::MAX);

    pub fn new(salt: u16, index: u16) -> Self {
        Self((u32::from(salt) << 16) | u32::from(index))
    }
    pub fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    pub fn raw(self) -> u32 {
        self.0
    }
    pub fn salt(self) -> u16 {
        (self.0 >> 16) as u16
    }
    pub fn index(self) -> u16 {
        self.0 as u16
    }
}

/// A handle whose slot is free, past the pool's high water, or holds
/// another salt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StaleHandle(pub Handle);

struct Slot<T> {
    salt: u16,
    value: Option<T>,
}

pub struct Pool<T> {
    capacity: u32,
    /// Slots `0..high_water`; free slots carry salt 0.
    slots: Vec<Slot<T>>,
    free_scan_start: u32,
    next_salt: u16,
    active: u32,
    /// Section hash, cleared by every `&mut` access.
    hash: Cell<Option<u64>>,
}

const SALT_FLOOR: u16 = 0x8000;
const SALT_END: u16 = 0xffff;

impl<T> Pool<T> {
    /// An empty pool of `capacity` slots whose first salt is
    /// `name | 0x8000`. A seed of `0xffff` is moved to `0x8000`: the
    /// next salt would wrap to 0, which marks a free slot.
    pub fn new(capacity: u32, name: u16) -> Self {
        let seed = name | SALT_FLOOR;
        Self {
            capacity,
            slots: Vec::new(),
            free_scan_start: 0,
            next_salt: if seed == SALT_END { SALT_FLOOR } else { seed },
            active: 0,
            hash: Cell::new(None),
        }
    }

    pub fn capacity(&self) -> u32 {
        self.capacity
    }
    pub fn len(&self) -> u32 {
        self.active
    }
    pub fn is_empty(&self) -> bool {
        self.active == 0
    }
    /// One past the highest live slot.
    pub fn high_water(&self) -> u32 {
        self.slots.len() as u32
    }

    /// Take a slot as `datum_new` does. `None` when the pool is full.
    pub fn alloc(&mut self, value: T) -> Option<Handle> {
        self.hash.set(None);
        let high = self.slots.len() as u32;
        let mut i = self.free_scan_start;
        let slot = loop {
            if i >= high {
                if high >= self.capacity {
                    return None;
                }
                break high;
            }
            if self.slots[i as usize].salt == 0 {
                break i;
            }
            i += 1;
        };
        let salt = self.next_salt;
        self.next_salt = salt.wrapping_add(1);
        if self.next_salt == SALT_END {
            self.next_salt = SALT_FLOOR;
        }
        if slot == high {
            self.slots.push(Slot { salt, value: Some(value) });
        } else {
            self.slots[slot as usize] = Slot { salt, value: Some(value) };
        }
        self.active += 1;
        self.free_scan_start = slot + 1;
        Some(Handle::new(salt, slot as u16))
    }

    /// Free a slot as `datum_delete` does, but refusing a handle whose salt
    /// does not match (retail frees whatever is in the slot).
    pub fn free(&mut self, h: Handle) -> Result<T, StaleHandle> {
        self.live(h).ok_or(StaleHandle(h))?;
        self.hash.set(None);
        let i = u32::from(h.index());
        let slot = &mut self.slots[i as usize];
        slot.salt = 0;
        let value = slot.value.take().ok_or(StaleHandle(h))?;
        self.free_scan_start = self.free_scan_start.min(i);
        while self.slots.last().is_some_and(|s| s.salt == 0) {
            self.slots.pop();
        }
        self.active -= 1;
        Ok(value)
    }

    fn live(&self, h: Handle) -> Option<usize> {
        let i = usize::from(h.index());
        let s = self.slots.get(i)?;
        (s.salt != 0 && s.salt == h.salt() && s.value.is_some()).then_some(i)
    }

    pub fn get(&self, h: Handle) -> Option<&T> {
        self.slots[self.live(h)?].value.as_ref()
    }

    /// Borrowing mutably clears the cached section hash.
    pub fn get_mut(&mut self, h: Handle) -> Option<&mut T> {
        let i = self.live(h)?;
        self.hash.set(None);
        self.slots[i].value.as_mut()
    }

    /// The handle of a live slot by index, for walks that hold an absolute
    /// index.
    pub fn handle_at(&self, index: u16) -> Option<Handle> {
        let s = self.slots.get(usize::from(index))?;
        (s.salt != 0).then(|| Handle::new(s.salt, index))
    }

    /// Live records in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (Handle, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, s)| {
            s.value.as_ref().map(|v| (Handle::new(s.salt, i as u16), v))
        })
    }

    /// The salt the next `alloc` takes (the header's `next_salt`).
    pub fn next_salt(&self) -> u16 {
        self.next_salt
    }

    /// Where `alloc` starts its scan (the header's `free_scan_start`).
    pub fn free_scan_start(&self) -> u32 {
        self.free_scan_start
    }

    /// A pool in the state `parts` describes: the slots below the high
    /// water as `(salt, record)`, a free slot being `(0, None)`. For a
    /// pool read out of retail's datum array, whose allocation state
    /// (`next_salt`, `free_scan_start`) is part of the truth.
    pub fn from_parts(
        capacity: u32,
        next_salt: u16,
        free_scan_start: u32,
        parts: Vec<(u16, Option<T>)>,
    ) -> Result<Self, &'static str> {
        if next_salt < SALT_FLOOR || next_salt == SALT_END {
            return Err("next salt out of range");
        }
        if parts.len() as u32 > capacity || free_scan_start as usize > parts.len() {
            return Err("counts exceed the pool");
        }
        if parts.iter().any(|(salt, v)| (*salt == 0) != v.is_none()) {
            return Err("a slot's salt and record disagree");
        }
        if parts.last().is_some_and(|(salt, _)| *salt == 0) {
            return Err("high water sits on a free slot");
        }
        if parts[..free_scan_start as usize].iter().any(|(salt, _)| *salt == 0) {
            return Err("free slot below the scan start");
        }
        let active = parts.iter().filter(|(salt, _)| *salt != 0).count() as u32;
        Ok(Self {
            capacity,
            slots: parts.into_iter().map(|(salt, value)| Slot { salt, value }).collect(),
            free_scan_start,
            next_salt,
            active,
            hash: Cell::new(None),
        })
    }

    /// Live handles in slot order; collect to mutate while walking.
    pub fn handles(&self) -> Vec<Handle> {
        self.iter().map(|(h, _)| h).collect()
    }
}

impl<T: Record> Pool<T> {
    /// Section version. One record layout per build, so a constant.
    pub fn version(&self) -> u16 {
        1
    }

    /// Header (`capacity, next_salt, free_scan_start, high_water,
    /// active`), then per slot below high water its salt and, when live,
    /// its record. Free slots keep their place: allocation order is state.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32(self.capacity);
        w.u16(self.next_salt);
        w.u32(self.free_scan_start);
        w.u32(self.slots.len() as u32);
        w.u32(self.active);
        for s in &self.slots {
            w.u16(s.salt);
            if let Some(v) = &s.value {
                v.write(&mut w);
            }
        }
        w.finish()
    }

    /// The fnv64 of `encode`, cached until the next `&mut` access.
    pub fn section_hash(&self) -> u64 {
        if let Some(h) = self.hash.get() {
            return h;
        }
        let h = Fnv64::of(&self.encode());
        self.hash.set(Some(h));
        h
    }

    pub fn decode(
        kind: u16,
        capacity: u32,
        name: u16,
        version: u16,
        body: &[u8],
    ) -> Result<Self, RestoreError> {
        let bad = |what| RestoreError::Corrupt { kind, what };
        let fresh = Self::new(capacity, name);
        if version != fresh.version() {
            return Err(RestoreError::SectionVersion { kind, version });
        }
        let mut r = Reader::new(body);
        if r.u32()? != capacity {
            return Err(bad("capacity differs from this build"));
        }
        let next_salt = r.u16()?;
        let free_scan_start = r.u32()?;
        let high = r.u32()?;
        let active = r.u32()?;
        if next_salt < SALT_FLOOR || next_salt == SALT_END {
            return Err(bad("next salt out of range"));
        }
        if high > capacity || free_scan_start > high || active > high {
            return Err(bad("counts exceed the pool"));
        }
        let mut slots = Vec::with_capacity(high as usize);
        let mut live = 0;
        for _ in 0..high {
            let salt = r.u16()?;
            let value = if salt == 0 {
                None
            } else {
                live += 1;
                Some(T::read(&mut r)?)
            };
            slots.push(Slot { salt, value });
        }
        if live != active {
            return Err(bad("active count disagrees with live slots"));
        }
        if slots.last().is_some_and(|s| s.salt == 0) {
            return Err(bad("high water sits on a free slot"));
        }
        if slots[..free_scan_start as usize].iter().any(|s| s.salt == 0) {
            return Err(bad("free slot below the scan start"));
        }
        r.end(kind)?;
        Ok(Self {
            capacity,
            slots,
            free_scan_start,
            next_salt,
            active,
            hash: Cell::new(None),
        })
    }
}
