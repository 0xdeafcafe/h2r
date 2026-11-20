//! The render palette at 0x00487b1c, filled in the guest's register-row
//! order. A pose update or a cache relocation needs the original setup.

use native_render::model::{NODE_MATRIX_SIZE, NodeMatrix};
use native_render::object_nodes::{BLOCK_MATRICES, MATRIX_STRIDE, NODE_BLOCKS};

use super::residency;
use super::staged::Staged;
use super::*;

pub(super) const PALETTE_CURSOR: u32 = NODE_BLOCKS - 4;
const PALETTE_LIMIT: u32 = 0x27000;
type Matrix = [f32; 12];

pub(super) fn size(m: &dyn GuestMemory, model: u32, lod: u8, sections: &[u8]) -> Result<u32> {
    let base = if single(m, model)? {
        1
    } else {
        let nodes = m.u32(at(model, 0x48)?)?;
        let weights = blend_group(m, model, lod)?.0;
        if nodes > 127 || weights > 255 {
            return Err(Refusal::Palette);
        }
        nodes + weights
    };
    let total = sections
        .iter()
        .try_fold(base, |count, &index| -> Result<u32> {
            if index == u8::MAX {
                return Ok(count);
            }
            let section = section_addr(m, model, u32::from(index))?;
            Ok(count + u32::from(m.u8(at(section, 0x25)?)?))
        })?;
    if total == 0 || total > u32::from(u16::MAX) {
        return Err(Refusal::Palette);
    }
    Ok(total)
}

pub(super) fn reserve(m: &mut Staged<'_>, count: u32) -> Result<Option<u32>> {
    let start = m.u32(PALETTE_CURSOR)?;
    let bytes = count * MATRIX_STRIDE + BLOCK_MATRICES;
    let end = at(start, bytes.next_multiple_of(4))?;
    if end > PALETTE_LIMIT {
        return Ok(None);
    }
    m.set_u32(PALETTE_CURSOR, end)?;
    Ok(Some(start))
}

pub(super) fn section_addr(m: &dyn GuestMemory, model: u32, index: u32) -> Result<u32> {
    if index >= m.u32(at(model, 0x24)?)? {
        return Err(Refusal::ObjectModel);
    }
    at(ptr(m, at(model, 0x28)?)?, index * 0x5c)
}

fn single(m: &dyn GuestMemory, model: u32) -> Result<bool> {
    Ok(m.u8(at(model, 4)?)? & 4 != 0)
}

fn blend_group(m: &dyn GuestMemory, model: u32, lod: u8) -> Result<(u32, u32)> {
    if lod > 5 {
        return Err(Refusal::ObjectModel);
    }
    let index = m.u8(at(model, 0x3c + u32::from(lod))?)?;
    if index > i8::MAX as u8 || u32::from(index) >= m.u32(at(model, 0x34)?)? {
        return Err(Refusal::ObjectModel);
    }
    let group = at(ptr(m, at(model, 0x38)?)?, u32::from(index) * 12)?;
    Ok((m.u32(at(group, 4)?)?, m.u32(at(group, 8)?)?))
}

fn matrix(m: &dyn GuestMemory, address: u32) -> Result<NodeMatrix> {
    let mut bytes = [0u8; NODE_MATRIX_SIZE];
    m.read(address, &mut bytes)?;
    let values =
        std::array::from_fn(|i| f32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap()));
    let matrix = NodeMatrix::from_words(&values);
    if !matrix.is_finite() {
        return Err(Refusal::Palette);
    }
    Ok(matrix)
}

fn compose(pose: NodeMatrix, rest: NodeMatrix) -> Matrix {
    let scale = pose.scale * rest.scale;
    let rotate = |[x, y, z]: [f32; 3], axis| {
        x * pose.forward[axis] + (y * pose.left[axis] + z * pose.up[axis])
    };
    std::array::from_fn(|i| {
        let axis = i / 4;
        match i % 4 {
            0 => rotate(rest.forward, axis) * scale,
            1 => rotate(rest.left, axis) * scale,
            2 => rotate(rest.up, axis) * scale,
            _ => {
                let [x, y, z] = rest.position;
                let local = (x * pose.forward[axis] + y * pose.left[axis]) + z * pose.up[axis];
                local * pose.scale + pose.position[axis]
            }
        }
    })
}

fn blend(m: &dyn GuestMemory, matrix_rows: &[Matrix], entry: u32) -> Result<Matrix> {
    let mut result = [0.0; 12];
    let mut sum = 0.0;
    for i in 0..4 {
        let index = m.u8(at(entry, i)?)?;
        if index == u8::MAX {
            break;
        }
        let row = matrix_rows.get(index as usize).ok_or(Refusal::Palette)?;
        let scale = if i == 3 {
            m.f32(ONE)? - sum
        } else {
            m.f32(at(entry, 4 + i * 4)?)?
        };
        if !scale.is_finite() {
            return Err(Refusal::Palette);
        }
        for (value, component) in result.iter_mut().zip(row) {
            *value = *component * scale + *value;
        }
        sum = scale + sum;
    }
    Ok(result)
}

