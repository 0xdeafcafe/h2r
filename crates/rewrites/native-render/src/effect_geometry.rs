//! Generated ribbons and effect quads. The guest still constructs the
//! vertices: endpoint fading, camera facing, widths, point aging and
//! curve evaluation retain their original owner and instruction order.
//! We recognize only researched builders at the dynamic upload leaf.

use crate::camera::GuestRead;
use crate::particles::Refused;

pub const DYNAMIC_DRAW: u32 = 0x0004_b220;
pub const CONTRAIL_BUILD: u32 = 0x0004_2cf0;
pub const CONTRAIL_SETUP: u32 = 0x0004_3720;
pub const SORTED_DYNAMIC: u32 = 0x0003_5b00;
pub const EFFECT_QUAD_BUILD: u32 = 0x0005_2020;
/// Engine primitive indices are mapped through this live table by the
/// upload helper, not the NV2A's public topology numbers.
pub const PRIMITIVE_TABLE: u32 = 0x0043_fdc0;
const PUSH_WORD_LIMIT: u32 = 0x800;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Contrail,
    EffectQuad,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DynamicCall {
    pub kind: Kind,
    pub esp: u32,
    pub return_address: u32,
    pub builder: u32,
    pub context: u32,
    pub vertices: u32,
    pub stride: u32,
    /// Actual BEGIN_END topology, resolved from the live engine table.
    pub primitive: u32,
}

fn word(mem: &dyn GuestRead, base: u32, offset: u32) -> Result<u32, Refused> {
    mem.read_u32(base.checked_add(offset).ok_or(Refused::AddressOverflow)?)
        .ok_or(Refused::Unreadable)
}

/// `0x0004b220`: ECX stride, stack (mode, engine primitive, vertex count,
/// builder, context). Only mode zero emits; sizes at the helper's push
/// limit and unsupported builders stay on D3D. No guest callbacks run here.
pub fn read_call(mem: &dyn GuestRead, esp: u32, stride: u32) -> Result<DynamicCall, Refused> {
    if word(mem, esp, 4)? != 0 {
        return Err(Refused::Disabled);
    }
    let primitive_index = word(mem, esp, 8)?;
    let vertices = word(mem, esp, 12)?;
    let builder = word(mem, esp, 16)?;
    let context = word(mem, esp, 20)?;
    let kind = match (builder, stride) {
        (CONTRAIL_BUILD, 24) if vertices >= 4 && vertices.is_multiple_of(2) => Kind::Contrail,
        (EFFECT_QUAD_BUILD, 20) if vertices == 4 => Kind::EffectQuad,
        _ => return Err(Refused::Geometry),
    };
    let bytes = vertices
        .checked_mul(stride)
        .ok_or(Refused::AddressOverflow)?;
    if bytes / 4 + 5 >= PUSH_WORD_LIMIT || primitive_index > 10 || context == 0 {
        return Err(Refused::Geometry);
    }
    let primitive = word(mem, PRIMITIVE_TABLE, primitive_index * 4)?;
    // Contrails preserve native strip assembly; effect quad builder copies
    // exactly four vertices, admitted only for a strip/fan/quad topology.
    if !match kind {
        Kind::Contrail => primitive == 6,
        Kind::EffectQuad => matches!(primitive, 6..=8),
    } {
        return Err(Refused::Geometry);
    }
    Ok(DynamicCall {
        kind,
        esp,
        return_address: word(mem, esp, 0)?,
        builder,
        context,
        vertices,
        stride,
        primitive,
    })
}

/// Copy the completed callback output at issue, not before the builder.
/// Captured D3D bytes and this independent read must agree before replacing
/// the primitive. The helper never emits more than its bounded push span.
pub fn read_generated(
    mem: &dyn GuestRead,
    data: u32,
    call: &DynamicCall,
) -> Result<Vec<u8>, Refused> {
    let bytes = call
        .vertices
        .checked_mul(call.stride)
        .ok_or(Refused::AddressOverflow)?;
    if data == 0 || bytes == 0 || !bytes.is_multiple_of(4) || bytes / 4 + 5 >= PUSH_WORD_LIMIT {
        return Err(Refused::Geometry);
    }
    data.checked_add(bytes - 1)
        .ok_or(Refused::AddressOverflow)?;
    let mut out = Vec::with_capacity(bytes as usize);
    for offset in (0..bytes).step_by(4) {
        out.extend(word(mem, data, offset)?.to_le_bytes());
    }
    Ok(out)
}

/// Draw indices, with strips kept as strips so winding, degenerates and
/// flat interpolation retain the guest's primitive assembly.
pub fn indices(call: &DynamicCall) -> Result<Vec<u32>, Refused> {
    if call.vertices > (PUSH_WORD_LIMIT - 6) * 4 / call.stride.max(1) {
        return Err(Refused::Geometry);
    }
    match call.primitive {
        6 if call.vertices >= 3 => Ok((0..call.vertices).collect()),
        7 | 8 if call.kind == Kind::EffectQuad && call.vertices == 4 => Ok(vec![0, 1, 2, 0, 2, 3]),
        _ => Err(Refused::Geometry),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    struct Mem(HashMap<u32, u32>);
    impl GuestRead for Mem {
        fn read_u32(&self, a: u32) -> Option<u32> {
            self.0.get(&a).copied()
        }
    }
    fn memory() -> Mem {
        Mem([
            (0x100, 0x35b81),
            (0x104, 0),
            (0x108, 4),
            (0x10c, 6),
            (0x110, CONTRAIL_BUILD),
            (0x114, 0x300),
            (PRIMITIVE_TABLE + 16, 6),
        ]
        .into_iter()
        .collect())
    }
    #[test]
    fn ribbon_keeps_runtime_topology_and_duplicate_points() {
        let mut mem = memory();
        let c = read_call(&mem, 0x100, 24).unwrap();
        assert_eq!(c.kind, Kind::Contrail);
        assert_eq!(indices(&c).unwrap(), [0, 1, 2, 3, 4, 5]);
        for i in 0..36 {
            mem.0.insert(0x400 + 4 * i, i % 6);
        }
        let data = read_generated(&mem, 0x400, &c).unwrap();
        assert_eq!(data.len(), 144);
        assert_eq!(
            &data[..24],
            &data[24..48],
            "no deduplication, even for identical pairs"
        );
        mem.0.remove(&0x48c);
        assert_eq!(read_generated(&mem, 0x400, &c), Err(Refused::Unreadable));
    }
    #[test]
    fn unowned_builders_odd_pairs_and_live_topology_mismatches_refuse() {
        let mut m = memory();
        m.0.insert(0x110, 0x1234);
        assert_eq!(read_call(&m, 0x100, 24), Err(Refused::Geometry));
        let mut m = memory();
        m.0.insert(0x10c, 5);
        assert_eq!(read_call(&m, 0x100, 24), Err(Refused::Geometry));
        let mut m = memory();
        m.0.insert(PRIMITIVE_TABLE + 16, 4);
        assert_eq!(
            read_call(&m, 0x100, 24),
            Err(Refused::Geometry),
            "engine's 4 is not assumed to be NV2A's 4"
        );
        let mut m = memory();
        m.0.insert(0x10c, 342);
        assert_eq!(
            read_call(&m, 0x100, 24),
            Err(Refused::Geometry),
            "same push limit as guest"
        );
    }
}
