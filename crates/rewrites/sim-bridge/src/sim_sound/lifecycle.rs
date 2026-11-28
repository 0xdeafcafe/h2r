//! The sound manager's lifecycle as Rust bodies: create `0x1253d0` and
//! drop `0x125600` (docs/engine/sound-instances.md, "Entries",
//! "Lifecycle").
//!
//! The instance pool (`[0x4e637c]`) and the channel records
//! (`[0x4e6378]`) are the table's ([`super::entries::Table`]): create
//! allocates neither and leaves both globals 0, so a guest function that
//! still reads them faults with its address. Everything else create makes
//! keeps its guest layout, because guest code still reads it:
//!
//! | global | what | readers left |
//! |---|---|---|
//! | `[0x4e6380]` | sound globals, `0x20c` bytes | the manager `0x125690` to `0x12af90`, the dialogue `0x189400` to `0x18d670` |
//! | `[0x51ebd8]`, `[0x51ebdc]` | companion array (`0x40` x `0x1c`) and its default record | `0x1257b0`, `0x127390`, `0x128ca0` |
//! | `[0x502110]` | looping sounds (`0x80` x `0xd4`) | `0x1257b0` (null-checked) |
//! | `[0x51ebe0]` | start records (`0x10` x `0x14`) | `0x225b60` |
//! | `[0x502114]` | class rows (`[0x44a0d4]` x `0x6c`) | `0x1295e0`, `0x21dde0` to `0x21e050` |
//!
//! Two pieces stay guest calls, each a subsystem of its own: the sound
//! cache (`0x2185d0` create, `0x2186b0` dispose: allocator vtables and the
//! cache object `0x13d170` builds, read by the cache code at `0x2ae170` on)
//! and the driver (`0x21e4b0` create, `0x21eae0` dispose: `DirectSoundCreate`
//! and the buffers at `[0x51ebe4]`, read by the per-map init `0x191270`,
//! the pause and Bink).

use guest_layout::GuestMemory;
use guest_layout::MemFault;
use guest_layout::datum::{DatumArray, bitmap_bytes, layout};
use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::points;
use crate::sim_sound::clock::system_time;

/// `[0x4e6374]`: create's reference count. Only the first create and the
/// last drop do any work.
pub const REFCOUNT: u32 = 0x004e_6374;
/// Bytes of the sound globals (`0x83` dwords, `0x125422`).
pub const GLOBALS_SIZE: u32 = 0x20c;

/// The game-state allocator: its base, the bytes taken and the CRC-32
/// register of every size it handed out (`0x11cc20`, `0x1253e1`).
pub const GAME_STATE_BASE: u32 = 0x0051_0800;
pub const GAME_STATE_USED: u32 = 0x0051_0804;
pub const GAME_STATE_CRC: u32 = 0x0051_0808;
/// The allocator object `data_new` (`0x11cc20`) gives each array.
pub const DATUM_ALLOCATOR: u32 = 0x0046_875c;

/// `0x2185d0`: `sound_cache_initialize`.
pub const CACHE_CREATE: u32 = 0x0021_85d0;
/// `0x2186b0`: the sound cache's dispose (its vtable objects released).
pub const CACHE_DISPOSE: u32 = 0x0021_86b0;
/// `0x21e4b0(classes) -> al`: the driver's create (`DirectSoundCreate`,
/// the buffers), `ret 4`.
pub const DRIVER_CREATE: u32 = 0x0021_e4b0;
/// `0x21eae0`: the driver's dispose.
pub const DRIVER_DISPOSE: u32 = 0x0021_eae0;

/// `0x44a0c8`: the sound classes. Words at `+6` are the channel counts of
/// the three classes, the word at `+0xc` the class-row count.
pub const CLASSES: u32 = 0x0044_a0c8;
const CLASS_CHANNELS: u32 = CLASSES + 6;
const CLASS_ROWS_COUNT: u32 = CLASSES + 0xc;
/// `[0x43f17c]`: the ambience slot's starting gain (`0x125445`).
pub const AMBIENCE_GAIN: u32 = 0x0043_f17c;

/// One guest datum array create makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Array {
    pub global: u32,
    pub name: u32,
    pub count: i32,
    pub size: i32,
}

/// `0x219b40`: the companion array.
pub const COMPANIONS: Array = Array { global: 0x0051_ebd8, name: 0x0045_70d0, count: 0x40, size: 0x1c };
/// `[0x51ebdc]`: the companions' default record, `0x1c` bytes.
pub const COMPANION_DEFAULT: u32 = 0x0051_ebdc;
const COMPANION_DEFAULT_SIZE: u32 = 0x1c;
/// `0x21a0c0`: the looping sounds.
pub const LOOPS: Array = Array { global: 0x0050_2110, name: 0x0045_70ec, count: 0x80, size: 0xd4 };
/// `0x21d490`: the start records.
pub const STARTS: Array = Array { global: 0x0051_ebe0, name: 0x0045_70fc, count: 0x10, size: 0x14 };
/// `0x21dda0`: the class rows, as many as the classes name.
pub const CLASS_ROWS: u32 = 0x0050_2114;
const CLASS_ROWS_NAME: u32 = 0x0045_710c;
const CLASS_ROW_SIZE: i32 = 0x6c;

