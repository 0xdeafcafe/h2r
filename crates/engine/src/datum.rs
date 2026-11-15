//! Datum arrays: the engine's fixed-capacity, salted object tables
//! (`0x16b570`-`0x16bd10`). See `docs/engine/datum_new.md`.
//!
//! A datum array is a `0x4c`-byte header followed (usually) by element
//! storage and an occupancy bitmap. Objects, players, script threads, UI
//! lists and ~250 other call sites allocate from these, and many more read
//! the header and elements inline, so the guest layout is the contract:
//! [`DatumArray`] is a view over a guest address, not a Rust-owned copy.
//!
//! Every method mirrors one guest function, including its quirks (they are
//! listed on each method). Reads and writes go to the same fields in the same
//! order as the original wherever that is observable; the element-zeroing and
//! bitmap-clearing loops are byte fills of the same ranges.

use crate::handle::DatumHandle;
use crate::memory::{GuestMemory, MemFault, strncpy};

type R<T> = Result<T, MemFault>;

/// Header field offsets. Names are ours; see the doc for the evidence.
pub mod layout {
    /// `char[32]`, NUL-terminated by the initializer at `+0x1f`.
    pub const NAME: u32 = 0x00;
    /// `i32`: element capacity.
    pub const MAXIMUM_COUNT: u32 = 0x20;
    /// `i32`: bytes per element. Element `i` lives at `data + i * size`.
    pub const ELEMENT_SIZE: u32 = 0x24;
    /// `u8`: log2 alignment of the data pointer (contiguous construction).
    pub const ALIGNMENT_BITS: u32 = 0x28;
    /// `u8`: 1 once the array has been made valid (cleared or attached).
    pub const VALID: u32 = 0x29;
    /// `u8`: see [`super::flags`].
    pub const FLAGS: u32 = 0x2a;
    /// `u32`: [`super::SIGNATURE`].
    pub const SIGNATURE: u32 = 0x2c;
    /// `u32`: guest allocator object (vtable `+0` alloc, `+4` free), or 0.
    pub const ALLOCATOR: u32 = 0x30;
    /// `i32`: where `datum_new` starts scanning for a free slot. Lowered by
    /// deletes, set to `slot + 1` by `datum_new`.
    pub const FREE_SCAN_START: u32 = 0x34;
    /// `i32`: one past the highest slot in use. Iteration and
    /// `datum_try_and_get` stop here, not at the capacity.
    pub const HIGH_WATER: u32 = 0x38;
    /// `i32`: number of live elements.
    pub const ACTIVE_COUNT: u32 = 0x3c;
    /// `u16`: salt the next allocation receives.
    pub const NEXT_SALT: u32 = 0x40;
    /// `u32`: element storage.
    pub const DATA: u32 = 0x44;
    /// `u32`: occupancy bitmap, one bit per slot, 32-bit words.
    pub const BITMAP: u32 = 0x48;
    /// Header size; initializers clear exactly this many bytes.
    pub const HEADER_SIZE: u32 = 0x4c;
}

/// Bits of [`layout::FLAGS`].
pub mod flags {
    /// Set together with bit 1 by `data_initialize`; cleared by
    /// `data_initialize_contiguous`. Meaning not recovered.
    pub const BIT0: u8 = 1 << 0;
    /// Set by `data_initialize`; cleared by contiguous construction and by
    /// `data_attach`. Meaning not recovered.
    pub const BIT1: u8 = 1 << 1;
    /// Set after every contiguous construction (`data_new` and the 43
    /// inlined copies of its tail). Assigned name.
    pub const CONTIGUOUS: u8 = 1 << 2;
    /// When set, freed elements are filled with `0xBA` before their salt is
    /// cleared. Nothing in the retail image sets it.
    pub const DEBUG_FILL: u8 = 1 << 3;
}

/// `'d@t@'` as a little-endian u32 (bytes `40 74 40 64`).
pub const SIGNATURE: u32 = 0x6440_7440;

/// Salt allocation wraps from `0xffff` back to this value.
pub const SALT_WRAP: u16 = 0x8000;

