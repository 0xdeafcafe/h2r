//! The live section, material pass and node palette at the sky model's
//! indexed-strip call (0x0004c976, inside 0x0004c690). Each call reads
//! the node block prepared for this view by 0x0003ede0.
use super::decorators::SCENARIO;
use super::object_nodes::{
    BLOCK_MATRICES, BLOCK_PARTS, Compression, MATRIX_STRIDE, MAX_PALETTE, MODEL_SECTIONS, Row,
    SECTION_NODE, SECTION_STRIDE, TAG_INSTANCES, decompression_rows,
};
use super::world_layers::SKY_INDEXED_RETURN;
use crate::GuestRead;

const SKY_INDEX: u32 = 0x004b_9eec;
const ACTIVE_NODE_BLOCK: u32 = 0x0048_5a5c;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Site,
    Geometry,
    Material,
    Transform,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkyItem {
    pub model: u32,
    pub section: u32,
    pub part: u32,
    pub shader: u32,
    pub pass: u32,
    pub rows: Vec<Row>,
    pub decompression: [Row; 3],
}

fn field(mem: &dyn GuestRead, base: u32, offset: u32) -> Option<u32> {
    if base == 0 {
        return None;
    }
    mem.read_u32(base.checked_add(offset)?)
}

fn tag(mem: &dyn GuestRead, table: u32, datum: u32) -> Option<u32> {
    if datum == u32::MAX {
        return None;
    }
    field(mem, table, (datum & 0xffff) * 16 + 8)
}

impl SkyItem {
    /// At the entry to DrawIndexedVertices. EBP is the pass ordinal
    /// pushed to 0x0001bbf0; layer 15, variant zero.
    pub fn read(mem: &dyn GuestRead, esp: u32, pass: u32) -> Result<Self, Refusal> {
        use Refusal::*;
        let stack = |offset| field(mem, esp, offset);
        // Four extra stack words for the return and indexed-draw arguments
        // relative to the locals in 0x0004c760..0x0004c976.
        if stack(0) != Some(SKY_INDEXED_RETURN) || stack(0xcc) != Some(0x0003_eeb1) {
            return Err(Site);
        }
        let table = mem.read_u32(TAG_INSTANCES).ok_or(Geometry)?;
        let scenario = mem.read_u32(SCENARIO).ok_or(Geometry)?;
        let sky_index = u32::from(mem.read_u32(SKY_INDEX).ok_or(Geometry)? as u16);
        if sky_index >= field(mem, scenario, 8).ok_or(Geometry)? || sky_index > i16::MAX as u32 {
            return Err(Geometry);
        }
        let skies = field(mem, scenario, 12).ok_or(Geometry)?;
        let sky = tag(
            mem,
            table,
            field(mem, skies, sky_index * 8 + 4).ok_or(Geometry)?,
        )
        .ok_or(Geometry)?;
        let model = field(mem, sky, 4).ok_or(Geometry)?;
        let model_data = tag(mem, table, model).ok_or(Geometry)?;
        if stack(0x28) != Some(model_data) {
            return Err(Geometry);
        }
        let sections = field(mem, model_data, MODEL_SECTIONS).ok_or(Geometry)?;
        let section_ptr = stack(0x24).ok_or(Geometry)?;
        let offset = section_ptr.checked_sub(sections).ok_or(Geometry)?;
        let section = offset / SECTION_STRIDE;
        if offset % SECTION_STRIDE != 0
            || section >= field(mem, model_data, MODEL_SECTIONS - 4).ok_or(Geometry)?
        {
            return Err(Geometry);
        }
        let part = stack(0x38).ok_or(Geometry)?;
        let material = stack(0x40).ok_or(Material)?;
        let shader = field(mem, material, 12)
            .filter(|v| *v != u32::MAX)
            .ok_or(Material)?;
        let region = stack(0x30).ok_or(Transform)?;
        let rows = palette(mem, section_ptr, region).ok_or(Transform)?;
        Ok(Self {
            model,
            section,
            part,
            shader,
            pass,
            rows,
            decompression: section_decompression(mem, model_data, section_ptr).ok_or(Geometry)?,
        })
    }
}

/// 0x0004c91a selects the model's first bounds record. The section flags
/// at +0x1a select position and UV independently for 0x0001cdd0.
fn section_decompression(mem: &dyn GuestRead, model: u32, section: u32) -> Option<[Row; 3]> {
    let mut compression = Compression::default();
    if (field(mem, model, 0x14)? as i32) > 0 {
        let flags = field(mem, section, 0x18)? >> 16;
        let bounds = field(mem, model, 0x18)?;
        // A null bounds pointer makes the upload use the identity.
        if bounds != 0 {
            fn values<const N: usize>(
                mem: &dyn GuestRead,
                base: u32,
                offset: u32,
            ) -> Option<[f32; N]> {
                let mut result = [0.0; N];
                for (i, value) in result.iter_mut().enumerate() {
                    *value = mem.read_f32(base.checked_add(offset + 4 * i as u32)?)?;
                }
                Some(result)
            }
            if flags & 1 != 0 {
                compression.position = Some(values(mem, bounds, 0)?);
            }
            if flags & 2 != 0 {
                compression.texcoord = Some(values(mem, bounds, 24)?);
            }
        }
    }
    let rows = decompression_rows(&compression);
    rows.iter().flatten().all(|v| v.is_finite()).then_some(rows)
}