/// Sound globals offsets create writes.
mod g {
    pub const READY: u32 = 0x78;
    pub const ON: u32 = 0x79;
    pub const BUSY: u32 = 0x7d;
    pub const MUTED: u32 = 0x7f;
    pub const SLOT_BITS: u32 = 0x60;
    pub const SLOT_BITS_LEN: u32 = 0x80;
    pub const SOURCE_BITS: u32 = 0x70;
    pub const SOURCE_BITS_LEN: u32 = 0x40;
    /// The two ambience slots' tag (-1), blend (0.0) and gain.
    pub const AMBIENCE: [u32; 2] = [0x1b0, 0x1cc];
    pub const CHANNEL_COUNT: u32 = 0x208;
    pub const CHANNEL_LIMIT: u32 = 0x20a;
}
/// `0x1255dc`: the word at globals `+0x20a`.
const CHANNEL_LIMIT: u16 = 0x33;

/// The reflected CRC-32 (polynomial `0xedb88320`) of `bytes` onto the
/// register `crc`, no pre- or post-inversion (`0x163ba0`).
pub fn crc_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    crc
}

/// The game-state allocator's inline body (`0x1253e1` to `0x12541b`):
/// the top aligned to 4, `size` plus the padding taken and fed to the CRC
/// register. Nothing is cleared.
pub fn game_state_alloc(m: &mut dyn GuestMemory, size: u32) -> Result<u32, MemFault> {
    let used = m.u32(GAME_STATE_USED)?;
    let top = m.u32(GAME_STATE_BASE)?.wrapping_add(used);
    let at = top.wrapping_add(3) & !3;
    let taken = at.wrapping_sub(top).wrapping_add(size);
    m.set_u32(GAME_STATE_USED, used.wrapping_add(taken))?;
    let crc = m.u32(GAME_STATE_CRC)?;
    m.set_u32(GAME_STATE_CRC, crc_update(crc, &taken.to_le_bytes()))?;
    Ok(at)
}

/// `data_new` (`0x11cc20`): a contiguous array in game state, header,
/// elements and bitmap, made valid and empty as each caller does next
/// (`+0x29 = 1`, `0x16b7a0`). Its address goes to `global`.
pub fn data_new(m: &mut dyn GuestMemory, a: Array) -> Result<u32, MemFault> {
    let bytes = (a.count.wrapping_mul(a.size) as u32).wrapping_add(bitmap_bytes(a.count)).wrapping_add(layout::HEADER_SIZE);
    let at = game_state_alloc(m, bytes)?;
    let allocator = m.u32(DATUM_ALLOCATOR)?;
    let array = DatumArray::at(at);
    array.initialize_contiguous(m, a.name, a.count, a.size, 0, allocator)?;
    m.set_u32(a.global, at)?;
    array.make_valid(m)?;
    Ok(at)
}

/// Create's first half (to `0x125438`): the reference count, and on the
/// first create the zeroed globals with sound on. `false`: not the first.
fn begin(m: &mut dyn GuestMemory) -> Result<bool, MemFault> {
    let n = m.u32(REFCOUNT)?;
    if n != 0 {
        m.set_u32(REFCOUNT, n.wrapping_add(1))?;
        return Ok(false);
    }
    let globals = game_state_alloc(m, GLOBALS_SIZE)?;
    m.fill(globals, GLOBALS_SIZE, 0)?;
    m.set_u32(points::SOUND_GLOBALS.base, globals)?;
    m.set_u8(globals + g::ON, 1)?;
    m.set_u8(globals + g::BUSY, 0)?;
    Ok(true)
}

/// The seed of the companions' default record: retail's `time(0) ^ rand()
/// ^ KeTickCount` (`0x219bba` to `0x219be4`), with the host's wall clock
/// for `time` and no CRT `rand` draw. It only seeds.
fn seed(m: &dyn GuestMemory) -> Result<u32, MemFault> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32);
    Ok(secs ^ system_time(m)?)
}

/// `0x219b40`: the companion array and its default record.
fn companions(m: &mut dyn GuestMemory, seed: u32) -> Result<(), MemFault> {
    data_new(m, COMPANIONS)?;
    let record = game_state_alloc(m, COMPANION_DEFAULT_SIZE)?;
    m.set_u32(COMPANION_DEFAULT, record)?;
    let muted = m.u8(m.u32(points::SOUND_GLOBALS.base)? + g::MUTED)?;
    m.set_u32(record + 8, seed)?;
    m.set_u32(record + 4, u32::MAX)?;
    let keep = m.u8(record + 2)? & 0x7f;
    m.set_u8(record + 2, ((!muted) << 7) | keep)?;
    m.set_u8(record + 3, 0x80)?;
    m.fill(record + 0xc, 0x10, 0)
}

