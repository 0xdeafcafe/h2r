//! Lightmap writes of the standard BSP geometry handler. Planning reads
//! the render thread's guest memory. It never mutates a descriptor while
//! a GPU reservation can still fail.

use std::collections::BTreeMap;

use guest_layout::memory::GuestMemory;
use super::world_walk::LIGHTMAP;

/// Last page read by the material's lightmap texture binding.
pub const PAGE: u32 = 0x0048_5a60;
/// The palette shortcut is shared by every stage.
pub const LAST_PALETTE: u32 = 0x0046_7000;
pub const PALETTE_COUNT: u32 = 0x0050_93ac;
pub const STAGE_PALETTES: u32 = 0x0048_4dc0;
/// The palette headers `STAGE_PALETTES` indexes, 12 bytes each.
pub const PALETTE_HEADERS: u32 = 0x0048_4dd0;
const STAGE: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamOffset {
    pub descriptor: u32,
    pub bytes: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub page: i32,
    pub palette: i8,
    /// The extra descriptors supplied to the vertex declaration binder.
    pub streams: [Option<StreamOffset>; 2],
}

/// The two cache comparisons and the signed range test in 0x15c90.
pub fn palette_header(requested: i32, last: i32, stage: i32, count: i32) -> Option<u32> {
    (requested >= 0 && requested < count && requested != last && requested != stage)
        .then(|| PALETTE_HEADERS + requested as u32 * 12)
}

fn pointer(mem: &dyn GuestMemory, base: u32, offset: u32) -> Option<u32> {
    let p = mem.u32(base.checked_add(offset)?).ok()?;
    (p != 0).then_some(p)
}

/// The placement's lighting and UV streams. Their storage is shared by
/// every placement referring to this bucket. Both descriptor offsets are
/// overwritten even when another placement already used this descriptor.
fn placement_streams(mem: &dyn GuestMemory, group: u32, index: u32) -> Option<[Option<StreamOffset>; 2]> {
    let at = pointer(mem, group, 0x54)?.checked_add(index.checked_mul(12)?)?;
    let bucket_index = u32::from(mem.u16(at.checked_add(2)?).ok()?);
    let first = u32::from(mem.u16(pointer(mem, at, 8)?).ok()?);
    let bucket = pointer(mem, group, 0x44)?.checked_add(bucket_index * 0x38)?;
    let flags = mem.u8(bucket).ok()?;
    if flags & 3 == 0 { return Some([None, None]); }
    let descriptors = pointer(mem, pointer(mem, bucket, 0x34)?, 4)?;
    descriptors.checked_add(if flags & 2 != 0 { 0x2b } else { 0xb })?;
    let lighting = (flags & 1 != 0).then_some(StreamOffset { descriptor: descriptors, bytes: first * 4 });
    let uv = if flags & 2 != 0 {
        let descriptor = descriptors.checked_add(0x20)?;
        let stride = if mem.u8(descriptor).ok()? == 0x2e { 4 } else { 3 };
        Some(StreamOffset { descriptor, bytes: first * stride })
    } else {
        None
    };
    Some([lighting, uv])
}

/// Decode the writes between geometry selection and vertex declaration
/// setup (0x4d285, 0x4dc60). Ok(None) means no writes. A failed read or a
/// non-BSP record is an error so the caller can retain the guest handler.
pub fn binding(mem: &dyn GuestMemory, record: u32, layer: u32, shader_flags: u16)
    -> Result<Option<Binding>, &'static str>
{
    let flags = mem.u32(record).map_err(|_| "record flags")?;
    if mem.u32(record.checked_add(4).ok_or("record")?).ok() != Some(u32::MAX) {
        return Err("non-BSP record");
    }
    if layer != 3 || flags >> 29 == 3 || shader_flags & 1 != 0 { return Ok(None); }
    let packed = mem.u32(record.checked_add(12).ok_or("record")?).map_err(|_| "record owner")?;
    let lm = pointer(mem, LIGHTMAP, 0).ok_or("active lightmap")?;
    let group = pointer(mem, lm, 0x84).ok_or("first lightmap group")?;
    let placement = (packed >> 18) & 0x7ff;
    let (infos, index, streams) = if placement == 0x7ff {
        (0x2c, (packed >> 9) & 511, [None, None])
    } else {
        (0x4c, placement, placement_streams(mem, group, placement).ok_or("placement lighting streams")?)
    };
    let at = pointer(mem, group, infos).and_then(|p| p.checked_add(index * 4)).ok_or("render info")?;
    let page = mem.u16(at).map_err(|_| "lightmap page")? as i16;
    let palette = mem.u8(at.checked_add(2).ok_or("render info")?).map_err(|_| "lightmap palette")? as i8;
    Ok(Some(Binding { page: i32::from(page), palette, streams }))
}

#[derive(Debug, PartialEq, Eq)]
pub struct Bindings {
    last_palette: i32,
    stage_palette: i32,
    palette_count: i32,
    /// Final value at each written guest address, including aliases.
    offsets: BTreeMap<u32, u32>,
    pub page: Option<i32>,
    /// Calls to D3DDevice_SetPalette in order, including a return to the
    /// original palette. The device acquires and releases resource refs.
    pub palettes: Vec<u32>,
}