/// Fill pattern for freed elements under [`flags::DEBUG_FILL`].
pub const DEBUG_FILL_BYTE: u8 = 0xba;

/// Bytes `data_new` (`0x16b570`) asks its allocator for.
pub fn allocation_size(maximum_count: i32, element_size: i32, alignment_bits: u8) -> u32 {
    let elements = (element_size as u32).wrapping_mul(maximum_count as u32);
    elements
        .wrapping_add(bitmap_bytes(maximum_count))
        .wrapping_add(1u32 << (alignment_bits & 31))
        .wrapping_add(0x4b)
}

/// Bitmap size the engine clears: `((count + 31) >> 5) * 4`, arithmetic shift.
pub fn bitmap_bytes(maximum_count: i32) -> u32 {
    ((maximum_count.wrapping_add(31) >> 5) as u32).wrapping_mul(4)
}

/// Address of the bitmap word holding `slot`'s bit, and the bit's mask.
fn bit_of(bitmap: u32, slot: i32) -> (u32, u32) {
    (
        bitmap.wrapping_add(((slot >> 5) as u32).wrapping_mul(4)),
        1u32 << (slot & 31),
    )
}

/// A datum-array header at a guest address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatumArray {
    pub addr: u32,
}

/// A snapshot of every header field, for tests and diagnostics. Engine
/// methods read fields individually, as the guest does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub name: [u8; 32],
    pub maximum_count: i32,
    pub element_size: i32,
    pub alignment_bits: u8,
    pub valid: bool,
    pub flags: u8,
    pub signature: u32,
    pub allocator: u32,
    pub free_scan_start: i32,
    pub high_water: i32,
    pub active_count: i32,
    pub next_salt: u16,
    pub data: u32,
    pub bitmap: u32,
}

impl DatumArray {
    pub const fn at(addr: u32) -> Self {
        DatumArray { addr }
    }

    fn f(&self, off: u32) -> u32 {
        self.addr.wrapping_add(off)
    }
    fn get(&self, m: &dyn GuestMemory, off: u32) -> R<i32> {
        m.i32(self.f(off))
    }
    fn put(&self, m: &mut dyn GuestMemory, off: u32, v: i32) -> R<()> {
        m.set_i32(self.f(off), v)
    }

    pub fn header(&self, m: &dyn GuestMemory) -> R<Header> {
        let mut name = [0; 32];
        m.read(self.addr, &mut name)?;
        Ok(Header {
            name,
            maximum_count: self.get(m, layout::MAXIMUM_COUNT)?,
            element_size: self.get(m, layout::ELEMENT_SIZE)?,
            alignment_bits: m.u8(self.f(layout::ALIGNMENT_BITS))?,
            valid: m.u8(self.f(layout::VALID))? != 0,
            flags: m.u8(self.f(layout::FLAGS))?,
            signature: m.u32(self.f(layout::SIGNATURE))?,
            allocator: m.u32(self.f(layout::ALLOCATOR))?,
            free_scan_start: self.get(m, layout::FREE_SCAN_START)?,
            high_water: self.get(m, layout::HIGH_WATER)?,
            active_count: self.get(m, layout::ACTIVE_COUNT)?,
            next_salt: m.u16(self.f(layout::NEXT_SALT))?,
            data: m.u32(self.f(layout::DATA))?,
            bitmap: m.u32(self.f(layout::BITMAP))?,
        })
    }

    /// Address of element `index`: `size * index + data`, 32-bit wrapping.
    pub fn element_addr(&self, m: &dyn GuestMemory, index: i32) -> R<u32> {
        let size = self.get(m, layout::ELEMENT_SIZE)?;
        let data = m.u32(self.f(layout::DATA))?;
        Ok((size.wrapping_mul(index) as u32).wrapping_add(data))
    }

    /// The salt word at the start of an element; zero means the slot is free.
    pub fn salt_at(&self, m: &dyn GuestMemory, index: i32) -> R<u16> {
        m.u16(self.element_addr(m, index)?)
    }

    // ---- construction ----------------------------------------------------