/// Create from `0x12543d` to the driver: the ambience slots, the pool and
/// channel globals 0, and the companion, loop, start and class-row arrays.
fn arrays(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let globals = m.u32(points::SOUND_GLOBALS.base)?;
    let gain = m.u32(AMBIENCE_GAIN)?;
    let [first, second] = g::AMBIENCE;
    m.set_u32(globals + first, u32::MAX)?;
    m.set_u32(globals + first + 4, 0)?;
    m.set_u32(globals + first + 8, gain)?;
    m.set_u32(globals + second, u32::MAX)?;
    m.set_u32(globals + second + 4, 0)?;
    m.set_u32(globals + second + 8, 0)?;
    m.set_u32(points::SOUND_INSTANCES.base, 0)?;
    m.set_u32(points::SOUND_CHANNELS.base, 0)?;
    let seed = seed(m)?;
    companions(m, seed)?;
    data_new(m, LOOPS)?;
    data_new(m, STARTS)?;
    let rows = m.u16(CLASS_ROWS_COUNT)? as i16;
    data_new(m, Array { global: CLASS_ROWS, name: CLASS_ROWS_NAME, count: i32::from(rows), size: CLASS_ROW_SIZE })?;
    Ok(())
}

/// Create after the driver (`0x12554d` to `0x1255e5`), when it came up:
/// the two bit sets clear, the classes' channel counts summed into
/// `+0x208` (the channel records are the table's), the limit, and ready.
fn ready(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let globals = m.u32(points::SOUND_GLOBALS.base)?;
    m.fill(globals + g::SLOT_BITS, bitmap_bytes(g::SLOT_BITS_LEN as i32), 0)?;
    m.fill(globals + g::SOURCE_BITS, bitmap_bytes(g::SOURCE_BITS_LEN as i32), 0)?;
    for class in 0..3 {
        let count = m.u16(CLASS_CHANNELS + 2 * class)?;
        let sum = m.u16(globals + g::CHANNEL_COUNT)?.wrapping_add(count);
        m.set_u16(globals + g::CHANNEL_COUNT, sum)?;
    }
    m.set_u16(globals + g::CHANNEL_LIMIT, CHANNEL_LIMIT)?;
    m.set_u8(globals + g::READY, 1)
}

fn add_ref(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let n = m.u32(REFCOUNT)?;
    m.set_u32(REFCOUNT, n.wrapping_add(1))
}

/// `0x1253d0`'s body. Returns the guest calls it made. A driver that does
/// not come up leaves `+0x78` 0, as retail does.
pub async fn create(cx: &Cx) -> Result<u64, EntryError> {
    if !cx.write(begin)? {
        return Ok(0);
    }
    cx.call(GuestCall::new(CACHE_CREATE)).await?;
    cx.write(arrays)?;
    let driver = cx.call(GuestCall::new(DRIVER_CREATE).args([CLASSES])).await?;
    cx.write(|m| {
        if driver.eax & 0xff != 0 {
            ready(m)?;
        }
        add_ref(m)
    })?;
    Ok(2)
}

/// Drop after the guest disposes (`0x125616` to `0x125687`): every global
/// create set, 0; the class rows and the pool marked invalid first.
fn clear(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    m.set_u32(STARTS.global, 0)?;
    for global in [CLASS_ROWS, points::SOUND_INSTANCES.base] {
        let array = m.u32(global)?;
        if array != 0 {
            m.set_u8(array + layout::VALID, 0)?;
            m.set_u32(global, 0)?;
        }
    }
    for global in [LOOPS.global, COMPANIONS.global, COMPANION_DEFAULT, points::SOUND_GLOBALS.base, points::SOUND_CHANNELS.base] {
        m.set_u32(global, 0)?;
    }
    Ok(())
}

/// `0x125600`'s body. Returns the guest calls it made.
pub async fn dispose(cx: &Cx) -> Result<u64, EntryError> {
    let last = cx.write(|m| -> Result<bool, MemFault> {
        let n = m.u32(REFCOUNT)?.wrapping_sub(1);
        m.set_u32(REFCOUNT, n)?;
        Ok(n == 0)
    })?;
    if !last {
        return Ok(0);
    }
    cx.call(GuestCall::new(CACHE_DISPOSE)).await?;
    cx.call(GuestCall::new(DRIVER_DISPOSE)).await?;
    cx.write(clear)?;
    Ok(2)
}

#[cfg(test)]
mod tests;
