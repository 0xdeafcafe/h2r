//! Dynamic decal callbacks (`0x00044040`) read into an immutable, ordered
//! snapshot. The two sorted-list handlers `0x00041cc0` and `0x000423c0` are
//! object/layer replays, not decals, and deliberately have no hook here.
//!
//! The guest callback is stdcall `(list, unused)`, `ret 8`. Its state wrapper
//! `0x0002c370` selects mode `n` at `0x00043a4f`, then `0x00163cd0` calls the
//! callback for each value in `[0x00547f94]`. The callback selects
//! `[[0x004ea94c] + 4*(n*512 + list)]` and follows `+0x34` links in that
//! order. See docs/graphics/native-decals-research.md.

use std::collections::HashSet;

/// The most record links followed in one callback (a cycle refuses it).
pub const MAX_RECORDS: usize = 4096;
/// The most vertices read for one record (bounds corrupt guest counts).
pub const MAX_QUADS: u32 = 1 << 16;
/// Bytes in one decal record.
pub const RECORD_SIZE: u32 = 0x40;
/// Bytes in one vertex: float3 position and packed D3DCOLOR.
pub const VERTEX_SIZE: usize = 16;

/// Current mode (`0x00043a4f` stores it here).
pub const CURRENT_MODE: u32 = 0x004c_8790;
/// Frame token used to suppress duplicate linked records.
pub const FRAME_TOKEN: u32 = 0x004b_a038;
/// Heads of the per-mode, per-list record chains.
pub const HEADS: u32 = 0x004e_a94c;
/// Decal datum-array header; its records are at `+0x44`.
pub const ARRAY: u32 = 0x004e_a950;
/// Dynamic geometry pool.
pub const GEOMETRY_POOL: u32 = 0x0050_9448;
/// Dynamic geometry buffer header.
pub const GEOMETRY_BUFFER: u32 = 0x0050_9444;
/// Guest tag-cache array.
pub const TAGS: u32 = 0x004e_3b44;
/// Bitmap residency generation.
pub const BITMAP_GENERATION: u32 = 0x004e_6488;
/// Engine rasterizer globals.
pub const RASTERIZER_GLOBALS: u32 = 0x0048_5a80;

/// A dynamic decal pass mode. Modes 0..5 are W-buffered; mode 6 is not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Mode {
    Alpha0 = 0,
    Alpha1 = 1,
    Multiply = 2,
    Darken = 3,
    Maximum = 4,
    Alpha5 = 5,
    Opaque = 6,
}

impl Mode {
    pub const COUNT: usize = 7;

    pub fn from_guest(v: i16) -> Option<Self> {
        Some(match v {
            0 => Self::Alpha0,
            1 => Self::Alpha1,
            2 => Self::Multiply,
            3 => Self::Darken,
            4 => Self::Maximum,
            5 => Self::Alpha5,
            6 => Self::Opaque,
            _ => return None,
        })
    }

    pub fn index(self) -> i16 {
        self as i16
    }

    /// Whether this mode's wrapper selects W depth.
    pub fn w_buffered(self) -> bool {
        self != Self::Opaque
    }
}

/// One vertex as the guest declaration supplies it (`v0` and `v3`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    /// Bytes in guest order (B, G, R, A for a D3DCOLOR).
    pub color: [u8; 4],
}

/// One record's draw in a callback snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    /// Record datum, for comparison identity.
    pub record: u32,
    /// Where the guest writes its duplicate marker after a successful draw.
    pub marker: u32,
    /// Bitmap datum and the signed image selector at record `+0x29`.
    pub bitmap: u32,
    pub selector: i8,
    /// Resolved image index and live texture header (boot fills these).
    pub image: Option<u32>,
    pub texture_header: Option<u32>,
    /// Vertex default `v10` at record `+0x24`.
    pub default_color: [u8; 4],
    /// The record's complete quad list, in guest order.
    pub vertices: Vec<Vertex>,
}

impl Draw {
    pub fn quad_count(&self) -> u32 {
        self.vertices.len() as u32 / 4
    }
}

/// Everything one callback needs, before any guest-visible marker changes.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub mode: Mode,
    pub list: i16,
    pub draws: Vec<Draw>,
    /// The token [`Snapshot::commit`] writes to each draw's marker.
    pub frame: u16,
}

impl Snapshot {
    pub fn quad_count(&self) -> u64 {
        self.draws.iter().map(|d| u64::from(d.quad_count())).sum()
    }

    /// Reproduces the guest's duplicate-marker writes after native admission.
    /// Every address was read while preparing the snapshot; no writes happen
    /// before this point, so a refusal leaves the whole callback to D3D.
    pub fn commit(&self, mem: &mut dyn Memory) {
        for d in &self.draws {
            mem.write_u16(d.marker, self.frame);
        }
    }
}