    /// `data_initialize` (`0x16b650`): write a fresh header with no data
    /// pointer. Clears the header, copies the name (`strncpy`, 32 bytes,
    /// forced NUL at `+0x1f`), sets flag bits 0 and 1, `valid = 0`, and stores
    /// capacity, element size, alignment, signature, bitmap and allocator.
    /// The element storage and bitmap are not touched.
    #[allow(clippy::too_many_arguments)]
    pub fn initialize(
        &self,
        m: &mut dyn GuestMemory,
        name: u32,
        maximum_count: i32,
        element_size: i32,
        alignment_bits: u8,
        allocator: u32,
        bitmap: u32,
    ) -> R<()> {
        let a = [
            name,
            maximum_count as u32,
            element_size as u32,
            alignment_bits as u32,
            allocator,
            bitmap,
        ];
        self.initialize_with(m, InitArgs::Values(a))
    }

    /// [`initialize`](Self::initialize) with its six arguments still in
    /// guest memory, where the original takes them: dwords at `args`,
    /// `args + 4`, ... (entry ESP + 4). Each is read at the point the
    /// original reads it, so an unmapped argument faults after the same
    /// stores. For example, the header is already cleared when the name
    /// read faults.
    pub fn initialize_from_stack(&self, m: &mut dyn GuestMemory, args: u32) -> R<()> {
        self.initialize_with(m, InitArgs::Stack(args))
    }

    /// The store and argument-read order of `0x16b650`.
    fn initialize_with(&self, m: &mut dyn GuestMemory, a: InitArgs) -> R<()> {
        m.fill(self.addr, layout::HEADER_SIZE, 0)?;
        let name = a.dword(m, 0)?;
        strncpy(m, self.addr, name, 0x20)?;
        let maximum_count = a.dword(m, 1)?;
        let element_size = a.dword(m, 2)?;
        let fl = m.u8(self.f(layout::FLAGS))?;
        m.set_u8(self.f(layout::FLAGS), fl | flags::BIT0 | flags::BIT1)?;
        m.set_u8(self.f(0x1f), 0)?;
        m.set_u32(self.f(layout::MAXIMUM_COUNT), maximum_count)?;
        let alignment_bits = a.byte(m, 3)?;
        m.set_u32(self.f(layout::ELEMENT_SIZE), element_size)?;
        let bitmap = a.dword(m, 5)?;
        m.set_u32(self.f(layout::DATA), 0)?;
        m.set_u8(self.f(layout::VALID), 0)?;
        let allocator = a.dword(m, 4)?;
        m.set_u8(self.f(layout::ALIGNMENT_BITS), alignment_bits)?;
        m.set_u32(self.f(layout::SIGNATURE), SIGNATURE)?;
        m.set_u32(self.f(layout::BITMAP), bitmap)?;
        m.set_u32(self.f(layout::ALLOCATOR), allocator)
    }

    /// `data_initialize_contiguous` (`0x16b5f0`): lay out header, aligned
    /// element storage and bitmap in one buffer starting at `self.addr`.
    ///
    /// `data = align_up(addr + 0x4c, 1 << bits)`, `bitmap = data + count *
    /// size`. The alignment shift uses only the low five bits of
    /// `alignment_bits`, like `shl eax, cl`; the full byte is stored. After
    /// [`initialize`](Self::initialize) it clears flag bits 0 and 1, stores
    /// the data pointer and zeroes the bitmap. It does not set
    /// [`flags::CONTIGUOUS`] itself: every caller does that inline.
    pub fn initialize_contiguous(
        &self,
        m: &mut dyn GuestMemory,
        name: u32,
        maximum_count: i32,
        element_size: i32,
        alignment_bits: u8,
        allocator: u32,
    ) -> R<()> {
        let mask = (1u32 << (alignment_bits & 31)).wrapping_sub(1);
        let data = self
            .addr
            .wrapping_add(layout::HEADER_SIZE)
            .wrapping_add(mask)
            & !mask;
        let bitmap = (maximum_count.wrapping_mul(element_size) as u32).wrapping_add(data);
        self.initialize(
            m,
            name,
            maximum_count,
            element_size,
            alignment_bits,
            allocator,
            bitmap,
        )?;
        let fl = m.u8(self.f(layout::FLAGS))?;
        m.set_u8(self.f(layout::FLAGS), fl & !(flags::BIT0 | flags::BIT1))?;
        m.set_u32(self.f(layout::DATA), data)?;
        let bitmap = m.u32(self.f(layout::BITMAP))?;
        m.fill(bitmap, bitmap_bytes(maximum_count), 0)
    }

