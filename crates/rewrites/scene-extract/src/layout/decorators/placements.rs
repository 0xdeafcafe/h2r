//! The model arm of the decorator placement producer (0x00041640).
//! The view supplies the ordered visible grid cells and their world origins.
//! Each group's BSP cluster indexes the R4 portal windows.

use super::*;
use crate::layout::skip::MAX_ITEMS;
use crate::visibility::frusta::WindowFrusta;

const ENABLED: u32 = 0x004b_a025;
const RADIUS_SCALE: u32 = 0x0045_e4d0;
const LOD_DISTANCES: u32 = 0x0052_5980;
// The projected size argument of the decorator shader selection call.
const PROJECTED_SIZE: u32 = 0x0004_1925;
const SIZE_SCALE: u32 = 0x0048_5b28;
const SIZE_OVERRIDE: u32 = 0x004b_a019;
const OVERRIDE_SCALE: u32 = 0x0045_dc0c;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ModelItem {
    pub origin: [f32; 3],
    pub shader: u32,
    pub argument: u32,
    pub variant: u8,
    pub pass: u8,
    pub mask: u32,
}

impl ModelItem {
    pub(crate) fn matches(&self, mem: &dyn GuestRead, item: u32) -> bool {
        let flags = u32::from(self.pass) | u32::from(self.variant) << 8;
        mem.read_u32(item + 4) == Some(DECORATOR_DRAW)
            && mem.read_u32(item + 8) == Some(self.shader)
            && mem.read_u32(item + 0xc) == Some(self.argument)
            && mem.read_u32(item + 0x10) == Some(self.mask)
            && mem.read_u32(item + 0x14).map(|w| w & 0xffff) == Some(flags)
    }
}

// 0x0001bf50 takes the first threshold the projected size reaches,
// clamping to the last level if the size falls below every threshold.
fn shader_level(mem: &dyn GuestRead, shader: u32) -> Result<(u8, u32), Refusal> {
    let pp = template_postprocess(mem, shader)?;
    let count = word(mem, pp, 0)?;
    if count == 0 || count > 256 {
        return Err(Refusal::Unread);
    }
    let mut size = float(mem, PROJECTED_SIZE, 0)?;
    if byte(mem, SIZE_OVERRIDE, 0)? != 0 {
        size *= float(mem, OVERRIDE_SCALE, 0)?;
    }
    size *= float(mem, SIZE_SCALE, 0)?;
    let levels = word(mem, pp, POSTPROCESS_LODS)?;
    for level in 0..count {
        let at = levels
            .checked_add(level * LOD_SIZE)
            .ok_or(Refusal::Unread)?;
        if level + 1 == count || size >= float(mem, at, 6)? {
            return Ok((level as u8, word(mem, at, 2)?));
        }
    }
    unreachable!()
}

fn in_range(camera: [f32; 3], centre: [f32; 3], radius: f32, far: f32) -> bool {
    let [x, y, z] = [0, 1, 2].map(|i| f64::from(centre[i]) - f64::from(camera[i]));
    ((x * x + y * y) + z * z).sqrt() - f64::from(radius) < f64::from(far)
}