/// The small guest-memory interface the pure snapshot reader needs.
pub trait Memory {
    fn read_u32(&self, addr: u32) -> Option<u32>;
    fn read_u16(&self, addr: u32) -> Option<u16> {
        Some(
            self.read_bytes(addr, 2)?
                .as_slice()
                .try_into()
                .ok()
                .map(u16::from_le_bytes)?,
        )
    }
    fn read_bytes(&self, addr: u32, len: usize) -> Option<Vec<u8>>;
    fn write_u16(&mut self, addr: u32, value: u16);
}

fn word(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn record_address(records: u32, datum: u32) -> Option<u32> {
    records.checked_add((datum & 0xffff).checked_mul(RECORD_SIZE)?)
}

/// Resolves a bitmap tag's guest header and image count.
pub fn bitmap_header(mem: &dyn Memory, datum: u32) -> Option<(u32, i32)> {
    let tags = mem.read_u32(TAGS)?;
    let tag = mem.read_u32(tags.checked_add((datum & 0xffff).checked_mul(16)? + 8)?)?;
    let count = i32::try_from(mem.read_u32(tag + 0x44)?).ok()?;
    (count > 0).then_some((tag, count))
}

/// Resolves the image `0x000144f0` selects, including its signed-selector
/// fallback rule. Negative remainders and non-2D images return `None`: those
/// take the engine's default bitmap, which native admission leaves to D3D.
pub fn selected_image(mem: &dyn Memory, bitmap: u32, selector: i8) -> Option<(u32, u32)> {
    let (tag, count) = bitmap_header(mem, bitmap)?;
    let selector = i32::from(selector);
    if selector < 0 {
        return None;
    }
    let index = selector % count;
    let image = mem
        .read_u32(tag + 0x48)?
        .checked_add((index as u32).checked_mul(0x74)?)?;
    if i16::from_le_bytes(
        mem.read_bytes(image + 0x0a, 2)?
            .as_slice()
            .try_into()
            .ok()?,
    ) != 0
    {
        return None;
    }
    Some((index as u32, image))
}

/// Reads `list`'s complete callback in guest order. `Err` is a stable refusal
/// reason for the feature's tally; no guest memory is written.
pub fn read_snapshot(mem: &dyn Memory, list: i16) -> Result<Snapshot, &'static str> {
    let mode = Mode::from_guest(i16::from_le_bytes(
        mem.read_bytes(CURRENT_MODE, 2)
            .ok_or("mode")?
            .as_slice()
            .try_into()
            .map_err(|_| "mode")?,
    ))
    .ok_or("mode")?;
    let index = i32::from(mode.index()) * 512 + i32::from(list);
    if index < 0 {
        return Err("list");
    }
    let heads = mem.read_u32(HEADS).ok_or("heads")?;
    let mut datum = mem
        .read_u32(heads.checked_add(4 * index as u32).ok_or("heads")?)
        .ok_or("heads")?;
    if datum == u32::MAX {
        return Ok(Snapshot {
            mode,
            list,
            draws: Vec::new(),
            frame: 0,
        });
    }
    let array = mem.read_u32(ARRAY).ok_or("records")?;
    let records = mem.read_u32(array + 0x44).ok_or("records")?;
    let frame = mem.read_u16(FRAME_TOKEN).ok_or("frame")?;
    let pool = mem.read_u32(GEOMETRY_POOL).ok_or("geometry pool")?;
    let allocations = mem
        .read_u32(pool + 0x64)
        .and_then(|a| mem.read_u32(a + 0x44))
        .ok_or("geometry allocations")?;
    let shift = mem.read_u32(pool + 0x34).ok_or("geometry shift")?;
    if shift >= 32 {
        return Err("geometry shift");
    }
    let buffer = mem
        .read_u32(GEOMETRY_BUFFER)
        .and_then(|b| mem.read_u32(b + 4))
        .ok_or("geometry buffer")?
        | 0x8000_0000;
    let mut draws = Vec::new();
    let mut seen = HashSet::new();
    for _ in 0..MAX_RECORDS {
        if !seen.insert(datum) {
            return Err("record cycle");
        }
        let record = record_address(records, datum).ok_or("record")?;
        let linked = record_address(records, mem.read_u32(record + 0x38).ok_or("marker")?)
            .ok_or("marker")?;
        let marker = linked + 8;
        if mem.read_u16(marker) == Some(frame) {
            datum = mem.read_u32(record + 0x34).ok_or("record link")?;
            if datum == u32::MAX {
                return Ok(Snapshot {
                    mode,
                    list,
                    draws,
                    frame,
                });
            }
            continue;
        }
        let quads = i16::from_le_bytes(
            mem.read_bytes(record + 0x2a, 2)
                .ok_or("quad count")?
                .as_slice()
                .try_into()
                .map_err(|_| "quad count")?,
        );
        if quads < 0 || quads as u32 > MAX_QUADS {
            return Err("quad count");
        }
        let allocation = mem.read_u32(record + 4).ok_or("geometry allocation")? & 0xffff;
        let packed = mem
            .read_u32(
                allocations
                    .checked_add(allocation.checked_mul(24).ok_or("geometry allocation")? + 8)
                    .ok_or("geometry allocation")?,
            )
            .ok_or("geometry allocation")?;
        let offset = packed.checked_shl(shift).ok_or("geometry offset")?;
        let address = buffer.checked_add(offset).ok_or("geometry address")?;
        let bytes = mem
            .read_bytes(address, quads as usize * 4 * VERTEX_SIZE)
            .ok_or("vertices")?;
        let vertices = bytes
            .as_chunks::<VERTEX_SIZE>()
            .0
            .iter()
            .map(|v| Vertex {
                position: [
                    f32::from_bits(word(&v[0..4]).expect("vertex word")),
                    f32::from_bits(word(&v[4..8]).expect("vertex word")),
                    f32::from_bits(word(&v[8..12]).expect("vertex word")),
                ],
                color: v[12..16].try_into().expect("vertex colour"),
            })
            .collect();
        draws.push(Draw {
            record: datum,
            marker,
            bitmap: mem.read_u32(record + 0x2c).ok_or("bitmap")?,
            selector: mem.read_bytes(record + 0x29, 1).ok_or("image selector")?[0] as i8,
            image: None,
            texture_header: None,
            default_color: mem
                .read_bytes(record + 0x24, 4)
                .ok_or("vertex default")?
                .as_slice()
                .try_into()
                .map_err(|_| "vertex default")?,
            vertices,
        });
        datum = mem.read_u32(record + 0x34).ok_or("record link")?;
        if datum == u32::MAX {
            return Ok(Snapshot {
                mode,
                list,
                draws,
                frame,
            });
        }
    }
    Err("record limit")
}