impl Bindings {
    pub fn read(mem: &dyn GuestMemory) -> Option<Self> {
        Some(Self {
            last_palette: mem.i32(LAST_PALETTE).ok()?,
            stage_palette: mem.i32(STAGE_PALETTES + STAGE * 4).ok()?,
            palette_count: mem.i32(PALETTE_COUNT).ok()?,
            page: None, offsets: BTreeMap::new(), palettes: Vec::new(),
        })
    }

    /// Fold one sorted handler before folding the next handler.
    pub fn apply(&mut self, binding: Option<Binding>) {
        let Some(binding) = binding else { return };
        for offset in binding.streams.into_iter().flatten() {
            self.offsets.insert(offset.descriptor.wrapping_add(4), offset.bytes);
            self.offsets.insert(offset.descriptor.wrapping_add(8), offset.bytes);
        }
        self.page = Some(binding.page);
        let requested = i32::from(binding.palette);
        if let Some(header) = palette_header(requested, self.last_palette, self.stage_palette, self.palette_count) {
            self.last_palette = requested;
            self.stage_palette = requested;
            self.palettes.push(header);
        }
    }

    /// Guest words after the run. The D3D bindings in `palettes` must
    /// also be applied through the device. With no effective palette
    /// call neither of its cached values should be written.
    pub fn words(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.offsets.iter().map(|(&at, &v)| (at, v))
            .chain(self.page.map(|page| (PAGE, page as u32)))
            .chain((!self.palettes.is_empty()).then_some([
                (LAST_PALETTE, self.last_palette as u32),
                (STAGE_PALETTES + STAGE * 4, self.stage_palette as u32),
            ]).into_iter().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn disjoint_descriptors_and_both_palette_shortcuts_survive_the_run() {
        let mut s = Bindings {
            last_palette: 1, stage_palette: 2, palette_count: 4,
            offsets: BTreeMap::new(), page: None, palettes: Vec::new(),
        };
        let part = |page, palette, descriptor, bytes| Some(Binding {
            page, palette, streams: [Some(StreamOffset { descriptor, bytes }), None],
        });
        s.apply(part(0, 1, 0x100, 32));
        s.apply(part(1, 2, 0x120, 12));
        assert!(s.palettes.is_empty());
        s.apply(part(2, 3, 0x100, 64));
        s.apply(part(7, 2, 0x120, 24));
        s.apply(part(-1, 4, 0x100, 68));
        s.apply(None);
        assert_eq!(s.palettes, [PALETTE_HEADERS + 36, PALETTE_HEADERS + 24]);
        assert_eq!(s.words().collect::<BTreeMap<_, _>>(), BTreeMap::from([
            (0x104, 68), (0x108, 68), (0x124, 24), (0x128, 24),
            (PAGE, u32::MAX), (LAST_PALETTE, 2), (STAGE_PALETTES + 12, 2),
        ]));
    }

    #[test]
    fn cluster_info_is_signed_and_unlit_handlers_leave_the_page_alone() {
        let mut m = FlatMemory::new(LIGHTMAP, 4096);
        let record = LIGHTMAP + 0x100;
        let group = LIGHTMAP + 0x400;
        let infos = LIGHTMAP + 0x600;
        m.set_u32(LIGHTMAP, LIGHTMAP + 0x200).unwrap();
        m.set_u32(LIGHTMAP + 0x284, group).unwrap();
        m.set_u32(group + 0x2c, infos).unwrap();
        m.set_u32(record + 4, u32::MAX).unwrap();
        m.set_u32(record + 12, (0x7ff << 18) | (1 << 9)).unwrap();
        m.set_u16(infos + 4, u16::MAX).unwrap();
        m.set_u8(infos + 6, u8::MAX).unwrap();
        assert_eq!(binding(&m, record, 3, 0), Ok(Some(Binding {
            page: -1, palette: -1, streams: [None; 2],
        })));
        // None of these draws dereferences even the active lightmap.
        m.set_u32(LIGHTMAP, 0).unwrap();
        assert_eq!(binding(&m, record, 1, 0), Ok(None));
        assert_eq!(binding(&m, record, 3, 1), Ok(None));
        m.set_u32(record, 3 << 29).unwrap();
        assert_eq!(binding(&m, record, 3, 0), Ok(None));
        m.set_u32(record, 0).unwrap();
        assert!(binding(&m, record, 3, 0).is_err());
    }

    #[test]
    fn placing_a_bucket_uses_its_coordinate_descriptor_stride() {
        let mut m = FlatMemory::new(0x1000, 4096);
        for (addr, ptr) in [(0x1054, 0x1200), (0x1044, 0x1400), (0x1208, 0x1300),
            (0x1434, 0x1500), (0x1504, 0x1600)] {
            m.set_u32(addr, ptr).unwrap();
        }
        m.set_u16(0x1300, 17).unwrap();
        m.set_u8(0x1400, 3).unwrap();
        let first = Some(StreamOffset { descriptor: 0x1600, bytes: 68 });
        for (kind, bytes) in [(0x2e, 68), (0x2f, 51)] {
            m.set_u8(0x1620, kind).unwrap();
            assert_eq!(placement_streams(&m, 0x1000, 0), Some([first,
                Some(StreamOffset { descriptor: 0x1620, bytes })]));
        }
        m.set_u8(0x1400, 2).unwrap();
        assert_eq!(placement_streams(&m, 0x1000, 0).unwrap()[0], None);
    }
}