    /// Seed [`layout::NEXT_SALT`] from the first two name bytes (`strncpy`
    /// of 2) with the top bit set. An empty name seeds `0x8000`.
    fn seed_salt(&self, m: &mut dyn GuestMemory) -> R<()> {
        strncpy(m, self.f(layout::NEXT_SALT), self.addr, 2)?;
        let s = m.u16(self.f(layout::NEXT_SALT))?;
        m.set_u16(self.f(layout::NEXT_SALT), s | 0x8000)
    }

    /// `data_delete_all` (`0x16b7a0`): free every slot. Resets the scan
    /// start, high-water mark and live count to zero, re-seeds the salt
    /// from the name (so salts repeat across clears of the same array),
    /// optionally debug-fills all storage, zeroes every element's salt word
    /// up to the capacity, then clears the bitmap.
    pub fn delete_all(&self, m: &mut dyn GuestMemory) -> R<()> {
        self.put(m, layout::HIGH_WATER, 0)?;
        self.put(m, layout::ACTIVE_COUNT, 0)?;
        self.put(m, layout::FREE_SCAN_START, 0)?;
        self.seed_salt(m)?;
        if m.u8(self.f(layout::FLAGS))? & flags::DEBUG_FILL != 0 {
            let size = self.get(m, layout::ELEMENT_SIZE)?;
            let n = size.wrapping_mul(self.get(m, layout::MAXIMUM_COUNT)?) as u32;
            let data = m.u32(self.f(layout::DATA))?;
            m.fill(data, n, DEBUG_FILL_BYTE)?;
        }
        let mut i = 0i32;
        while i < self.get(m, layout::MAXIMUM_COUNT)? {
            let e = self.element_addr(m, i)?;
            m.set_u16(e, 0)?;
            i += 1;
        }
        let count = self.get(m, layout::MAXIMUM_COUNT)?;
        let bitmap = m.u32(self.f(layout::BITMAP))?;
        m.fill(bitmap, bitmap_bytes(count), 0)
    }

    /// `data_make_valid` (`0x16b790`): set `valid = 1`, then
    /// [`delete_all`](Self::delete_all).
    pub fn make_valid(&self, m: &mut dyn GuestMemory) -> R<()> {
        m.set_u8(self.f(layout::VALID), 1)?;
        self.delete_all(m)
    }

    /// `data_attach` (`0x16b6b0`, assigned name): point the array at existing
    /// element storage and rebuild its bookkeeping from the salts found
    /// there. Used once, by a loader of saved state.
    ///
    /// A slot is live iff its salt word is non-zero. Live slots get their
    /// bit set, raise the high-water mark and the live count; free slots get
    /// their bit cleared (and are debug-filled under [`flags::DEBUG_FILL`])
    /// and lower the scan start, which begins at the capacity. The salt is
    /// re-seeded from the name, so new allocations can collide with salts
    /// already in the storage.
    pub fn attach(&self, m: &mut dyn GuestMemory, maximum_count: i32, data: u32) -> R<()> {
        let fl = m.u8(self.f(layout::FLAGS))?;
        m.set_u8(self.f(layout::FLAGS), fl & !flags::BIT1)?;
        self.put(m, layout::MAXIMUM_COUNT, maximum_count)?;
        m.set_u32(self.f(layout::DATA), data)?;
        m.set_u8(self.f(layout::VALID), 1)?;
        self.put(m, layout::HIGH_WATER, 0)?;
        self.put(m, layout::ACTIVE_COUNT, 0)?;
        self.put(m, layout::FREE_SCAN_START, maximum_count)?;
        self.seed_salt(m)?;
        let mut i = 0i32;
        if self.get(m, layout::MAXIMUM_COUNT)? <= 0 {
            return Ok(());
        }
        loop {
            let size = self.get(m, layout::ELEMENT_SIZE)?;
            let e = self.element_addr(m, i)?;
            let used = m.u16(e)? != 0;
            let debug = m.u8(self.f(layout::FLAGS))? & flags::DEBUG_FILL != 0;
            let (word, bit) = bit_of(m.u32(self.f(layout::BITMAP))?, i);
            if used {
                let w = m.u32(word)?;
                m.set_u32(word, w | bit)?;
                self.put(m, layout::HIGH_WATER, i + 1)?;
                let n = self.get(m, layout::ACTIVE_COUNT)?;
                self.put(m, layout::ACTIVE_COUNT, n.wrapping_add(1))?;
            } else {
                if debug {
                    m.fill(e, size as u32, DEBUG_FILL_BYTE)?;
                    m.set_u16(e, 0)?;
                }
                // The bitmap pointer is re-read after the fill.
                let (word, bit) = bit_of(m.u32(self.f(layout::BITMAP))?, i);
                let w = m.u32(word)?;
                m.set_u32(word, w & !bit)?;
                if i < self.get(m, layout::FREE_SCAN_START)? {
                    self.put(m, layout::FREE_SCAN_START, i)?;
                }
            }
            i += 1;
            if i >= self.get(m, layout::MAXIMUM_COUNT)? {
                return Ok(());
            }
        }
    }

