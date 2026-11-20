//! `pmov` (particle movement). Layout provenance: `docs/formats/effects.md`.
//!
//! A `pmov` is a 0x14-byte root followed by a block of 20-byte movement
//! records, each holding a block of 20-byte `{id, property}` parameters. The
//! property records are the shared function-driven scalars ([`Property`]).
use super::EffectTag;
use super::mem::{Ctx, Property, Rec, TagRef};
use crate::Result;

/// Root record size.
pub const ROOT_SIZE: usize = 0x14;
/// Movement record size (**V**: tiles every instance; **E**: see docs).
pub const MOVEMENT_SIZE: usize = 0x14;
/// Parameter record size: `u32 id` then a 16-byte property (**V**).
pub const PARAMETER_SIZE: usize = 0x14;

/// One `{id, property}` parameter of a movement.
#[derive(Clone, Debug, PartialEq)]
pub struct MovementParameter {
    /// Parameter slot (+0). Observed 0..=8; meaning depends on the movement kind.
    pub id: u32,
    /// The function-driven scalar (+4).
    pub property: Property,
}

/// One movement stage.
#[derive(Clone, Debug, PartialEq)]
pub struct Movement {
    /// Movement kind (+0). Observed 0..=3; unknown values are preserved.
    pub kind: u32,
    /// Parameters (+4 count, +8 address).
    pub parameters: Vec<MovementParameter>,
    /// +0xc, uninterpreted (observed 0, 1, 3, 5, 6, 7, 0x1ff).
    pub unknown_0c: u32,
    /// +0x10, uninterpreted (observed 0, 1, 4, 8, 12, 0x8000).
    pub unknown_10: u32,
}

/// A decoded `pmov` tag.
#[derive(Clone, Debug, PartialEq)]
pub struct ParticleMovement {
    /// +0: reference to another `pmov` (a template/parent); null on 30 of the
    /// disc's distinct instances.
    pub template: TagRef,
    /// +8, uninterpreted bit set (observed 0, 1, 3, 7, 0xf, 0x1f, 0x87, 0xc3).
    pub flags: u32,
    /// +0xc count, +0x10 address.
    pub movements: Vec<Movement>,
}

impl EffectTag for ParticleMovement {
    const GROUP: [u8; 4] = *b"pmov";
    fn decode_at(ctx: &mut Ctx<'_>, address: u32) -> Result<Self> {
        let root = ctx.mem.read(address, ROOT_SIZE)?;
        let r = Rec::exact(&root, ROOT_SIZE)?;
        let mut movements = Vec::new();
        for rec in ctx.block(r.block(0xc), MOVEMENT_SIZE)? {
            let m = Rec::exact(&rec, MOVEMENT_SIZE)?;
            let mut parameters = Vec::new();
            for p in ctx.block(m.block(4), PARAMETER_SIZE)? {
                let p = Rec::exact(&p, PARAMETER_SIZE)?;
                let property = Property::decode(Rec::exact(&p.0[4..], Property::SIZE)?, ctx)?;
                parameters.push(MovementParameter {
                    id: p.u32(0),
                    property,
                });
            }
            movements.push(Movement {
                kind: m.u32(0),
                parameters,
                unknown_0c: m.u32(0xc),
                unknown_10: m.u32(0x10),
            });
        }
        Ok(Self {
            template: r.tag_ref(0),
            flags: r.u32(8),
            movements,
        })
    }
    fn references(&self) -> Vec<(String, TagRef)> {
        vec![("template".into(), self.template)]
    }

