//! The D3D fallback's drawn geometry. The guest walk always selects its
//! retail sections (the sim's choice, whatever the display setting), and a
//! distinct reference copy carries the readbacks. When the guest's own D3D
//! draws render the display directly — no reference copy, so no protected
//! readbacks can see them — the Highest choice swaps only what those draws
//! read: the render record's section, its palette block and the part's
//! index range within the substituted section. The walk, its pose
//! evaluation, the object's live node matrices and every resource request
//! stay exactly as the retail run left them.
//!
//! The swap runs per entry at the sorted geometry handler, after the
//! native objects walk has taken the runs it covers, so it only touches
//! entries the guest is about to draw itself. Everything it writes is
//! per-frame render scratch (the record array and the render palette),
//! rebuilt by the next view's walk. A record whose substituted section
//! needs a different vertex classification or material than the entry was
//! built for, whose geometry is not resident, or whose remap rows are not
//! already in the palette keeps its retail draw.

use std::sync::Arc;

use native_render::light_list::HANDLER_RETURN;
use native_render::object_nodes::{
    BLOCK_MATRICES, MATRIX_STRIDE, MODEL_RECORD, NODE_BLOCKS, RECORD_KEY, RECORD_MODEL,
    RECORD_NODES,
};
use native_render::object_walk::RECORD_SUBPARTS;
use native_render::objects::{RecordOwner, record_owner};
use native_render::skip::{
    CURRENT_LIST, GEOMETRY_BUILD, GEOMETRY_DRAW, ITEM_SIZE, RENDER_RECORD_SIZE, RENDER_RECORDS,
};

use super::setup::{blend_group, finest_lod, permutations, resource_cache_in_use, section_index};
use super::*;

/// The item list header: the item index range `[first, end)` and the item
/// table pointer (the layer draw's sorted list, `object_walk::read`).
const LIST_END: u32 = 4;
const LIST_FIRST: u32 = 0xc;
const LIST_ITEMS: u32 = 0x14;
/// One part element in a section's geometry, and its material index.
const PART_SIZE: u32 = 0x48;
const PART_MATERIAL: u32 = 4;
const SECTION_STRIDE: u32 = 0x5c;

pub(in crate::native_scene_lists) fn install(
    b: &mut Builder,
    scale: ViewLodScale,
    d3d_display: bool,
) {
    if !d3d_display {
        // A distinct reference copy takes the readbacks: the guest's D3D
        // draws are that reference and keep the retail geometry.
        return;
    }
    b.raw()
        .before(
            &points::LIGHT_GEOMETRY_DRAW,
            "display lod geometry swap",
            move |c| {
                if c.addr != points::LIGHT_GEOMETRY_DRAW.base {
                    return Ok(Flow::Continue);
                }
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                // Only the sorted layer-3 entry loop of the main view: the
                // light views' records and every other caller keep their own.
                if mem.u32(c.cpu.esp).ok() == Some(HANDLER_RETURN)
                    && mem.u32(records::CURRENT_VIS).ok() == Some(objects::MAIN_VIS)
                    && scale.display(&*mem) == LodDetail::Highest
                {
                    swap(&mut *mem, c.cpu.esi);
                }
                Ok(Flow::Continue)
            },
        )
        // After the native objects walk: a run it took never reaches the
        // guest handler, so only the entries it left behind are swapped.
        .after_feature(crate::native_objects::ID)
        // The native lights and world layers take their runs at the same
        // draw first, for the same reason.
        .after_feature(crate::native_lights::ID)
        .after_feature(crate::native_world::ID)
        .context_only()
        .optional();
}

/// Swap the geometry one sorted layer-3 entry is about to draw. Every read
/// failure leaves the entry exactly as the retail walk wrote it.
fn swap(m: &mut dyn GuestMemory, entry: u32) {
    let _ = swap_entry(m, entry);
}