    // ---- allocation ------------------------------------------------------

    /// Mark `slot` live and update the counters, as all three allocators do.
    /// `datum_new` additionally moves the scan start; the `at` variants
    /// don't.
    fn claim(&self, m: &mut dyn GuestMemory, slot: i32, move_scan_start: bool) -> R<u32> {
        let elem = self.element_addr(m, slot)?;
        let (word, bit) = bit_of(m.u32(self.f(layout::BITMAP))?, slot);
        let w = m.u32(word)?;
        m.set_u32(word, w | bit)?;
        let live = self.get(m, layout::ACTIVE_COUNT)?.wrapping_add(1);
        let hw = self.get(m, layout::HIGH_WATER)?;
        self.put(m, layout::ACTIVE_COUNT, live)?;
        if move_scan_start {
            self.put(m, layout::FREE_SCAN_START, slot.wrapping_add(1))?;
        }
        if slot >= hw {
            self.put(m, layout::HIGH_WATER, slot.wrapping_add(1))?;
        }
        Ok(elem)
    }

    /// `datum_initialize` (`0x16ba00`; called by the `_at` allocators,
    /// inlined into `datum_new`): zero the element at `elem` (any address;
    /// it is not derived from an index) and give it the next salt. It does
    /// not touch the bitmap or counters. Salts count up and skip from
    /// `0xfffe` to [`SALT_WRAP`], so `0xffff` is only ever assigned if the
    /// name seed itself was `0xffff`.
    pub fn initialize_datum(&self, m: &mut dyn GuestMemory, elem: u32) -> R<()> {
        let size = self.get(m, layout::ELEMENT_SIZE)?;
        m.fill(elem, size as u32, 0)?;
        let salt = m.u16(self.f(layout::NEXT_SALT))?;
        m.set_u16(elem, salt)?;
        m.set_u16(self.f(layout::NEXT_SALT), salt.wrapping_add(1))?;
        if m.u16(self.f(layout::NEXT_SALT))? == 0xffff {
            m.set_u16(self.f(layout::NEXT_SALT), SALT_WRAP)?;
        }
        Ok(())
    }