    fn functions(&self) -> Vec<(String, &[u8])> {
        let mut v = Vec::new();
        for (i, m) in self.movements.iter().enumerate() {
            for (j, p) in m.parameters.iter().enumerate() {
                v.push((
                    format!("movements[{i}].parameters[{j}]"),
                    p.property.data.as_slice(),
                ));
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::SliceMemory;
    use map::{FourCC, TagId};

    fn le(v: &mut Vec<u8>, x: u32) {
        v.extend(x.to_le_bytes());
    }
    fn prop(v: &mut Vec<u8>, size: u32, addr: u32) {
        v.extend([0, 0, 2, 0, 1, 0, 3, 0]);
        le(v, size);
        le(v, addr);
    }
    /// Root + 2 movements (kind 0 with 2 parameters, kind 9 with none).
    fn build(base: u32) -> (SliceMemory, u32) {
        let mut mem = SliceMemory::new(base);
        let data = mem.push(&[1, 2, 3, 4]);
        let mut params = Vec::new();
        for id in 0..2 {
            le(&mut params, id);
            prop(&mut params, 4, data);
        }
        let mut moves = Vec::new();
        le(&mut moves, 0);
        le(&mut moves, 2);
        le(&mut moves, 0); // patched below
        le(&mut moves, 7);
        le(&mut moves, 0);
        le(&mut moves, 0x77);
        le(&mut moves, 0);
        le(&mut moves, 0);
        le(&mut moves, 5);
        le(&mut moves, 8);
        let moves_at = mem.push(&moves);
        assert_eq!(moves_at, base + 4);
        let params_real = mem.push(&params);
        // patch the first movement's parameter pointer
        let off = (moves_at - base) as usize + 8;
        mem.bytes[off..off + 4].copy_from_slice(&params_real.to_le_bytes());
        let mut root = Vec::new();
        root.extend(u32::from_be_bytes(*b"pmov").to_le_bytes());
        le(&mut root, 0xe1000001);
        le(&mut root, 0x83);
        le(&mut root, 2);
        le(&mut root, moves_at);
        let at = mem.push(&root);
        (mem, at)
    }

    #[test]
    fn decodes_valid_tag() {
        let (mut mem, at) = build(0x8000_0000);
        let t = ParticleMovement::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert_eq!(t.flags, 0x83);
        assert_eq!(t.template.group, FourCC(*b"pmov"));
        assert_eq!(t.template.id, TagId(0xe1000001));
        assert_eq!(t.movements.len(), 2);
        assert_eq!(t.movements[0].parameters.len(), 2);
        assert_eq!(t.movements[0].parameters[1].id, 1);
        assert_eq!(t.movements[0].parameters[0].property.data, vec![1, 2, 3, 4]);
        assert_eq!(t.movements[1].kind, 0x77, "unknown kinds are preserved");
        assert!(t.movements[1].parameters.is_empty());
        assert_eq!(t.references().len(), 1);
    }

    #[test]
    fn empty_block_ignores_its_address() {
        let mut mem = SliceMemory::new(0x1000);
        let mut root = vec![0xff; 8];
        le(&mut root, 0);
        le(&mut root, 0);
        le(&mut root, 0xdead_beef);
        let at = mem.push(&root);
        let t = ParticleMovement::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert!(t.movements.is_empty());
        assert!(t.template.is_null());
    }

    #[test]
    fn aliased_blocks_may_live_before_the_root() {
        // The same bytes decode identically wherever the shared block sits.
        let (mut mem, at) = build(0x2000);
        let again = {
            let copy = mem.bytes[(at - 0x2000) as usize..].to_vec();
            mem.push(&copy)
        };
        let a = ParticleMovement::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        let b = ParticleMovement::decode_at(&mut Ctx::new(&mut mem), again).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn out_of_bounds_block_is_an_error() {
        let mut mem = SliceMemory::new(0x1000);
        let mut root = vec![0; 8];
        le(&mut root, 0);
        le(&mut root, 3);
        le(&mut root, 0x9000);
        let at = mem.push(&root);
        assert!(ParticleMovement::decode_at(&mut Ctx::new(&mut mem), at).is_err());
        // truncated root
        assert!(ParticleMovement::decode_at(&mut Ctx::new(&mut mem), at + 4).is_err());
    }

    #[test]
    fn hostile_counts_exhaust_the_budget() {
        let mut mem = SliceMemory::new(0x1000);
        let mut root = vec![0; 8];
        le(&mut root, 0);
        le(&mut root, 0x00ff_ffff);
        le(&mut root, 0x1000);
        let at = mem.push(&root);
        let mut ctx = Ctx::new(&mut mem);
        assert!(ParticleMovement::decode_at(&mut ctx, at).is_err());
    }
}