pub(super) fn object_pose(
    m: &dyn GuestMemory,
    model: u32,
    body: u32,
    slot: u32,
    hlmt: u32,
    sections: &[u8],
) -> Result<u32> {
    if m.u16(at(body, 0x112)?)? != u16::MAX && m.u8(at(body, 0xb3)?)? == 0 {
        // The animation helper can clear the dirty bit and evaluate a pose.
        // Otherwise it returns the node pointer it was passed.
        let flags = m.u32(at(body, 4)?)?;
        if flags & (1 << 30) != 0 && flags & (1 << 29) == 0 {
            for &section in sections.iter().filter(|&&index| index != u8::MAX) {
                let index = u32::from(section);
                if m.u32(at(hlmt, 0xc4 + (index / 32) * 4)?)? & (1 << (index % 32)) != 0 {
                    return Err(Refusal::Palette);
                }
            }
        }
        if m.u8(at(slot, 3)?)? <= 1 && animated_actor(m, body)? {
            return Err(Refusal::Palette);
        }
    }
    let nodes = m.u32(at(model, 0x48)?)?;
    let position = m.u16(at(body, 0x116)?)? as i16;
    let size = m.u16(at(body, 0x114)?)? as i16;
    if position < 0
        || size < 0
        || nodes == 0
        || nodes > 127
        || nodes * NODE_MATRIX_SIZE as u32 > size as u32
    {
        return Err(Refusal::Palette);
    }
    at(body, position as u32)
}

// These two conditions select the actor pose overlay helpers. If neither
// applies, the cached node matrices remain the input to the palette.
fn animated_actor(m: &dyn GuestMemory, body: u32) -> Result<bool> {
    let offset = m.u16(at(body, 0x33e)?)? as i16;
    if offset < 0 {
        return Err(Refusal::Palette);
    }
    let actor = at(body, offset as u32)?;
    if m.u16(at(actor, 0x36)?)? != u16::MAX
        && ((1..7).contains(&m.u8(at(actor, 2)?)?)
            || m.f32(at(actor, 0x40)?)? >= m.f32(0x0045_dbdc)?)
    {
        return Ok(true);
    }
    if m.u16(at(actor, 0x5e)?)? != u16::MAX {
        let channels = m.bytes(at(actor, 0x6e)?, 13)?;
        return Ok(channels.into_iter().any(|channel| channel != 0));
    }
    Ok(false)
}

pub(super) fn fill(
    m: &mut Staged<'_>,
    model: u32,
    lod: u8,
    sections: &[u8],
    pose: u32,
    offset: u32,
    reserved: u32,
) -> Result<()> {
    let nodes = m.u32(at(model, 0x48)?)?;
    if nodes == 0 || nodes > 127 {
        return Err(Refusal::Palette);
    }
    let rest = ptr(m, at(model, 0x4c)?)?;
    let (blends, blends_addr) = blend_group(m, model, lod)?;
    if blends > 255 || (blends != 0 && blends_addr == 0) {
        return Err(Refusal::Palette);
    }
    let mut rows = Vec::with_capacity((nodes + blends) as usize);
    for i in 0..nodes {
        let pose_node = matrix(m, at(pose, i * NODE_MATRIX_SIZE as u32)?)?;
        let rest_node = matrix(m, at(rest, i * 0x60 + 0x28)?)?;
        rows.push(compose(pose_node, rest_node));
    }
    for i in 0..blends {
        rows.push(blend(m, &rows, at(blends_addr, i * 16)?)?);
    }

    let single = single(m, model)?;
    let mut final_rows = if single {
        rows[..1].to_vec()
    } else {
        rows.clone()
    };
    let addr = at(NODE_BLOCKS, offset)?;
    m.set_u16(addr, final_rows.len() as u16)?;
    m.set_u16(at(addr, 2)?, sections.len() as u16)?;
    for (region, &index) in sections.iter().enumerate() {
        let pair = at(addr, 4 + region as u32 * 4)?;
        if index == u8::MAX {
            m.set_u32(pair, 0)?;
            continue;
        }
        let section = section_addr(m, model, u32::from(index))?;
        if residency::resource_cache_in_use(m)? && !residency::touch(m, at(section, 0x38)?, false)?
        {
            return Err(Refusal::Residency);
        }
        let geometry = ptr(m, at(section, 0x34)?)?;
        let remap = m.i32(at(geometry, 0x64)?)?;
        if remap > 0 {
            if final_rows.len() + remap as usize > reserved as usize {
                return Err(Refusal::Palette);
            }
            let map = ptr(m, at(geometry, 0x68)?)?;
            m.set_u16(pair, final_rows.len() as u16)?;
            m.set_u16(at(pair, 2)?, remap as u16)?;
            for j in 0..remap as u32 {
                let index = m.u8(at(map, j)?)? as usize;
                let source = if single { &rows } else { &final_rows };
                let row = *source.get(index).ok_or(Refusal::Palette)?;
                final_rows.push(row);
            }
        } else {
            let count = m.u8(at(section, 0x24)?)?;
            m.set_u16(pair, 0)?;
            m.set_u16(
                at(pair, 2)?,
                if count == 0 {
                    nodes.min(40) as u16
                } else {
                    u16::from(count)
                },
            )?;
        }
    }
    if final_rows.len() > reserved as usize || final_rows.iter().flatten().any(|v| !v.is_finite()) {
        return Err(Refusal::Palette);
    }
    let bytes: Vec<_> = final_rows
        .into_iter()
        .flatten()
        .flat_map(f32::to_le_bytes)
        .collect();
    m.write(at(addr, BLOCK_MATRICES)?, &bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_composes_the_inverse_bind_in_register_row_order() {
        let pose = NodeMatrix {
            scale: 2.0,
            position: [5.0, 6.0, 7.0],
            ..NodeMatrix::IDENTITY
        };
        let inverse = NodeMatrix {
            scale: 0.5,
            position: [-1.0, 2.0, 3.0],
            ..NodeMatrix::IDENTITY
        };
        assert_eq!(
            compose(pose, inverse),
            [1.0, 0.0, 0.0, 3.0, 0.0, 1.0, 0.0, 10.0, 0.0, 0.0, 1.0, 13.0]
        );
    }
}