fn swap_entry(m: &mut dyn GuestMemory, entry: u32) -> Result<(), Refusal> {
    if entry == 0 {
        return Ok(());
    }
    let list = m.u32(CURRENT_LIST)?;
    if list == 0 {
        return Ok(());
    }
    let index = u32::from(m.u16(entry)?);
    let first = m.u32(at(list, LIST_FIRST)?)?;
    let end = m.u32(at(list, LIST_END)?)?;
    let table = m.u32(at(list, LIST_ITEMS)?)?;
    if first > end || end > m.u32(list)? || table == 0 || !(first..end).contains(&index) {
        return Ok(());
    }
    let item = at(table, index * ITEM_SIZE)?;
    if m.u32(item)? != GEOMETRY_BUILD || m.u32(at(item, 4)?)? != GEOMETRY_DRAW {
        return Ok(());
    }
    let packed = m.u32(at(item, 0xc)?)?;
    if packed >> 24 != 0 {
        return Ok(());
    }
    let Ok(record) = u16::try_from(packed >> 8) else {
        return Ok(());
    };
    if u32::from(record) >= m.u32(records::COUNT)? {
        return Ok(());
    }
    let record_addr = at(RENDER_RECORDS, RENDER_RECORD_SIZE * u32::from(record))?;
    let flags = m.u32(record_addr)?;
    let model_datum = m.u32(at(record_addr, RECORD_MODEL)?)?;
    let handle = m.u32(at(record_addr, RECORD_NODES)?)?;
    let key = m.u32(at(record_addr, RECORD_KEY)?)?;
    if key & 0xe000_0000 != MODEL_RECORD || model_datum == NONE {
        return Ok(());
    }
    let retail_lod = ((flags >> 21) & 0xff) as u8;
    if retail_lod > setup::HIGHEST_LOD {
        return Ok(());
    }

    let model = tag(m, ptr(m, TAGS)?, model_datum)?;
    // A single-node model's remap rows reference composed nodes that the
    // reserved block does not carry.
    if m.u8(at(model, 4)?)? & 4 != 0 {
        return Ok(());
    }
    let nodes = m.u32(at(model, 0x48)?)?;
    if nodes == 0 || nodes > 127 {
        return Ok(());
    }
    let owner = match record_owner(&crate::native_world::Mem(m), record) {
        Some(RecordOwner::Object(owner)) => owner,
        _ => return Ok(()),
    };
    let pool = Pool::read(m, ptr(m, OBJECTS)?, 12)?;
    let body = pool.body(m, owner)?;

    let (regions, current) = permutations(m, model, body, false)?;
    let region = (key & 0xf) as usize;
    let Some(&permutation) = current.get(region) else {
        return Ok(());
    };
    if permutation == u8::MAX {
        return Ok(());
    }
    let region_addr = at(regions, region as u32 * 16)?;
    let section_count = m.u32(at(model, 0x24)?)?;
    let sections = ptr(m, at(model, 0x28)?)?;
    let tier = finest_lod(m, model, body, false)?;
    if tier <= retail_lod {
        return Ok(());
    }
    let target = u32::from(section_index(m, region_addr, permutation, tier)?);
    let current_section = (key >> 9) & 0x1ff;
    if target == current_section || target >= section_count || current_section >= section_count {
        return Ok(());
    }
    let target_at = at(sections, target * SECTION_STRIDE)?;
    let current_at = at(sections, current_section * SECTION_STRIDE)?;
    // The entry's vertex classification was built from the retail section;
    // the substituted section must classify identically.
    if m.u16(at(target_at, 0x18)?)? != m.u16(at(current_at, 0x18)?)?
        || m.u8(at(target_at, 0x14)?)? != m.u8(at(current_at, 0x14)?)?
    {
        return Ok(());
    }
    // The entry's shader and material bindings were chosen for the retail
    // part's material.
    if part_element(m, target_at, region)? != part_element(m, current_at, region)? {
        return Ok(());
    }
    // A substituted section with more subpart words than the retail one
    // would read past the record's allocated mask.
    if flags & RECORD_SUBPARTS != 0
        && m.u16(at(target_at, 0x28)?)? > m.u16(at(current_at, 0x28)?)?
    {
        return Ok(());
    }
    // The geometry must already be resident: the swap never requests I/O
    // and never stamps the resource cache.
    if resource_cache_in_use(m)? && m.u8(at(at(target_at, 0x38)?, 0x1e)?)? == 0 {
        return Ok(());
    }
    let geometry = ptr(m, at(target_at, 0x34)?)?;
    let map_count = m.i32(at(geometry, 0x64)?)?.max(0) as usize;
    let map: Vec<u8> = if map_count > 0 {
        m.bytes(ptr(m, at(geometry, 0x68)?)?, map_count)?
    } else {
        Vec::new()
    };

    // The palette block: copy its rows and append the substituted section's
    // remap, then point the record at the new block. The pose itself is the
    // retail walk's: no pose step runs for the substituted tier.
    if (handle as i32) < 0 {
        return Ok(());
    }
    let block = NODE_BLOCKS.wrapping_add(handle & 0x0fff_ffff);
    let total = u32::from(m.u16(block)?);
    let blends = blend_group(m, model, retail_lod)?.0;
    let base = nodes + blends;
    if total < base || map.iter().any(|&index| u32::from(index) >= base) {
        return Ok(());
    }
    // A rigid section's node must also be one of the block's base rows.
    let rigid = m.u16(at(target_at, 0x2c)?)? as i16;
    if rigid >= 0 && rigid as u32 >= base {
        return Ok(());
    }
    let (pair_first, pair_count, extra) = if map.is_empty() {
        // The section draws the palette's own node rows (palette::fill's
        // empty remap): nothing to append.
        let count = m.u8(at(target_at, 0x24)?)?;
        let count = if count == 0 {
            nodes.min(40)
        } else {
            u32::from(count)
        };
        if count > total {
            return Ok(());
        }
        (0, count, 0)
    } else {
        (total, map.len() as u32, map.len() as u32)
    };
    let new_total = total + extra;
    if new_total > u32::from(u16::MAX) {
        return Ok(());
    }
    let cursor = m.u32(setup::PALETTE_CURSOR)?;
    let bytes = new_total * MATRIX_STRIDE + BLOCK_MATRICES;
    let end = at(cursor, bytes.next_multiple_of(4))?;
    if end > setup::PALETTE_LIMIT {
        return Ok(());
    }
    let mut block_bytes = Vec::with_capacity(bytes as usize);
    block_bytes.extend_from_slice(&(new_total as u16).to_le_bytes());
    block_bytes.extend_from_slice(&m.u16(at(block, 2)?)?.to_le_bytes());
    for i in 0..16 {
        let pair = m.u32(at(block, 4 + i * 4)?)?;
        let pair = if i == region as u32 {
            pair_first | pair_count << 16
        } else {
            pair
        };
        block_bytes.extend_from_slice(&pair.to_le_bytes());
    }
    let rows = at(block, BLOCK_MATRICES)?;
    block_bytes.extend_from_slice(&m.bytes(rows, (total * MATRIX_STRIDE) as usize)?);
    for &index in &map {
        let row = at(rows, u32::from(index) * MATRIX_STRIDE)?;
        block_bytes.extend_from_slice(&m.bytes(row, MATRIX_STRIDE as usize)?);
    }
    m.write(at(NODE_BLOCKS, cursor)?, &block_bytes)?;
    m.set_u32(setup::PALETTE_CURSOR, end)?;
    m.set_u32(at(record_addr, RECORD_NODES)?, cursor)?;
    m.set_u32(
        at(record_addr, RECORD_KEY)?,
        (key & !(0x1ff << 9)) | target << 9,
    )?;
    m.set_u32(record_addr, (flags & !(0xff << 21)) | u32::from(tier) << 21)?;
    Ok(())
}

