//! Layer 5's PRT stream of an object's record (docs/graphics/framescene.md,
//! slice 4): what the geometry draw `0x0004d0b0` adds for an object item
//! of layer 5, read as the guest reads it ([`read`]).
//!
//! Research points (**Code**, `h2 disasm`):
//!
//! - `0x0004d220`..`0x0004d281`: in layer 5, for a record whose model
//!   (`+4`) is not -1 and whose shader has no flag bit 0, the draw takes
//!   the model's block at `+0x74` (count) / `+0x78` (elements of `0x58`
//!   bytes; the first, the PRT block). With an element, it calls
//!   `0x00024040` (ECX the object key `[[record +0x10]]`, EAX the slot
//!   `[record +0xc] & 0xf`, the PRT block pushed), then `0x00024490` (EAX
//!   the object key, EBX the section `[record +0xc] >> 9 & 0x1ff`, EDI the
//!   PRT block), whose result is the extra stream the stream setup
//!   `0x0001c8c0` binds after the section's (none when 0).
//! - The objects' PRT entries: `[0x005093e8]` of them at `0x004b89b0`,
//!   `0x68` bytes each, matched by the object key at `+0`. `+4` (i16) the
//!   object's detail step, `+8` the datum of its coefficients (none when
//!   -1 or negative; else `datum & 0x0fffffff` past `0x00487b1c`), `+0xc`
//!   sixteen dword offsets into them, `+0x4c` sixteen counts (bytes), one
//!   of each per slot, and `+0x5c` the vector of `c[68]`.
//! - `0x00024040`: without an entry, or with a count of 0, it sets
//!   nothing. Otherwise `c[68]` the entry's vector, `c[69]` the floats at
//!   `0x0045dbc0`, `0x0045dc10`, `0x0045e448` and `0x0045e444`, and from
//!   `c[70]` `n` registers of the coefficients at the slot's offset (in
//!   dwords), where `n = (word [prt +4] * 3 + 4) * word [prt +2] *
//!   min(count, 2) / 4` (signed words, the division truncating), through
//!   `0x003f71d0` (one), `0x003f7280` (four) or `0x003f7330` (any other).
//! - `0x00024490`: without an entry it returns 0. The level is
//!   `min(5 - entry +4, [prt +0x14] - 1)`; `[prt +0x18]` holds the levels
//!   (`0xc` bytes: `+4` count, `+8` pairs of a section and an offset);
//!   without the item's section it returns 0. It asks the geometry cache
//!   (`0x0012de70`, flags 3) for the block at `prt +0x34` and returns 0
//!   when not given; else it writes `offset / 5` vertices, in bytes (10 a
//!   vertex for stream type `0x39`, else 20), to the header `[prt +0x30]`
//!   at `+4` and `+8`, and returns the header (byte `+0` the type, `+1`
//!   the stride, `+0x10` the D3D vertex buffer).
//!
//! Every draw writes the header's offset before it binds it, so the guest
//! draws that remain see the header as they would have.

use frame_scene::{LayerObjectRefusal, PRT_FIRST_CONSTANT, PrtStream};

use crate::GuestRead;
use crate::layout::decorators::{RESIDENT, RESOURCE_DATA, STREAM_BUFFER, STREAM_STRIDE};
use crate::layout::object_nodes::{RECORD_KEY, TAG_INSTANCES};
use crate::layout::objects::{RECORD_LIGHTING, RECORD_MODEL};