/// The vertex program selected by engine slot 3 at `0x00043fe8`: microcode
/// bytes with their four-byte header, and the signed start slot.
pub fn vertex_program(mem: &dyn Memory) -> Option<(Vec<u8>, i32)> {
    let globals = mem.read_u32(RASTERIZER_GLOBALS)?;
    let table = mem.read_u32(globals + 0x5c)?;
    let datum = mem.read_u32(table + 3 * 8 + 4)?;
    let tags = mem.read_u32(TAGS)?;
    let tag = mem.read_u32(tags.checked_add((datum & 0xffff).checked_mul(16)? + 8)?)?;
    let shader = mem.read_u32(tag + 8)?;
    let function = mem.read_u32(shader + 0x10)?;
    let start = (mem.read_u32(shader + 0x0c)? as i32) >> 4;
    let header = mem.read_bytes(function, 4)?;
    let count = u16::from_le_bytes(header[2..4].try_into().ok()?) as usize;
    if count == 0 || count > 136 || start < 0 || start as usize + count > 136 {
        return None;
    }
    Some((mem.read_bytes(function, 4 + 16 * count)?, start))
}

/// Triangle indices for a quad list, as the D3D path expands type 8.
pub fn quad_indices(quads: u32) -> Vec<u32> {
    (0..quads).flat_map(|q| {
        let b = q * 4;
        [b, b + 1, b + 2, b, b + 2, b + 3]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Mem {
        words: HashMap<u32, u32>,
        bytes: HashMap<u32, u8>,
        writes: Vec<(u32, u16)>,
    }

    impl Mem {
        fn w(&mut self, at: u32, value: u32) {
            self.words.insert(at, value);
            for (i, b) in value.to_le_bytes().into_iter().enumerate() {
                self.bytes.insert(at + i as u32, b);
            }
        }

        fn h(&mut self, at: u32, value: u16) {
            for (i, b) in value.to_le_bytes().into_iter().enumerate() {
                self.bytes.insert(at + i as u32, b);
            }
        }

        fn b(&mut self, at: u32, value: u8) {
            self.bytes.insert(at, value);
        }
    }

    impl Memory for Mem {
        fn read_u32(&self, addr: u32) -> Option<u32> {
            self.words.get(&addr).copied()
        }

        fn read_u16(&self, addr: u32) -> Option<u16> {
            Some(u16::from_le_bytes([
                *self.bytes.get(&addr)?,
                *self.bytes.get(&(addr + 1))?,
            ]))
        }

        fn read_bytes(&self, addr: u32, len: usize) -> Option<Vec<u8>> {
            (0..len)
                .map(|i| self.bytes.get(&(addr + i as u32)).copied())
                .collect()
        }

        fn write_u16(&mut self, addr: u32, value: u16) {
            self.h(addr, value);
            self.writes.push((addr, value));
        }
    }

    const RECORDS: u32 = 0x0100_0000;
    const ALLOCS: u32 = 0x0200_0000;

    fn fixture() -> Mem {
        let mut m = Mem::default();
        m.h(CURRENT_MODE, 2);
        m.w(HEADS, 0x1000);
        m.w(0x1000 + 4 * (2 * 512 + 7), 3);
        m.w(ARRAY, 0x2000);
        m.w(0x2000 + 0x44, RECORDS);
        m.h(FRAME_TOKEN, 0x55aa);
        m.w(GEOMETRY_POOL, 0x3000);
        m.w(0x3000 + 0x64, 0x4000);
        m.w(0x4000 + 0x44, ALLOCS);
        m.w(0x3000 + 0x34, 4);
        m.w(GEOMETRY_BUFFER, 0x5000);
        m.w(0x5000 + 4, 0x0600_0000);
        m.w(RECORDS + 3 * RECORD_SIZE + 4, 2);
        m.w(ALLOCS + 2 * 24 + 8, 0x123);
        m.w(RECORDS + 3 * RECORD_SIZE + 0x38, 9);
        m.h(RECORDS + 9 * RECORD_SIZE + 8, 1);
        m.h(RECORDS + 3 * RECORD_SIZE + 0x2a, 1);
        m.w(RECORDS + 3 * RECORD_SIZE + 0x24, 0x4433_2211);
        m.w(RECORDS + 3 * RECORD_SIZE + 0x2c, 0xe000_0042);
        m.b(RECORDS + 3 * RECORD_SIZE + 0x29, 0);
        m.w(RECORDS + 3 * RECORD_SIZE + 0x34, u32::MAX);
        for (i, b) in [1.0f32, 2.0, 3.0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .chain([4, 5, 6, 7])
            .enumerate()
        {
            m.b(0x8600_1230 + i as u32, b);
        }
        m
    }

    #[test]
    fn reads_geometry_and_defers_the_marker() {
        let mut m = fixture();
        let s = read_snapshot(&m, 7).unwrap();
        assert_eq!(s.mode, Mode::Multiply);
        assert_eq!(s.draws.len(), 1);
        assert_eq!(s.draws[0].vertices[0].position, [1.0, 2.0, 3.0]);
        assert_eq!(s.draws[0].vertices[0].color, [4, 5, 6, 7]);
        assert_eq!(s.draws[0].default_color, [0x11, 0x22, 0x33, 0x44]);
        assert!(m.writes.is_empty());
        s.commit(&mut m);
        assert_eq!(m.writes, [(RECORDS + 9 * RECORD_SIZE + 8, 0x55aa)]);
    }

    #[test]
    fn a_duplicate_draw_is_not_copied_or_marked_again() {
        let mut m = fixture();
        m.h(RECORDS + 9 * RECORD_SIZE + 8, 0x55aa);
        let s = read_snapshot(&m, 7).unwrap();
        assert!(s.draws.is_empty());
        s.commit(&mut m);
        assert!(m.writes.is_empty());
    }

    #[test]
    fn a_cycle_and_bad_geometry_refuse_the_whole_callback() {
        let mut m = fixture();
        m.w(RECORDS + 3 * RECORD_SIZE + 0x34, 3);
        assert_eq!(read_snapshot(&m, 7), Err("record cycle"));
        let mut m = fixture();
        m.w(0x3000 + 0x34, 32);
        assert_eq!(read_snapshot(&m, 7), Err("geometry shift"));
    }

    #[test]
    fn selects_only_a_positive_modulo_2d_image() {
        let mut m = Mem::default();
        m.w(TAGS, 0x1000);
        m.w(0x1000 + 0x42 * 16 + 8, 0x2000);
        m.w(0x2000 + 0x44, 3);
        m.w(0x2000 + 0x48, 0x3000);
        m.h(0x3000 + 0x74 + 0x0a, 0);
        assert_eq!(selected_image(&m, 0xe000_0042, 4), Some((1, 0x3074)));
        assert_eq!(selected_image(&m, 0xe000_0042, -1), None);
        m.h(0x3000 + 0x74 + 0x0a, 1);
        assert_eq!(selected_image(&m, 0xe000_0042, 1), None);
    }

    #[test]
    fn quad_list_expansion_matches_d3d() {
        assert_eq!(quad_indices(2), [0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
    }
}