/// The part element for `region` within a section's part table, by its
/// material index: the entry's material must survive the substitution.
fn part_element(m: &dyn GuestMemory, section: u32, region: usize) -> Result<u16> {
    let geometry = ptr(m, at(section, 0x34)?)?;
    let count = m.u32(geometry)?;
    if count <= region as u32 {
        return Err(Refusal::Layout);
    }
    let parts = ptr(m, at(geometry, 4)?)?;
    let part = at(parts, PART_SIZE * region as u32)?;
    m.u16(at(part, PART_MATERIAL)?).map_err(Refusal::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const ITEM_LIST: u32 = 0x10000;
    const ITEMS: u32 = 0x100a0;
    const ENTRY: u32 = 0x10200;
    const OBJECT_POOL: u32 = 0x11000;
    const OBJECT_SLOTS: u32 = 0x11100;
    const BODY: u32 = 0x12000;
    const HANDLE: u32 = 0x7100_0000;
    const TAG_TABLE: u32 = 0x14000;
    const MODEL: u32 = 0x17000;
    const REGION_ROWS: u32 = 0x17510;
    const SECTIONS: u32 = 0x18000;
    const GEO1: u32 = 0x18400;
    const GEO2: u32 = 0x18500;
    const PARTS1: u32 = 0x18600;
    const PARTS2: u32 = 0x18700;
    const MAP1: u32 = 0x18800;
    const MAP2: u32 = 0x18810;
    const LIGHT_CACHE: u32 = 0x19000;
    const CACHE_ENTRY: u32 = LIGHT_CACHE + 0x100;
    const MODEL_DATUM: u32 = 0x4141_0003;
    /// The retail record: region 0, section 1, group "none", tier 2.
    const FLAGS: u32 = 2 << 21;
    const KEY: u32 = 0x2000_0000 | (0x1f << 4) | (1 << 9);
    /// The retail palette block's size: its header plus three rows.
    const OLD_BLOCK: u32 = BLOCK_MATRICES + 3 * MATRIX_STRIDE;

    fn fixture() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.map(ITEM_LIST, 0xc000)
            .map(NODE_BLOCKS - 4, 0x800)
            .map(RENDER_RECORDS, 2 * RENDER_RECORD_SIZE)
            .map(0x004c_0000, 0x1000)
            .map(0x0046_7000, 0x1000);
        // The sorted item list: one object item naming record 0, part 0.
        m.put_u32(CURRENT_LIST, ITEM_LIST)
            .put_u32(ITEM_LIST, 2)
            .put_u32(ITEM_LIST + 4, 1)
            .put_u32(ITEM_LIST + 0xc, 0)
            .put_u32(ITEM_LIST + 0x14, ITEMS)
            .put_u32(ITEMS, GEOMETRY_BUILD)
            .put_u32(ITEMS + 4, GEOMETRY_DRAW)
            .put_u32(ITEMS + 0xc, 0)
            .put_u16(ENTRY, 0);
        // The render record: retail section 1 at tier 2, region 0.
        m.put_u32(records::COUNT, 1)
            .put_u32(RENDER_RECORDS, FLAGS)
            .put_u32(RENDER_RECORDS + RECORD_MODEL, MODEL_DATUM)
            .put_u32(RENDER_RECORDS + RECORD_NODES, 0)
            .put_u32(RENDER_RECORDS + RECORD_KEY, KEY)
            .put_u32(RENDER_RECORDS + 0x10, CACHE_ENTRY + 0x44);
        // The object pool and its live body, with the region's permutation.
        m.put_u32(OBJECTS, OBJECT_POOL)
            .put_u32(OBJECT_POOL + 0x24, 12)
            .put_u32(OBJECT_POOL + 0x38, 1)
            .put_u32(OBJECT_POOL + 0x44, OBJECT_SLOTS)
            .put_u16(OBJECT_SLOTS, (HANDLE >> 16) as u16)
            .put_u8(OBJECT_SLOTS + 3, 2)
            .put_u32(OBJECT_SLOTS + 8, BODY)
            .put_u16(BODY + 0x118, 10)
            .put_u16(BODY + 0x11a, 0x200)
            .put_u8(BODY + 0x200, 1)
            .put_u32(BODY + 0xcc, 0x7a00_0000);
        // The render-state cache entry that owns the record.
        m.put_u32(0x0050_9434, LIGHT_CACHE)
            .put_u32(LIGHT_CACHE + 0x38, 1)
            .put_u32(LIGHT_CACHE + 0x44, CACHE_ENTRY)
            .put_u32(CACHE_ENTRY + 4, HANDLE);
        // The model tag: one region, three sections, two nodes, no blends.
        m.put_u32(TAGS, TAG_TABLE);
        let entry = TAG_TABLE + (MODEL_DATUM & 0xffff) * 16;
        m.put_u32(entry + 4, MODEL_DATUM).put_u32(entry + 8, MODEL);
        m.put_u32(MODEL + 4, 0)
            .put_u32(MODEL + 0x1c, 1)
            .put_u32(MODEL + 0x20, 0x17500)
            .put_u32(0x17500 + 8, 2)
            .put_u32(0x17500 + 0xc, REGION_ROWS)
            .put_u16(REGION_ROWS + 16 + 4 + 2 * 2, 1)
            .put_u16(REGION_ROWS + 16 + 4 + 2 * 5, 2)
            .put_u32(MODEL + 0x24, 3)
            .put_u32(MODEL + 0x28, SECTIONS)
            .put_u32(MODEL + 0x34, 1)
            .put_u32(MODEL + 0x38, 0x17600)
            .put_u32(0x17600 + 4, 0)
            .put_u32(0x17600 + 8, 0)
            .put_u32(MODEL + 0x48, 2)
            .put_u8(MODEL + 0x3c + 2, 0)
            .put_u8(MODEL + 0x3c + 5, 0);
        // Sections 1 (retail, tier 2) and 2 (finest, tier 5) agree on wire,
        // parts and residency; the finest section's remap is [0, 1]. The
        // residency record is inline at `+0x38`.
        for (index, geometry, parts, _map) in [(1u32, GEO1, PARTS1, MAP1), (2, GEO2, PARTS2, MAP2)] {
            let section = SECTIONS + SECTION_STRIDE * index;
            m.put_u8(section + 0x14, 1)
                .put_u16(section + 0x18, 1)
                .put_u8(section + 0x24, 2)
                .put_u16(section + 0x28, 0)
                .put_u16(section + 0x2c, 0xffff)
                .put_u32(section + 0x34, geometry)
                .put_u8(section + 0x38 + 0x1e, 1);
            m.put_u32(geometry, 1)
                .put_u32(geometry + 4, parts)
                .put_u16(parts + 4, 0x11);
        }
        m.put_u32(0x004e_6494, 1)
            .put_u32(GEO1 + 0x64, 1)
            .put_u32(GEO1 + 0x68, MAP1)
            .put_u8(MAP1, 0)
            .put_u32(GEO2 + 0x64, 2)
            .put_u32(GEO2 + 0x68, MAP2)
            .put_u8(MAP2, 0)
            .put_u8(MAP2 + 1, 1);
        // The retail palette block: two node rows and the retail remap row.
        m.put_u16(NODE_BLOCKS, 3)
            .put_u16(NODE_BLOCKS + 2, 1)
            .put_u32(NODE_BLOCKS + 4, 2 | 1 << 16)
            .put_u32(setup::PALETTE_CURSOR, OLD_BLOCK);
        for (i, value) in [1.0f32, 2.0, 3.0].into_iter().enumerate() {
            for j in 0..12u32 {
                m.put_f32(
                    NODE_BLOCKS + BLOCK_MATRICES + i as u32 * MATRIX_STRIDE + 4 * j,
                    value,
                );
            }
        }
        m
    }

    fn swapped(m: &FakeGuest) -> (u32, u32, u32) {
        (
            m.u32(RENDER_RECORDS + RECORD_KEY).unwrap(),
            m.u32(RENDER_RECORDS + RECORD_NODES).unwrap(),
            m.u32(RENDER_RECORDS).unwrap(),
        )
    }

    fn rows(m: &FakeGuest, block: u32, count: u32) -> Vec<f32> {
        (0..count * 12)
            .map(|i| m.f32(block + BLOCK_MATRICES + 4 * i).unwrap())
            .collect()
    }

    #[test]
    fn the_draw_boundary_swaps_only_the_record_and_the_palette() {
        let mut m = fixture();
        let before = m.clone();
        swap_entry(&mut m, ENTRY).unwrap();
        // The record names the finest section, its tier, and the new block.
        assert_eq!(
            swapped(&m),
            (KEY & !(0x1ff << 9) | 2 << 9, OLD_BLOCK, 5 << 21)
        );
        let block = NODE_BLOCKS + OLD_BLOCK;
        let new_block = BLOCK_MATRICES + 5 * MATRIX_STRIDE;
        assert_eq!(m.u32(setup::PALETTE_CURSOR).unwrap(), OLD_BLOCK + new_block);
        assert_eq!(m.u16(block).unwrap(), 5);
        assert_eq!(m.u16(block + 2).unwrap(), 1);
        assert_eq!(m.u32(block + 4).unwrap(), 3 | 2 << 16);
        for i in 1..16 {
            assert_eq!(m.u32(block + 4 + 4 * i).unwrap(), 0);
        }
        assert_eq!(
            rows(&m, block, 5),
            [1.0f32, 2.0, 3.0, 1.0, 2.0]
                .into_iter()
                .flat_map(|v| [v; 12])
                .collect::<Vec<_>>()
        );
        // The retail block, the body and the record's other fields are
        // untouched: only the drawn geometry moved.
        assert_eq!(
            m.bytes(NODE_BLOCKS, OLD_BLOCK as usize).unwrap(),
            before.bytes(NODE_BLOCKS, OLD_BLOCK as usize).unwrap()
        );
        assert_eq!(
            m.bytes(BODY, 0x300).unwrap(),
            before.bytes(BODY, 0x300).unwrap()
        );
        assert_eq!(m.u32(RENDER_RECORDS + 0x10).unwrap(), CACHE_ENTRY + 0x44);
        assert_eq!(
            m.u32(RENDER_RECORDS + 0x14).unwrap(),
            before.u32(RENDER_RECORDS + 0x14).unwrap()
        );
        // The swap is idempotent: the second draw of the same record keeps it.
        let done = m.clone();
        swap_entry(&mut m, ENTRY).unwrap();
        assert_eq!(swapped(&m), swapped(&done));
        assert_eq!(
            m.bytes(NODE_BLOCKS, 0x400).unwrap(),
            done.bytes(NODE_BLOCKS, 0x400).unwrap()
        );
    }

    #[test]
    fn a_section_without_a_remap_points_at_the_palette_node_rows() {
        let mut m = fixture();
        m.put_u32(GEO2 + 0x64, 0);
        swap_entry(&mut m, ENTRY).unwrap();
        assert_eq!(
            swapped(&m),
            (KEY & !(0x1ff << 9) | 2 << 9, OLD_BLOCK, 5 << 21)
        );
        let block = NODE_BLOCKS + OLD_BLOCK;
        assert_eq!(m.u16(block).unwrap(), 3);
        assert_eq!(m.u32(block + 4).unwrap(), 0 | 2 << 16);
        assert_eq!(
            rows(&m, block, 3),
            [1.0f32, 2.0, 3.0]
                .into_iter()
                .flat_map(|v| [v; 12])
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn entries_the_walk_would_not_admit_keep_their_retail_draw() {
        for (name, edit) in [
            (
                "a non-object item",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u32(ITEMS, 0);
                }) as Box<dyn Fn(&mut FakeGuest)>,
            ),
            (
                "a draw handler item",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u32(ITEMS + 4, 0);
                }),
            ),
            (
                "an item past the list",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u16(ENTRY, 5);
                }),
            ),
            (
                "a record past the count",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u32(records::COUNT, 0);
                }),
            ),
            (
                "a world record",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u32(RENDER_RECORDS + RECORD_MODEL, NONE);
                }),
            ),
            (
                "a negative palette handle",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u32(RENDER_RECORDS + RECORD_NODES, 0x8000_0000);
                }),
            ),
            (
                "a single-node model",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u8(MODEL + 4, 4);
                }),
            ),
            (
                "an owner that does not resolve",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u32(CACHE_ENTRY + 4, 0x7100_0009);
                }),
            ),
            (
                "a record already at the finest tier",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u32(RENDER_RECORDS, 5 << 21);
                }),
            ),
        ] {
            let mut m = fixture();
            edit(&mut m);
            // The case may itself set the handle (the negative one): the
            // record must keep whatever the retail walk left.
            let retail_handle = m.u32(RENDER_RECORDS + RECORD_NODES).unwrap();
            swap_entry(&mut m, ENTRY).unwrap();
            let key = m.u32(RENDER_RECORDS + RECORD_KEY).unwrap();
            let handle = m.u32(RENDER_RECORDS + RECORD_NODES).unwrap();
            let cursor = m.u32(setup::PALETTE_CURSOR).unwrap();
            assert!(
                (key, handle, cursor) == (KEY, retail_handle, OLD_BLOCK),
                "{name}"
            );
        }
    }

    #[test]
    fn a_substitution_the_entry_cannot_draw_keeps_the_retail_draw() {
        for (name, edit) in [
            (
                "a different classification",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u16(SECTIONS + SECTION_STRIDE * 2 + 0x18, 2);
                }) as Box<dyn Fn(&mut FakeGuest)>,
            ),
            (
                "different node influences",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u8(SECTIONS + SECTION_STRIDE * 2 + 0x14, 2);
                }),
            ),
            (
                "a different material",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u16(PARTS2 + 4, 0x12);
                }),
            ),
            (
                "geometry not resident",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u8(SECTIONS + SECTION_STRIDE * 2 + 0x38 + 0x1e, 0);
                }),
            ),
            (
                "a remap beyond the base rows",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u8(MAP2 + 1, 2);
                }),
            ),
            (
                "a rigid node beyond the base rows",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u16(SECTIONS + SECTION_STRIDE * 2 + 0x2c, 2);
                }),
            ),
            (
                "no room left in the palette",
                Box::new(|m: &mut FakeGuest| {
                    m.put_u32(setup::PALETTE_CURSOR, setup::PALETTE_LIMIT - 0x10);
                }),
            ),
        ] {
            let mut m = fixture();
            edit(&mut m);
            // The palette case moves the cursor itself: it must not move again.
            let cursor = m.u32(setup::PALETTE_CURSOR).unwrap();
            swap_entry(&mut m, ENTRY).unwrap();
            assert!(
                swapped(&m) == (KEY, 0, FLAGS) && m.u32(setup::PALETTE_CURSOR).unwrap() == cursor,
                "{name}"
            );
        }
    }
}
