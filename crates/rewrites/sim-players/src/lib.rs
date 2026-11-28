//! The players, as the native simulation holds them (docs/engine/sim-world.md,
//! "The players store"): retail's players datum array (`players_initialize`
//! `0x0014b4b0`: 16 slots of `0x21c` bytes, `datum_new` `0x0016b840` and
//! `datum_delete` `0x0016ba40` over it), the players' globals block (`0x130`
//! bytes, the local-user map at `+0xc`) and the four local users' first-person
//! state (`0x20cc` bytes each, the `first_person` phase `0x00165dc1`), as
//! one [`PlayersState`]. The pool is the truth; [`PlayersState::scatter`]
//! writes it to the arena for the guest readers that remain and
//! [`PlayersMem`] shows it to the typed views (`guest_layout::player`,
//! `sim-state`) as guest memory.
//!
//! A record is the datum's bytes after its salt, kept whole: the retail
//! layout is the state, so a record never disagrees with the guest's and
//! the views read the offsets `guest_layout` names.
#![deny(clippy::disallowed_methods)]

use guest_layout::datum_array::DatumArray;
use guest_layout::player::Player;
use guest_layout::{Fault, Mem, MemMut, Off};
use sim_world::{Fnv64, Handle, Pool, Reader, Record, RestoreError, StaleHandle, Writer, kind};

/// Slots in retail's players array (`players_initialize` passes `0x10`).
pub const PLAYER_CAPACITY: u32 = 16;
/// The array's name seed: the first two bytes of the name `0x453984`
/// ("pl"), as the little-endian word `data_initialize` ORs with `0x8000`
/// into `next_salt`.
pub const PLAYER_NAME: u16 = 0x6c70;
/// Bytes per player datum, salt included.
pub const PLAYER_SIZE: u32 = Player::SIZE;
/// Bytes of a record: the datum after its salt.
pub const RECORD_SIZE: usize = PLAYER_SIZE as usize - 2;
/// Bytes of the players' globals block.
pub const GLOBALS_SIZE: usize = 0x130;
/// Local users with first-person state.
pub const FIRST_PERSON_USERS: usize = 4;
/// Bytes of one user's first-person block.
pub const FIRST_PERSON_USER_SIZE: usize = 0x20cc;
/// Bytes of the first-person block.
pub const FIRST_PERSON_SIZE: usize = FIRST_PERSON_USERS * FIRST_PERSON_USER_SIZE;
/// The section kind the state is saved under.
pub const KIND: u16 = kind::PLAYERS;

/// One player: the datum's bytes from `+2`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerRecord {
    pub bytes: Box<[u8; RECORD_SIZE]>,
}

impl Default for PlayerRecord {
    fn default() -> Self {
        Self { bytes: Box::new([0; RECORD_SIZE]) }
    }
}

impl PlayerRecord {
    /// A record from the datum's bytes after the salt; `None` when `bytes`
    /// is not [`RECORD_SIZE`] long.
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        Some(Self { bytes: Box::new(bytes.try_into().ok()?) })
    }
}

impl Record for PlayerRecord {
    fn write(&self, w: &mut Writer) {
        w.bytes(&self.bytes[..]);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, RestoreError> {
        Ok(Self::from_slice(r.take(RECORD_SIZE)?).expect("take returned the length asked"))
    }
}

/// A block of guest memory the simulation owns: where it is, and its bytes.
/// The address is state: the guest's allocator placed it and readers that
/// remain find it there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    pub addr: u32,
    pub bytes: Vec<u8>,
}

impl Blob {
    fn zeroed(len: usize) -> Self {
        Self { addr: 0, bytes: vec![0; len] }
    }

    fn write(&self, w: &mut Writer) {
        w.u32(self.addr);
        w.bytes(&self.bytes);
    }