/// The objects' PRT entries: the count, the array and its stride.
pub const ENTRY_COUNT: u32 = 0x0050_93e8;
pub const ENTRIES: u32 = 0x004b_89b0;
pub const ENTRY_SIZE: u32 = 0x68;
/// An entry's detail step (i16), coefficient datum, offsets (sixteen
/// dwords), counts (sixteen bytes) and the vector of `c[68]`.
pub const ENTRY_STEP: u32 = 4;
pub const ENTRY_POOL: u32 = 8;
pub const ENTRY_OFFSETS: u32 = 0xc;
pub const ENTRY_COUNTS: u32 = 0x4c;
pub const ENTRY_VECTOR: u32 = 0x5c;
/// Where a coefficient datum's offset counts from.
pub const POOL_BASE: u32 = 0x0048_7b1c;
/// The floats of `c[69]`, in order.
pub const FIXED_VECTOR: [u32; 4] = [0x0045_dbc0, 0x0045_dc10, 0x0045_e448, 0x0045_e444];
/// A model's PRT block: count and elements.
pub const MODEL_PRT_COUNT: u32 = 0x74;
pub const MODEL_PRT: u32 = 0x78;
/// A PRT block's two shape words (i16), level count, levels, stream
/// header and geometry block.
pub const PRT_SHAPE_COUNT: u32 = 2;
pub const PRT_SHAPE_ORDER: u32 = 4;
pub const PRT_LEVEL_COUNT: u32 = 0x14;
pub const PRT_LEVELS: u32 = 0x18;
pub const PRT_STREAM: u32 = 0x30;
pub const PRT_GEOMETRY: u32 = 0x34;
/// A level: `+4` the count of its pairs, `+8` the pairs (8 bytes).
pub const LEVEL_SIZE: u32 = 0xc;
/// The detail step's ceiling (`5 - step`).
pub const STEPS: i32 = 5;
/// The stream type whose vertices are 10 bytes (else 20).
pub const SHORT_STREAM: u8 = 0x39;
/// The slots of an entry.
const SLOTS: u32 = 16;
/// The constant file's registers (hardware numbering).
const CONSTANTS: i32 = 192;
/// Most entries and pairs read.
const MAX_ENTRIES: u32 = 0x7fff;
const MAX_PAIRS: u32 = 1 << 16;