    /// `datum_new` (`0x16b840`): allocate the lowest free slot at or above
    /// the scan start, zero it and salt it.
    ///
    /// Slots below the high-water mark are found through the bitmap; if none
    /// is free the high-water slot itself is taken, unless the array is at
    /// capacity (`None`, guest `-1`). That slot's bitmap bit is not checked.
    pub fn new_datum(&self, m: &mut dyn GuestMemory) -> R<Option<DatumHandle>> {
        let mut i = self.get(m, layout::FREE_SCAN_START)?;
        let hw = self.get(m, layout::HIGH_WATER)?;
        let mut slot = None;
        if i < hw {
            let bitmap = m.u32(self.f(layout::BITMAP))?;
            loop {
                let (word, bit) = bit_of(bitmap, i);
                if m.u32(word)? & bit == 0 {
                    // The guest re-checks for -1 here; only a corrupt
                    // negative scan start can reach it.
                    if i != -1 {
                        slot = Some(i);
                    }
                    break;
                }
                i += 1;
                if i >= self.get(m, layout::HIGH_WATER)? {
                    break;
                }
            }
        }
        let slot = match slot {
            Some(s) => s,
            None => {
                if hw >= self.get(m, layout::MAXIMUM_COUNT)? || hw == -1 {
                    return Ok(None);
                }
                hw
            }
        };
        let elem = self.claim(m, slot, true)?;
        self.initialize_datum(m, elem)?;
        Ok(Some(DatumHandle::new(m.u16(elem)?, slot as u32)))
    }

    /// `datum_new_at_index` (`0x16b910`): allocate the slot a handle names
    /// and give it the handle's salt.
    ///
    /// Fails if the index is at or past capacity or the slot's salt is
    /// non-zero (the bitmap is not consulted). The scan start is not moved,
    /// and a salt is still consumed from [`layout::NEXT_SALT`] before being
    /// overwritten with the requested one.
    pub fn new_datum_at(
        &self,
        m: &mut dyn GuestMemory,
        handle: DatumHandle,
    ) -> R<Option<DatumHandle>> {
        let index = handle.index() as i32;
        let salt = handle.salt();
        if index >= self.get(m, layout::MAXIMUM_COUNT)? {
            return Ok(None);
        }
        if self.salt_at(m, index)? != 0 {
            return Ok(None);
        }
        let elem = self.claim(m, index, false)?;
        self.initialize_datum(m, elem)?;
        m.set_u16(elem, salt)?;
        Ok(Some(DatumHandle::new(salt, index as u32)))
    }

    /// `datum_new_at_absolute_index` (`0x16b990`): allocate slot `index`
    /// with a fresh salt. Same checks and quirks as
    /// [`new_datum_at`](Self::new_datum_at), plus a negative-index check.
    pub fn new_datum_at_absolute(
        &self,
        m: &mut dyn GuestMemory,
        index: i32,
    ) -> R<Option<DatumHandle>> {
        if index < 0 || index >= self.get(m, layout::MAXIMUM_COUNT)? {
            return Ok(None);
        }
        if self.salt_at(m, index)? != 0 {
            return Ok(None);
        }
        let elem = self.claim(m, index, false)?;
        self.initialize_datum(m, elem)?;
        Ok(Some(DatumHandle::new(m.u16(elem)?, index as u32)))
    }

    /// `datum_delete` (`0x16ba40`): free the slot a handle names. Returns the
    /// new live count (left in EAX; callers ignore it).
    ///
    /// **Unchecked:** only the low 16 bits are used. A stale handle frees
    /// whatever now occupies the slot, and deleting a free slot still
    /// decrements the live count. Deleting the top slot lowers the
    /// high-water mark past any free slots below it.
    pub fn delete(&self, m: &mut dyn GuestMemory, handle: DatumHandle) -> R<i32> {
        let index = handle.index() as i32;
        let data = m.u32(self.f(layout::DATA))?;
        let size = self.get(m, layout::ELEMENT_SIZE)?;
        let elem = (size.wrapping_mul(index) as u32).wrapping_add(data);
        if m.u8(self.f(layout::FLAGS))? & flags::DEBUG_FILL != 0 {
            m.fill(elem, size as u32, DEBUG_FILL_BYTE)?;
        }
        let (word, bit) = bit_of(m.u32(self.f(layout::BITMAP))?, index);
        let w = m.u32(word)?;
        m.set_u32(word, w & !bit)?;
        m.set_u16(elem, 0)?;
        if index < self.get(m, layout::FREE_SCAN_START)? {
            self.put(m, layout::FREE_SCAN_START, index)?;
        }
        if index + 1 == self.get(m, layout::HIGH_WATER)? {
            let size = self.get(m, layout::ELEMENT_SIZE)? as u32;
            let mut p = elem;
            loop {
                let hw = self.get(m, layout::HIGH_WATER)?.wrapping_sub(1);
                p = p.wrapping_sub(size);
                self.put(m, layout::HIGH_WATER, hw)?;
                if hw <= 0 || m.u16(p)? != 0 {
                    break;
                }
            }
        }
        let live = self.get(m, layout::ACTIVE_COUNT)?.wrapping_sub(1);
        self.put(m, layout::ACTIVE_COUNT, live)?;
        Ok(live)
    }