    fn read(r: &mut Reader<'_>, len: usize) -> Result<Self, RestoreError> {
        Ok(Self { addr: r.u32()?, bytes: r.take(len)?.to_vec() })
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

/// The players, their globals and the first-person state.
pub struct PlayersState {
    pub players: Pool<PlayerRecord>,
    /// `0x130` bytes at `[4e8c20]`; `addr` 0 while no game is running.
    pub globals: Blob,
    /// `0x8330` bytes at `[4e9bc8]`.
    pub first_person: Blob,
}

impl Default for PlayersState {
    fn default() -> Self {
        Self {
            players: Pool::new(PLAYER_CAPACITY, PLAYER_NAME),
            globals: Blob::zeroed(GLOBALS_SIZE),
            first_person: Blob::zeroed(FIRST_PERSON_SIZE),
        }
    }
}

impl PlayersState {
    /// The section version.
    pub const VERSION: u16 = 1;

    /// `datum_new`: the first free slot at or after the scan start, else
    /// the high water. `None` when the array is full.
    pub fn alloc(&mut self, record: PlayerRecord) -> Option<Handle> {
        self.players.alloc(record)
    }

    /// `datum_delete`, refusing a handle whose salt is not the slot's.
    pub fn delete(&mut self, h: Handle) -> Result<PlayerRecord, StaleHandle> {
        self.players.free(h)
    }

    /// Live players in slot order.
    pub fn iter(&self) -> impl Iterator<Item = (Handle, &PlayerRecord)> {
        self.players.iter()
    }

    /// The canonical section bytes: the pool's (free slots keep their
    /// place), then the globals and the first-person blocks, each with its
    /// address.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        let pool = self.players.encode();
        w.u32(pool.len() as u32);
        w.bytes(&pool);
        self.globals.write(&mut w);
        self.first_person.write(&mut w);
        w.finish()
    }

    pub fn section_hash(&self) -> u64 {
        Fnv64::of(&self.encode())
    }

    /// A saved section checked and decoded.
    pub fn decode(version: u16, body: &[u8]) -> Result<Self, RestoreError> {
        if version != Self::VERSION {
            return Err(RestoreError::SectionVersion { kind: KIND, version });
        }
        let mut r = Reader::new(body);
        let len = r.u32()? as usize;
        let pool = r.take(len)?;
        let players = Pool::decode(KIND, PLAYER_CAPACITY, PLAYER_NAME, 1, pool)?;
        let globals = Blob::read(&mut r, GLOBALS_SIZE)?;
        let first_person = Blob::read(&mut r, FIRST_PERSON_SIZE)?;
        r.end(KIND)?;
        Ok(Self { players, globals, first_person })
    }

    /// The state the guest's arrays hold now: the players array with its
    /// allocation state (`next_salt`, `free_scan_start`), the globals block
    /// and the first-person block, each at the address its global names. A
    /// slot is live when its bitmap bit and its salt agree. `None` while
    /// no game has initialised the players.
    pub fn import<M: Mem + ?Sized>(m: &M) -> Result<Option<Self>, ImportError> {
        let Some(addr) = guest_layout::global::PLAYERS.pointer(m)? else {
            return Ok(None);
        };
        let array = DatumArray::at(addr);
        let capacity = array.maximum_count(m)?.max(0) as u32;
        let high = array.high_water(m)?.max(0) as u32;
        let scan = Off::<i32>::new(0x34).read(m, addr)?.max(0) as u32;
        let next_salt = Off::<u16>::new(0x40).read(m, addr)?;
        let (data, bitmap) = (array.data(m)?, array.bitmap(m)?);
        if array.element_size(m)? as u32 != PLAYER_SIZE {
            return Err(ImportError::Corrupt("element size is not a player's"));
        }
        if capacity != PLAYER_CAPACITY {
            return Err(ImportError::Corrupt("capacity is not the players'"));
        }
        let mut parts = Vec::with_capacity(high as usize);
        for slot in 0..high {
            let at = data.wrapping_add(slot * PLAYER_SIZE);
            let word = Off::<u32>::new(0).read(m, bitmap.wrapping_add((slot >> 5) * 4))?;
            let salt = Off::<u16>::new(0).read(m, at)?;
            if salt == 0 || word & (1 << (slot & 31)) == 0 {
                parts.push((0, None));
                continue;
            }
            let mut bytes = [0u8; RECORD_SIZE];
            m.read_bytes(at.wrapping_add(2), &mut bytes)?;
            parts.push((salt, Some(PlayerRecord { bytes: Box::new(bytes) })));
        }
        let players = Pool::from_parts(capacity, next_salt, scan, parts).map_err(ImportError::Corrupt)?;
        let globals = import_blob(m, guest_layout::global::PLAYER_GLOBALS.pointer(m)?, GLOBALS_SIZE)?;
        let first_person = import_blob(m, guest_layout::global::FIRST_PERSON.pointer(m)?, FIRST_PERSON_SIZE)?;
        Ok(Some(Self { players, globals, first_person }))
    }

    /// The array's header fields as retail stores them, for the allocation
    /// state a scatter writes.
    fn array_fields(&self) -> [(u32, [u8; 4]); 4] {
        [
            (0x34, (self.players.free_scan_start() as i32).to_le_bytes()),
            (0x38, (self.players.high_water() as i32).to_le_bytes()),
            (0x3c, (self.players.len() as i32).to_le_bytes()),
            (0x40, [self.players.next_salt().to_le_bytes()[0], self.players.next_salt().to_le_bytes()[1], 0, 0]),
        ]
    }

    /// `slots` datums, free slots zero.
    fn data_image(&self, slots: u32) -> Vec<u8> {
        let mut out = vec![0u8; (slots * PLAYER_SIZE) as usize];
        for (h, r) in self.players.iter() {
            let at = (u32::from(h.index()) * PLAYER_SIZE) as usize;
            if at + PLAYER_SIZE as usize > out.len() {
                continue;
            }
            out[at..at + 2].copy_from_slice(&h.salt().to_le_bytes());
            out[at + 2..at + PLAYER_SIZE as usize].copy_from_slice(&r.bytes[..]);
        }
        out
    }

    /// The occupancy bitmap over `slots` slots, in 32-bit words.
    fn bitmap_image(&self, slots: u32) -> Vec<u8> {
        let mut words = vec![0u32; slots.div_ceil(32) as usize];
        for (h, _) in self.players.iter() {
            if let Some(w) = words.get_mut(usize::from(h.index()) >> 5) {
                *w |= 1 << (h.index() & 31);
            }
        }
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// Write the state to the guest: the array's allocation state, every
    /// datum and bitmap word up to the larger of the old and new high water
    /// (so a freed slot's salt and bit clear), and the two blocks at their
    /// addresses. The array's own storage (`data`, `bitmap`,
    /// `maximum_count`) is the guest's and stays.
    pub fn scatter<M: MemMut + ?Sized>(&self, m: &mut M) -> Result<(), Fault> {
        if let Some(addr) = guest_layout::global::PLAYERS.pointer(&*m)? {
            let array = DatumArray::at(addr);
            let (data, bitmap) = (array.data(&*m)?, array.bitmap(&*m)?);
            let span = (array.high_water(&*m)?.max(0) as u32).max(self.players.high_water());
            m.write_bytes(data, &self.data_image(span))?;
            m.write_bytes(bitmap, &self.bitmap_image(span))?;
            for (at, v) in self.array_fields() {
                let len = if at == 0x40 { 2 } else { 4 };
                m.write_bytes(addr.wrapping_add(at), &v[..len])?;
            }
        }
        for blob in [&self.globals, &self.first_person] {
            if blob.addr != 0 {
                m.write_bytes(blob.addr, &blob.bytes)?;
            }
        }
        Ok(())
    }
}

fn import_blob<M: Mem + ?Sized>(m: &M, addr: Option<u32>, len: usize) -> Result<Blob, ImportError> {
    let Some(addr) = addr else { return Ok(Blob::zeroed(len)) };
    let mut bytes = vec![0u8; len];
    m.read_bytes(addr, &mut bytes)?;
    Ok(Blob { addr, bytes })
}

/// Where the array header appears in [`PlayersMem`]: addresses past the
/// guest's 64 MB, so they never collide with a block.
pub const VIRT_ARRAY: u32 = 0x7f20_0000;
const VIRT_BITMAP: u32 = 0x7f21_0000;
const VIRT_DATA: u32 = 0x7f22_0000;
/// The globals block and the first-person block in [`PlayersMem`].
pub const VIRT_GLOBALS: u32 = 0x7f23_0000;
pub const VIRT_FIRST_PERSON: u32 = 0x7f24_0000;

/// The state seen as guest memory: the three globals (`PLAYERS`,
/// `PLAYER_GLOBALS`, `FIRST_PERSON`) pointing at a virtual array header,
/// datums and bitmap, and the two blocks, and nothing else. A snapshot of
/// the shape taken when built: build one per read burst and keep none across
/// a tick.
pub struct PlayersMem<'a> {
    state: &'a PlayersState,
    array: [u8; 0x4c],
    data: Vec<u8>,
    bitmap: Vec<u8>,
}

impl<'a> PlayersMem<'a> {
    pub fn new(state: &'a PlayersState) -> Self {
        let high = state.players.high_water();
        let mut array = [0u8; 0x4c];
        let put = |a: &mut [u8; 0x4c], at: usize, v: &[u8]| a[at..at + v.len()].copy_from_slice(v);
        put(&mut array, 0x20, &(state.players.capacity() as i32).to_le_bytes());
        put(&mut array, 0x24, &(PLAYER_SIZE as i32).to_le_bytes());
        array[0x29] = 1;
        for (at, v) in state.array_fields() {
            let len = if at == 0x40 { 2 } else { 4 };
            put(&mut array, at as usize, &v[..len]);
        }
        put(&mut array, 0x44, &VIRT_DATA.to_le_bytes());
        put(&mut array, 0x48, &VIRT_BITMAP.to_le_bytes());
        Self { state, array, data: state.data_image(high), bitmap: state.bitmap_image(high) }
    }
}

impl Mem for PlayersMem<'_> {
    fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> Result<(), Fault> {
        use guest_layout::global::{FIRST_PERSON, PLAYERS, PLAYER_GLOBALS};
        fn within(addr: u32, len: usize, base: u32, bytes: &[u8]) -> Option<&[u8]> {
            let at = addr.checked_sub(base)? as usize;
            bytes.get(at..at.checked_add(len)?)
        }
        let fault = Fault { addr, len: buf.len() as u32, write: false };
        if buf.len() == 4 {
            let pointer = if addr == PLAYERS.base {
                Some(VIRT_ARRAY)
            } else if addr == PLAYER_GLOBALS.base {
                Some(VIRT_GLOBALS)
            } else if addr == FIRST_PERSON.base {
                Some(VIRT_FIRST_PERSON)
            } else {
                None
            };
            if let Some(p) = pointer {
                buf.copy_from_slice(&p.to_le_bytes());
                return Ok(());
            }
        }
        let bytes = within(addr, buf.len(), VIRT_ARRAY, &self.array)
            .or_else(|| within(addr, buf.len(), VIRT_BITMAP, &self.bitmap))
            .or_else(|| within(addr, buf.len(), VIRT_DATA, &self.data))
            .or_else(|| within(addr, buf.len(), VIRT_GLOBALS, &self.state.globals.bytes))
            .or_else(|| within(addr, buf.len(), VIRT_FIRST_PERSON, &self.state.first_person.bytes))
            .ok_or(fault)?;
        buf.copy_from_slice(bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