/// Build layer `layer`'s model item keys in append order. Unresolved cache
/// requests, a virtual shader or an out-of-bounds block leave the view to
/// the guest comparison arm.
pub(crate) fn model_items(
    mem: &dyn GuestRead,
    k: &Constants,
    camera: [f32; 3],
    windows: &WindowFrusta,
    layer: u8,
) -> Result<Vec<ModelItem>, Refusal> {
    let mut items = Vec::new();
    if byte(mem, ENABLED, 0)? == 0 {
        return Ok(items);
    }
    if layer >= 32 {
        return Err(Refusal::Unread);
    }
    let bsp = word(mem, ACTIVE_BSP, 0)?;
    let block = word(mem, bsp, BSP_DECORATORS)?;
    let scenario = word(mem, SCENARIO, 0)?;
    let mut visited = 0;
    let mut levels = std::collections::HashMap::new();
    let cells = cells::build(mem, block, camera)?;
    if !cells::matches(mem, &cells)? {
        return Err(Refusal::GridMismatch);
    }
    for (slot, visible) in cells.into_iter().enumerate() {
        let slot = slot as u32;
        let cell = block_record(mem, block, 0x20, visible.index, 0x18)?;
        let count = half(mem, cell, 0x12)? as i16;
        if count <= 0 {
            continue;
        }
        visited += count as u32;
        if visited > MAX_ITEMS {
            return Err(Refusal::Unread);
        }
        let first = word(mem, cell, 0x14)?;
        let end = first
            .checked_add(count as u32)
            .filter(|end| *end <= 1 << 22)
            .ok_or(Refusal::Unread)?;
        let origin = visible.origin.map(|v| v - k.origin_bias);
        for index in first..end {
            let group = block_record(mem, block, BLOCK_PLACEMENTS - 4, index, PLACEMENT_SIZE)?;
            let palette = byte(mem, group, 0)? as i8;
            if palette < 0
                || i32::from(palette) >= word(mem, scenario, SCENARIO_PALETTE - 4)? as i32
                || half(mem, group, PLACEMENT_COUNT)? as i16 <= 0
            {
                continue;
            }
            let entry = block_record(mem, scenario, SCENARIO_PALETTE - 4, palette as u32, 8)?;
            let set = word(mem, entry, 4)?;
            if set == u32::MAX {
                continue;
            }
            let def = definition(mem, set).ok_or(Refusal::Unread)?;
            let class = u32::from(byte(mem, group, 1)?);
            if class as i32 >= word(mem, def, SET_CLASSES - 4)? as i32 {
                continue;
            }
            let class = block_record(mem, def, SET_CLASSES - 4, class, CLASS_SIZE)?;
            if byte(mem, class, 4)? != 0 {
                continue;
            }
            let perm = u32::from(byte(mem, group, 2)?);
            if perm as i32 >= word(mem, class, CLASS_PERMUTATIONS - 4)? as i32 {
                continue;
            }
            let perm = block_record(mem, class, CLASS_PERMUTATIONS - 4, perm, PERMUTATION_SIZE)?;
            let shader = u32::from(byte(mem, perm, 4)?);
            if shader as i32 >= word(mem, def, 0)? as i32 {
                continue;
            }
            let shader = word(mem, block_record(mem, def, 0, shader, 8)?, 4)?;
            if shader == u32::MAX {
                continue;
            }
            let cluster = half(mem, group, 4)?;
            let local = position(k, word(mem, group, 0x14)?);
            let centre = [0, 1, 2].map(|i| origin[i] + local[i]);
            let radius = f32::from(byte(mem, group, 3)?) * float(mem, RADIUS_SCALE, 0)?;
            if !windows.admits(cluster, centre, radius) {
                continue;
            }
            let lod = u32::from(byte(mem, perm, 9)?);
            if lod > 3 {
                return Err(Refusal::Unread);
            }
            if !in_range(camera, centre, radius, float(mem, LOD_DISTANCES, 4 * lod)?) {
                continue;
            }
            let info = block_record(
                mem,
                block,
                BLOCK_GEOMETRY - 4,
                u32::from(half(mem, group, PLACEMENT_GEOMETRY)?),
                GEOMETRY_SIZE,
            )?;
            if !resident(mem, info)? {
                return Err(Refusal::NotResident);
            }
            let (variant, mask) = match levels.entry(shader) {
                std::collections::hash_map::Entry::Occupied(e) => *e.get(),
                std::collections::hash_map::Entry::Vacant(e) => {
                    *e.insert(shader_level(mem, shader)?)
                }
            };
            if mask & (1 << layer) != 0 {
                items.push(ModelItem {
                    origin,
                    shader,
                    argument: lod << 30 | index << 8 | slot,
                    variant,
                    pass: 0,
                    mask,
                });
            }
        }
    }
    Ok(items)
}