    // ---- lookup ----------------------------------------------------------

    /// `datum_try_and_get` (`0x16bae0`): the element a handle names, if its
    /// slot is below the high-water mark, live, and carries the handle's
    /// salt. `None` (guest 0) otherwise, including for
    /// [`DatumHandle::NONE`].
    pub fn try_get(&self, m: &dyn GuestMemory, handle: DatumHandle) -> R<Option<u32>> {
        if handle.is_none() {
            return Ok(None);
        }
        let index = handle.index() as i32;
        if index >= self.get(m, layout::HIGH_WATER)? {
            return Ok(None);
        }
        let elem = self.element_addr(m, index)?;
        let salt = m.u16(elem)?;
        Ok((salt != 0 && salt == handle.salt()).then_some(elem))
    }

    /// `datum_get_absolute` (`0x16bb20`): element `index` if it is in
    /// `0..high_water` and live; salts are not compared.
    pub fn get_absolute(&self, m: &dyn GuestMemory, index: i32) -> R<Option<u32>> {
        if index < 0 || index >= self.get(m, layout::HIGH_WATER)? {
            return Ok(None);
        }
        let elem = self.element_addr(m, index)?;
        Ok((m.u16(elem)? != 0).then_some(elem))
    }

    /// `datum_absolute_index_to_index` (`0x16bb50`): the handle for slot
    /// `index`, built from whatever salt is stored there. No bounds or
    /// liveness check; `-1` maps to `None`.
    pub fn handle_of(&self, m: &dyn GuestMemory, index: i32) -> R<Option<DatumHandle>> {
        if index == -1 {
            return Ok(None);
        }
        // A composed 0xffffffff is indistinguishable from "none" to callers.
        Ok(DatumHandle::from_raw(
            DatumHandle::new(self.salt_at(m, index)?, index as u32).raw(),
        ))
    }

    // ---- iteration -------------------------------------------------------