fn word(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<u32, LayerObjectRefusal> {
    mem.read_u32(base.wrapping_add(offset)).ok_or(LayerObjectRefusal::Unread)
}

fn byte(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<u8, LayerObjectRefusal> {
    let addr = base.wrapping_add(offset);
    Ok((word(mem, addr & !3, 0)? >> (8 * (addr & 3))) as u8)
}

fn short(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<i16, LayerObjectRefusal> {
    let addr = base.wrapping_add(offset);
    if addr & 3 == 3 {
        return Ok(i16::from_le_bytes([byte(mem, addr, 0)?, byte(mem, addr, 1)?]));
    }
    Ok((word(mem, addr & !3, 0)? >> (8 * (addr & 3))) as u16 as i16)
}

fn vector(mem: &dyn GuestRead, at: u32) -> Result<[f32; 4], LayerObjectRefusal> {
    let mut v = [0.0; 4];
    for (n, x) in v.iter_mut().enumerate() {
        *x = f32::from_bits(word(mem, at, 4 * n as u32)?);
    }
    Ok(v)
}

/// The PRT entry of object key `key` (`0x00024040`'s search), `None`
/// without one.
fn entry(mem: &dyn GuestRead, key: u32) -> Result<Option<u32>, LayerObjectRefusal> {
    let count = word(mem, ENTRY_COUNT, 0)? as i32;
    if count > MAX_ENTRIES as i32 {
        return Err(LayerObjectRefusal::Unread);
    }
    for n in 0..count.max(0) as u32 {
        let at = ENTRIES + ENTRY_SIZE * n;
        if word(mem, at, 0)? == key {
            return Ok(Some(at));
        }
    }
    Ok(None)
}

/// What the geometry draw adds for the object item of layer 5 whose
/// render record is at `record` (the module notes): `None` when the
/// record's model has no PRT block, and a named cause when the draw's
/// stream or constants do not read, or are left as the draws before it
/// left them. The caller has checked the shader's flag bit 0
/// (`object_walk::layer_part`).
pub fn read(mem: &dyn GuestRead, record: u32) -> Result<Option<PrtStream>, LayerObjectRefusal> {
    use frame_scene::LayerObjectRefusal::Layer5Stream as Stale;
    let mode = word(mem, record, RECORD_MODEL)?;
    if mode == u32::MAX {
        return Ok(None);
    }
    let table = word(mem, TAG_INSTANCES, 0)?;
    let model = word(mem, table, 16 * (mode & 0xffff) + 8)?;
    if (word(mem, model, MODEL_PRT_COUNT)? as i32) <= 0 {
        return Ok(None);
    }
    let prt = word(mem, model, MODEL_PRT)?;
    let packed = word(mem, record, RECORD_KEY)?;
    let slot = packed & (SLOTS - 1);
    let section = packed >> 9 & 0x1ff;
    let key = word(mem, word(mem, record, RECORD_LIGHTING)?, 0)?;
    let entry = entry(mem, key)?.ok_or(Stale)?;

    // 0x00024040: the constants.
    let count = byte(mem, entry, ENTRY_COUNTS + slot)?;
    if count == 0 {
        return Err(Stale);
    }
    let pool = word(mem, entry, ENTRY_POOL)?;
    if pool as i32 <= -1 {
        return Err(Stale);
    }
    let coefficients = (pool & 0x0fff_ffff).wrapping_add(POOL_BASE);
    let offset = word(mem, entry, ENTRY_OFFSETS + 4 * slot)?;
    let first = coefficients.wrapping_add(offset.wrapping_mul(4));
    let shape = (i32::from(short(mem, prt, PRT_SHAPE_ORDER)?) * 3 + 4).wrapping_mul(i32::from(short(mem, prt, PRT_SHAPE_COUNT)?));
    let n = shape.wrapping_mul(i32::from(count.min(2))) / 4;
    if n < 0 || n > CONSTANTS - i32::from(PRT_FIRST_CONSTANT) - 2 {
        return Err(Stale);
    }
    let mut constants = Vec::with_capacity(2 + n as usize);
    constants.push(vector(mem, entry + ENTRY_VECTOR)?);
    let mut fixed = [0.0; 4];
    for (x, &at) in fixed.iter_mut().zip(&FIXED_VECTOR) {
        *x = f32::from_bits(word(mem, at, 0)?);
    }
    constants.push(fixed);
    for r in 0..n as u32 {
        constants.push(vector(mem, first.wrapping_add(16 * r))?);
    }

    // 0x00024490: the stream.
    let step = i32::from(short(mem, entry, ENTRY_STEP)?);
    let level = (STEPS - step).min((word(mem, prt, PRT_LEVEL_COUNT)? as i32).wrapping_sub(1));
    if level < 0 {
        return Err(Stale);
    }
    let level = word(mem, prt, PRT_LEVELS)?.wrapping_add(LEVEL_SIZE * level as u32);
    let pairs = word(mem, level, 4)? as i32;
    if pairs > MAX_PAIRS as i32 {
        return Err(LayerObjectRefusal::Unread);
    }
    let list = word(mem, level, 8)?;
    let mut vertices = None;
    for n in 0..pairs.max(0) as u32 {
        if word(mem, list, 8 * n)? == section {
            vertices = Some(word(mem, list, 8 * n + 4)? as i32 / 5);
            break;
        }
    }
    let vertices = vertices.ok_or(Stale)?;
    if byte(mem, prt, PRT_GEOMETRY + RESIDENT)? == 0 {
        return Err(Stale);
    }
    let header = word(mem, prt, PRT_STREAM)?;
    let kind = byte(mem, header, 0)?;
    let stride = byte(mem, header, STREAM_STRIDE)?;
    let bytes = vertices.wrapping_mul(if kind == SHORT_STREAM { 10 } else { 20 });
    let buffer = word(mem, header, STREAM_BUFFER)?;
    let data = (0x8000_0000 | (word(mem, buffer, RESOURCE_DATA)? & 0x0fff_ffff)).wrapping_add(bytes as u32);
    Ok(Some(PrtStream { kind, stride, data, constants }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;

    const RECORD: u32 = 0x1000;
    const TABLE: u32 = 0x2000;
    const MODEL: u32 = 0x3000;
    const PRT: u32 = 0x4000;
    const LIGHTING: u32 = 0x5000;
    const LEVELS: u32 = 0x6000;
    const PAIRS: u32 = 0x7000;
    const HEADER: u32 = 0x8000;
    const BUFFER: u32 = 0x9000;
    const KEY: u32 = 0xe000_0042;
    const MODE: u32 = 0xe000_0007;
    /// The word holding the PRT geometry block's resident byte (`+0x52`).
    const RESIDENT_WORD: u32 = PRT + ((PRT_GEOMETRY + RESIDENT) & !3);

    /// A record of section 9, slot 2, whose model has a PRT block of
    /// shape (1, 2) (so `(2·3 + 4)·1·2 / 4` = 5 registers at count 2), and
    /// the second entry naming its object at detail step 3 (level 2).
    fn scene() -> Mem {
        let mut m = Mem::default();
        let entry = ENTRIES + ENTRY_SIZE;
        for (at, v) in [
            (RECORD + RECORD_MODEL, MODE),
            (RECORD + RECORD_KEY, 9 << 9 | 2),
            (RECORD + RECORD_LIGHTING, LIGHTING),
            (LIGHTING, KEY),
            (TAG_INSTANCES, TABLE),
            (TABLE + 16 * 7 + 8, MODEL),
            (MODEL + MODEL_PRT_COUNT, 1),
            (MODEL + MODEL_PRT, PRT),
            (PRT, 1 << 16),
            (PRT + 4, 2),
            (PRT + PRT_LEVEL_COUNT, 4),
            (PRT + PRT_LEVELS, LEVELS),
            (PRT + PRT_STREAM, HEADER),
            (RESIDENT_WORD, 1 << 16),
            (LEVELS + 2 * LEVEL_SIZE + 4, 2),
            (LEVELS + 2 * LEVEL_SIZE + 8, PAIRS),
            (PAIRS, 4),
            (PAIRS + 4, 0),
            (PAIRS + 8, 9),
            (PAIRS + 12, 35),
            (HEADER, 0x14 << 8 | 0x2c),
            (HEADER + STREAM_BUFFER, BUFFER),
            (BUFFER + RESOURCE_DATA, 0x0012_0000),
            (ENTRY_COUNT, 2),
            (ENTRIES, 0xe000_0001),
            (entry, KEY),
            (entry + ENTRY_STEP, 3),
            (entry + ENTRY_POOL, 0x1000_0100),
            (entry + ENTRY_OFFSETS + 8, 0x40),
            (entry + ENTRY_COUNTS, 3 << 16),
        ] {
            m.w(at, v);
        }
        for n in 0..4 {
            m.f(entry + ENTRY_VECTOR + 4 * n, n as f32);
            m.f(FIXED_VECTOR[n as usize], 10.0 + n as f32);
        }
        let first = 0x100 + POOL_BASE + 4 * 0x40;
        for n in 0..20 {
            m.f(first + 4 * n, 100.0 + n as f32);
        }
        m
    }

    #[test]
    fn layer_5_reads_the_objects_constants_and_its_sections_stream_at_its_level() {
        let mut m = scene();
        let got = read(&m, RECORD).unwrap().unwrap();
        assert_eq!(got.kind, 0x2c);
        assert_eq!(got.stride, 0x14);
        assert_eq!(got.data, 0x8012_0000 + 35 / 5 * 20, "seven vertices of 20 bytes");
        assert_eq!(got.constants.len(), 2 + 5);
        assert_eq!(got.constants[0], [0.0, 1.0, 2.0, 3.0]);
        assert_eq!(got.constants[1], [10.0, 11.0, 12.0, 13.0]);
        assert_eq!(got.constants[2], [100.0, 101.0, 102.0, 103.0]);
        assert_eq!(got.constants[6], [116.0, 117.0, 118.0, 119.0]);
        // A stream of type 0x39 holds 10 bytes a vertex.
        m.w(HEADER, 0x0a << 8 | u32::from(SHORT_STREAM));
        assert_eq!(read(&m, RECORD).unwrap().unwrap().data, 0x8012_0000 + 7 * 10);
        // A count of 1 halves the registers: (10 · 1) / 4 = 2.
        let entry = ENTRIES + ENTRY_SIZE;
        m.w(entry + ENTRY_COUNTS, 1 << 16);
        assert_eq!(read(&m, RECORD).unwrap().unwrap().constants.len(), 2 + 2);
    }

    #[test]
    fn a_model_without_a_prt_block_adds_nothing() {
        let mut m = scene();
        m.w(MODEL + MODEL_PRT_COUNT, 0);
        assert_eq!(read(&m, RECORD), Ok(None));
        m.w(RECORD + RECORD_MODEL, u32::MAX);
        assert_eq!(read(&m, RECORD), Ok(None), "the structure's record");
    }

    #[test]
    fn a_draw_that_leaves_its_stream_or_constants_as_they_were_is_named() {
        use frame_scene::LayerObjectRefusal::{Layer5Stream, Unread};
        let entry = ENTRIES + ENTRY_SIZE;
        let cases: [(&str, u32, u32); 6] = [
            ("no entry for the object", LIGHTING, 0xe000_0099),
            ("no coefficients for the slot", entry + ENTRY_COUNTS, 0),
            ("no coefficient datum", entry + ENTRY_POOL, u32::MAX),
            ("no pair for the section", PAIRS + 8, 8),
            ("geometry not resident", RESIDENT_WORD, 0),
            ("no levels", PRT + PRT_LEVEL_COUNT, 0),
        ];
        for (why, at, v) in cases {
            let mut m = scene();
            m.w(at, v);
            assert_eq!(read(&m, RECORD), Err(Layer5Stream), "{why}");
        }
        let mut m = scene();
        m.0.remove(&(HEADER + STREAM_BUFFER));
        assert_eq!(read(&m, RECORD), Err(Unread));
    }
}