/// 0x0004c7bc uploads the named node, or calls 0x000151e0 with the
/// region ordinal in ESI for its palette. Matrices are in register order.
fn palette(mem: &dyn GuestRead, section: u32, region: u32) -> Option<Vec<Row>> {
    let node = field(mem, section, SECTION_NODE)? as i16;
    let block = mem.read_u32(ACTIVE_NODE_BLOCK)?;
    let header = field(mem, block, 0)?;
    let (first, count) = if node == -1 {
        if region >= header >> 16 || region >= (BLOCK_MATRICES - BLOCK_PARTS) / 4 {
            return None;
        }
        let slice = field(mem, block, region.checked_mul(4)?.checked_add(BLOCK_PARTS)?)?;
        (u32::try_from(slice as i16).ok()?, slice >> 16)
    } else {
        let index = u32::try_from(node).ok()?;
        if index >= (header & 0xffff) {
            return None;
        }
        (index, 1)
    };
    // The remapped region palettes can follow the base matrices counted by
    // the low half of the header (0x00137b28).
    if count == 0 || count > MAX_PALETTE as u32 {
        return None;
    }
    let start = block
        .checked_add(BLOCK_MATRICES)?
        .checked_add(first * MATRIX_STRIDE)?;
    (0..count * 3)
        .map(|r| {
            let mut row = [0.0; 4];
            for (i, component) in row.iter_mut().enumerate() {
                *component = mem.read_f32(start.checked_add(16 * r + 4 * i as u32)?)?;
                if !component.is_finite() {
                    return None;
                }
            }
            Some(row)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;

    #[test]
    fn strip_resolves_its_model_section_and_material_from_the_active_view() {
        let mut mem = Mem::default();
        let esp = 0x1000;
        let scenario = 0x2000;
        let skies = 0x3000;
        let table = 0x4000;
        let sky = 0x5000;
        let model = 0x6000;
        let sections = 0x7000;
        let section = sections + SECTION_STRIDE;
        let material = 0x8000;
        let block = 0x9000;
        for (at, value) in [
            (esp, SKY_INDEXED_RETURN),
            (esp + 0xcc, 0x0003_eeb1),
            (SCENARIO, scenario),
            (SKY_INDEX, 1),
            (scenario + 8, 2),
            (scenario + 12, skies),
            (skies + 12, 0xab00_0002),
            (TAG_INSTANCES, table),
            (table + 2 * 16 + 8, sky),
            (sky + 4, 0xbc00_0003),
            (table + 3 * 16 + 8, model),
            (esp + 0x28, model),
            (model + 0x14, 0),
            (model + MODEL_SECTIONS, sections),
            (model + MODEL_SECTIONS - 4, 2),
            (esp + 0x24, section),
            (esp + 0x38, 7),
            (esp + 0x40, material),
            (material + 12, 0xcd00_0025),
            (section + SECTION_NODE, 0),
            (esp + 0x30, 0),
            (ACTIVE_NODE_BLOCK, block),
            (block, 1),
        ] {
            mem.w(at, value);
        }
        for i in 0..12 {
            mem.f(block + BLOCK_MATRICES + i * 4, i as f32);
        }
        let read = |m: &Mem| SkyItem::read(m, esp, 3);
        let item = read(&mem).unwrap();
        assert_eq!((item.model, item.section, item.part), (0xbc00_0003, 1, 7));
        assert_eq!((item.shader, item.pass), (0xcd00_0025, 3));
        assert_eq!(item.rows[1], [4.0, 5.0, 6.0, 7.0]);
        mem.w(ACTIVE_NODE_BLOCK, 0);
        assert_eq!(read(&mem), Err(Refusal::Transform));
        mem.w(material + 12, u32::MAX);
        assert_eq!(read(&mem), Err(Refusal::Material));
        mem.w(SKY_INDEX, 2);
        assert_eq!(read(&mem), Err(Refusal::Geometry));
        mem.w(esp + 0xcc, 0);
        assert_eq!(read(&mem), Err(Refusal::Site));
    }

    #[test]
    fn compressed_uvs_use_the_models_bounds_and_leave_positions_alone() {
        let mut mem = Mem::default();
        let model = 0x800;
        let section = 0x1200;
        let bounds = 0x2400;
        mem.w(model + 0x14, 1);
        mem.w(model + 0x18, bounds);
        mem.w(section + 0x18, 2 << 16);
        for (i, v) in [3.0, 7.0, -7.0, 5.0].into_iter().enumerate() {
            mem.f(bounds + 24 + 4 * i as u32, v);
        }
        assert_eq!(
            section_decompression(&mem, model, section),
            Some([[1.0; 4], [0.0; 4], [2.0, 6.0, 5.0, -1.0]])
        );
        mem.w(section + 0x18, 3 << 16);
        assert_eq!(section_decompression(&mem, model, section), None);
    }

    #[test]
    fn current_view_block_and_section_node_select_the_rows() {
        let mut m = Mem::default();
        let section = 0x420;
        for (block, value) in [(0x800, 7.0), (0xc00, 11.0)] {
            m.w(ACTIVE_NODE_BLOCK, block);
            m.w(block, (2 << 16) | 3);
            m.w(section + SECTION_NODE, 2);
            for i in 0..12 {
                m.f(block + BLOCK_MATRICES + 2 * MATRIX_STRIDE + 4 * i, value);
            }
            assert_eq!(palette(&m, section, 1), Some(vec![[value; 4]; 3]));
            m.w(section + SECTION_NODE, 0xffff);
            m.w(block + BLOCK_PARTS + 4, (1 << 16) | 2);
            assert_eq!(palette(&m, section, 1), Some(vec![[value; 4]; 3]));
            m.w(block + BLOCK_PARTS + 4, (2 << 16) | 2);
            assert_eq!(palette(&m, section, 1), None);
        }
    }
}