    /// `data_next_index` (`0x16bc00`): the first slot at or after `start`
    /// whose bitmap bit is set, below the high-water mark.
    pub fn next_index(&self, m: &dyn GuestMemory, start: i32) -> R<Option<i32>> {
        if start < 0 {
            return Ok(None);
        }
        let hw = self.get(m, layout::HIGH_WATER)?;
        if start >= hw {
            return Ok(None);
        }
        let bitmap = m.u32(self.f(layout::BITMAP))?;
        for i in start..hw {
            let (word, bit) = bit_of(bitmap, i);
            if m.u32(word)? & bit != 0 {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    /// `data_next_datum` (`0x16bbc0`): the live datum after `prev` (or the
    /// first, for `None`). Occupancy comes from the bitmap; the salt is read
    /// from the element.
    pub fn next_datum(
        &self,
        m: &dyn GuestMemory,
        prev: Option<DatumHandle>,
    ) -> R<Option<DatumHandle>> {
        let start = prev.map_or(0, |h| h.index() as i32 + 1);
        match self.next_index(m, start)? {
            Some(i) => self.handle_of(m, i),
            None => Ok(None),
        }
    }

    /// `data_previous_datum` (`0x16bcc0`, assigned name): the live datum
    /// before `next` (or the last below the high-water mark, for `None`).
    /// Occupancy comes from salts, not the bitmap.
    ///
    /// Quirk: when nothing is found the guest loop reads one more salt, at
    /// element `-1` (just below the data pointer), before giving up; the
    /// result is `None` either way. The read is reproduced so faults match.
    pub fn previous_datum(
        &self,
        m: &dyn GuestMemory,
        next: Option<DatumHandle>,
    ) -> R<Option<DatumHandle>> {
        let start = match next {
            None => self.get(m, layout::HIGH_WATER)?,
            Some(h) => h.index() as i32,
        };
        let mut i = start.wrapping_sub(1);
        if i < 0 || i >= self.get(m, layout::HIGH_WATER)? {
            return Ok(None);
        }
        let data = m.u32(self.f(layout::DATA))?;
        let size = self.get(m, layout::ELEMENT_SIZE)? as u32;
        let mut p = size.wrapping_mul(i as u32).wrapping_add(data);
        loop {
            let salt = m.u16(p)?;
            if salt != 0 {
                // At i == -1 the guest computes (salt << 16) | -1 == -1.
                return Ok(DatumHandle::from_raw(((salt as u32) << 16) | i as u32));
            }
            p = p.wrapping_sub(size);
            let checked = i;
            i -= 1;
            if checked < 0 {
                return Ok(None);
            }
        }
    }

    /// `data_last_datum` (`0x16bc40`, assigned name): walk every live datum
    /// with [`next_datum`](Self::next_datum) and return the last. (Linear in
    /// the high-water mark; the guest does the same walk.) A composed
    /// handle of `0xffffffff` ends the walk, as in the guest.
    pub fn last_datum(&self, m: &dyn GuestMemory) -> R<Option<DatumHandle>> {
        let mut cur = None;
        while let Some(h) = self.next_datum(m, cur)? {
            if h.is_none() {
                break;
            }
            cur = Some(h);
        }
        Ok(cur)
    }
}

/// A guest data iterator, as advanced by `data_iterator_next` (`0x16bb70`).
///
/// Layout: `+0` array pointer, `+4` current handle, `+8` current absolute
/// index. Callers initialize it inline (index `-1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataIterator {
    pub addr: u32,
}

impl DataIterator {
    pub const ARRAY: u32 = 0;
    pub const HANDLE: u32 = 4;
    pub const INDEX: u32 = 8;

    pub const fn at(addr: u32) -> Self {
        DataIterator { addr }
    }

    /// `data_iterator_next` (`0x16bb70`): advance to the next live element
    /// and return its address. At the end it stores the array's capacity as
    /// the index and `NONE` as the handle, and returns `None` (guest 0).
    pub fn next(&self, m: &mut dyn GuestMemory) -> R<Option<u32>> {
        let index = m.i32(self.addr.wrapping_add(Self::INDEX))?.wrapping_add(1);
        let array = DatumArray::at(m.u32(self.addr.wrapping_add(Self::ARRAY))?);
        match array.next_index(m, index)? {
            Some(i) => {
                let elem = array.element_addr(m, i)?;
                m.set_i32(self.addr.wrapping_add(Self::INDEX), i)?;
                let h = DatumHandle::new(m.u16(elem)?, i as u32);
                m.set_u32(self.addr.wrapping_add(Self::HANDLE), h.raw())?;
                Ok(Some(elem))
            }
            None => {
                let cap = array.get(m, layout::MAXIMUM_COUNT)?;
                m.set_i32(self.addr.wrapping_add(Self::INDEX), cap)?;
                m.set_u32(
                    self.addr.wrapping_add(Self::HANDLE),
                    DatumHandle::NONE.raw(),
                )?;
                Ok(None)
            }
        }
    }
}

/// Where [`DatumArray::initialize_with`] gets its arguments.
enum InitArgs {
    Values([u32; 6]),
    /// Guest address of the first stack argument.
    Stack(u32),
}

impl InitArgs {
    fn dword(&self, m: &dyn GuestMemory, n: u32) -> R<u32> {
        match self {
            InitArgs::Values(v) => Ok(v[n as usize]),
            InitArgs::Stack(base) => m.u32(base.wrapping_add(4 * n)),
        }
    }

    /// The original reads the alignment argument as a byte
    /// (`mov cl, [esp+20h]`).
    fn byte(&self, m: &dyn GuestMemory, n: u32) -> R<u8> {
        match self {
            InitArgs::Values(v) => Ok(v[n as usize] as u8),
            InitArgs::Stack(base) => m.u8(base.wrapping_add(4 * n)),
        }
    }
}
