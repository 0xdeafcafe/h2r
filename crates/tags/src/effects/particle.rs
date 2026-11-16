//! `prt3` (particle). Layout provenance: `docs/formats/effects.md`.
//!
//! Root record (0xbc bytes) with: a shader-template reference, a block of
//! 0x28-byte shader parameters (each may reference a bitmap), five
//! function-driven [`Property`] records, two auxiliary references, a block of
//! 4-byte records, a block of nested particle systems (the same 0x38-byte
//! records `effe` events use, see [`EffectParticleSystem`]) and a 0x7c-byte
//! embedded shader post-process record.
use super::EffectTag;
use super::effect::EffectParticleSystem;
use super::mem::{Ctx, Property, Rec, TagRef};
use crate::Result;
use crate::wire::StringId;

pub const ROOT_SIZE: usize = 0xbc;
pub const PARAMETER_SIZE: usize = 0x28;
pub use super::effect::{EMITTER_SIZE, PARTICLE_SYSTEM_SIZE as SYSTEM_SIZE};
pub const POSTPROCESS_SIZE: usize = 0x7c;
/// Number of function-driven properties in the root (+0x1c, 16 bytes each).
pub const PROPERTY_COUNT: usize = 5;

/// A shader parameter (+0x14 block, 0x28 bytes). Most carry a bitmap.
#[derive(Clone, Debug, PartialEq)]
pub struct ShaderParameter {
    /// Parameter name (+0). A string ID; **V**: index/length resolve on every
    /// instance checked.
    pub name: StringId,
    /// +4, observed 0 or 1; meaning unknown.
    pub unknown_04: u32,
    /// +8 bitmap reference (class `bitm`, null for 445 of 5,415 instances).
    pub bitmap: TagRef,
    /// +0x10 scalar (observed 0, 0.5, 1, 5, ...).
    pub value: f32,
    /// +0x14 three scalars (colour-like; usually zero).
    pub color: [f32; 3],
    /// +0x20 count and +0x24 address of a block that is empty on all disc
    /// data; kept raw.
    pub unknown_20: [u32; 2],
}

/// A decoded `prt3` tag.
#[derive(Clone, Debug, PartialEq)]
pub struct Particle {
    /// +0 bit set (observed values include 0, 4, 6, 7, 8, 0xe, 0x10, 0x17, 0x18, 0xc0).
    pub flags: u16,
    /// +2 observed 0 or 1.
    pub unknown_02: u16,
    /// +4 observed 0, 1, 2.
    pub unknown_04: u32,
    /// +8 (u16 at +8 observed 0..=4, u16 at +0xa observed 0..=8).
    pub unknown_08: [u16; 2],
    /// +0xc reference to the shader template (class `stem`).
    pub shader_template: TagRef,
    /// +0x14 block of shader parameters.
    pub parameters: Vec<ShaderParameter>,
    /// +0x1c.. five properties. Slot 0 is colour-like (its functions have the
    /// non-scalar flag set), the others are scalars; their roles are not
    /// established (see docs).
    pub properties: Vec<Property>,
    /// +0x6c reference; class observed `foot`, `snd!` or `effe`, or null.
    pub ref_6c: TagRef,
    /// +0x74 reference; class observed `snd!` or `effe`, or null.
    pub ref_74: TagRef,
    /// +0x7c block of 4-byte records (at most 1 on the disc); kept raw.
    pub unknown_7c: Vec<[u8; 4]>,
    /// +0x84 block of nested particle systems.
    pub systems: Vec<EffectParticleSystem>,
    /// +0x8c block (count 1 on the disc) of 0x7c-byte embedded shader
    /// post-process records. Their own nested blocks are not followed.
    pub postprocess: Vec<Vec<u8>>,
    /// +0x94, always 0 on the disc.
    pub unknown_94: u32,
    /// +0x98 bit set (low bits 1, 4, 8 and 0x800 observed).
    pub flags_98: u32,
    /// +0x9c four scalars, mostly (-0.5, -0.5, 0.5, 0.5) or (-1, -1, 2, 2):
    /// a quad extent. Exact meaning unconfirmed (see docs).
    pub quad: [f32; 4],
    /// +0xac four scalars: (min, max) pairs, usually (0, 1, 0, 1): texture
    /// coordinate bounds. Unconfirmed.
    pub uv: [f32; 4],
}

impl EffectTag for Particle {
    const GROUP: [u8; 4] = *b"prt3";
    fn decode_at(ctx: &mut Ctx<'_>, address: u32) -> Result<Self> {
        let root = ctx.mem.read(address, ROOT_SIZE)?;
        let r = Rec::exact(&root, ROOT_SIZE)?;
        let mut parameters = Vec::new();
        for rec in ctx.block(r.block(0x14), PARAMETER_SIZE)? {
            let p = Rec::exact(&rec, PARAMETER_SIZE)?;
            parameters.push(ShaderParameter {
                name: StringId(p.u32(0)),
                unknown_04: p.u32(4),
                bitmap: p.tag_ref(8),
                value: p.f32(0x10),
                color: [p.f32(0x14), p.f32(0x18), p.f32(0x1c)],
                unknown_20: [p.u32(0x20), p.u32(0x24)],
            });
        }
        let mut properties = Vec::with_capacity(PROPERTY_COUNT);
        for i in 0..PROPERTY_COUNT {
            let o = 0x1c + i * Property::SIZE;
            properties.push(Property::decode(
                Rec::exact(&root[o..o + Property::SIZE], Property::SIZE)?,
                ctx,
            )?);
        }
        let unknown_7c = ctx
            .block(r.block(0x7c), 4)?
            .into_iter()
            .map(|b| [b[0], b[1], b[2], b[3]])
            .collect();
        let mut systems = Vec::new();
        for rec in ctx.block(r.block(0x84), SYSTEM_SIZE)? {
            systems.push(EffectParticleSystem::decode(
                Rec::exact(&rec, SYSTEM_SIZE)?,
                ctx,
            )?);
        }
        let postprocess = ctx.block(r.block(0x8c), POSTPROCESS_SIZE)?;
        Ok(Self {
            flags: r.u16(0),
            unknown_02: r.u16(2),
            unknown_04: r.u32(4),
            unknown_08: [r.u16(8), r.u16(0xa)],
            shader_template: r.tag_ref(0xc),
            parameters,
            properties,
            ref_6c: r.tag_ref(0x6c),
            ref_74: r.tag_ref(0x74),
            unknown_7c,
            systems,
            postprocess,
            unknown_94: r.u32(0x94),
            flags_98: r.u32(0x98),
            quad: [r.f32(0x9c), r.f32(0xa0), r.f32(0xa4), r.f32(0xa8)],
            uv: [r.f32(0xac), r.f32(0xb0), r.f32(0xb4), r.f32(0xb8)],
        })
    }
    fn references(&self) -> Vec<(String, TagRef)> {
        let mut v = vec![("shader_template".to_string(), self.shader_template)];
        for (i, p) in self.parameters.iter().enumerate() {
            v.push((format!("parameters[{i}].bitmap"), p.bitmap));
        }
        v.push(("ref_6c".into(), self.ref_6c));
        v.push(("ref_74".into(), self.ref_74));
        for (i, s) in self.systems.iter().enumerate() {
            v.push((format!("systems[{i}].particle"), s.particle));
            for (j, e) in s.emitters.iter().enumerate() {
                v.push((format!("systems[{i}].emitters[{j}].movement"), e.movement));
            }
        }
        v
    }

    fn functions(&self) -> Vec<(String, &[u8])> {
        let mut v: Vec<(String, &[u8])> = self
            .properties
            .iter()
            .enumerate()
            .map(|(i, p)| (format!("properties[{i}]"), p.data.as_slice()))
            .collect();
        for (i, s) in self.systems.iter().enumerate() {
            for (j, e) in s.emitters.iter().enumerate() {
                for (k, p) in e.properties.iter().enumerate() {
                    v.push((
                        format!("systems[{i}].emitters[{j}].properties[{k}]"),
                        p.data.as_slice(),
                    ));
                }
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
    fn tref(v: &mut Vec<u8>, g: &[u8; 4], id: u32) {
        le(v, u32::from_be_bytes(*g));
        le(v, id);
    }
    fn root(
        mem: &mut SliceMemory,
        params: (u32, u32),
        sys: (u32, u32),
        post: (u32, u32),
        prop_data: (u32, u32),
    ) -> u32 {
        let mut r = Vec::new();
        le(&mut r, 0x0001_0007); // flags, unknown_02
        le(&mut r, 1);
        le(&mut r, 0x0003_0000);
        tref(&mut r, b"stem", 0xe100_0001);
        le(&mut r, params.0);
        le(&mut r, params.1);
        for i in 0..5 {
            r.extend([0, 0, 2, 0, 0, 0, 3, 0]);
            le(&mut r, if i == 2 { 0 } else { prop_data.0 });
            le(&mut r, if i == 2 { 0 } else { prop_data.1 });
        }
        assert_eq!(r.len(), 0x6c);
        tref(&mut r, b"foot", 0xffff_ffff);
        tref(&mut r, b"effe", 0xe100_0002);
        le(&mut r, 0);
        le(&mut r, 0);
        le(&mut r, sys.0);
        le(&mut r, sys.1);
        le(&mut r, post.0);
        le(&mut r, post.1);
        le(&mut r, 0);
        le(&mut r, 0x805);
        for f in [-0.5f32, -0.5, 0.5, 0.5, 0.0, 1.0, 0.0, 1.0] {
            r.extend(f.to_le_bytes());
        }
        assert_eq!(r.len(), ROOT_SIZE);
        mem.push(&r)
    }
    fn parameter(name: u32, bitmap: u32) -> Vec<u8> {
        let mut p = Vec::new();
        le(&mut p, name);
        le(&mut p, 1);
        tref(&mut p, b"bitm", bitmap);
        p.extend(0.5f32.to_le_bytes());
        for _ in 0..5 {
            le(&mut p, 0);
        }
        // +0x14..0x20 colour zeros already; +0x20/+0x24 follow
        p.truncate(PARAMETER_SIZE);
        while p.len() < PARAMETER_SIZE {
            p.push(0);
        }
        p
    }

    #[test]
    fn decodes_valid_tag_with_aliased_function_data() {
        let mut mem = SliceMemory::new(0x8100_0000);
        let shared = mem.push(&[0x01, 0x10, 0, 0, 9, 9, 9, 9]); // "another tag's" bytes
        let mut params = parameter(0x0b00_0102, 0xe100_0010);
        params.extend(parameter(0x0b00_0103, 0xffff_ffff));
        let params_at = mem.push(&params);
        let mut sys = vec![0u8; SYSTEM_SIZE];
        sys[..4].copy_from_slice(&u32::from_be_bytes(*b"prt3").to_le_bytes());
        sys[4..8].copy_from_slice(&0xe100_0003u32.to_le_bytes());
        let mut em = vec![0u8; EMITTER_SIZE];
        em[..4].copy_from_slice(&u32::from_be_bytes(*b"pmov").to_le_bytes());
        em[4..8].copy_from_slice(&0xe100_0004u32.to_le_bytes());
        let em_at = mem.push(&em);
        sys[0x30..0x34].copy_from_slice(&1u32.to_le_bytes());
        sys[0x34..0x38].copy_from_slice(&em_at.to_le_bytes());
        let sys_at = mem.push(&sys);
        let post_at = mem.push(&[7u8; POSTPROCESS_SIZE]);
        let at = root(
            &mut mem,
            (2, params_at),
            (1, sys_at),
            (1, post_at),
            (8, shared),
        );
        let t = Particle::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert_eq!(t.flags, 7);
        assert_eq!(t.unknown_02, 1);
        assert_eq!(t.shader_template.group, FourCC(*b"stem"));
        assert_eq!(t.parameters.len(), 2);
        assert_eq!(t.parameters[0].bitmap.id, TagId(0xe100_0010));
        assert!(t.parameters[1].bitmap.is_null());
        assert_eq!(t.parameters[0].value, 0.5);
        assert_eq!(t.properties.len(), 5);
        assert_eq!(t.properties[0].data.len(), 8);
        assert!(t.properties[2].data.is_empty());
        assert_eq!(t.systems[0].emitters[0].movement.id, TagId(0xe100_0004));
        assert_eq!(t.postprocess[0].len(), POSTPROCESS_SIZE);
        assert_eq!(t.flags_98, 0x805);
        assert_eq!(t.quad, [-0.5, -0.5, 0.5, 0.5]);
        let refs = t.references();
        assert!(
            refs.iter()
                .any(|(n, r)| n == "systems[0].emitters[0].movement" && !r.is_null())
        );
        assert_eq!(refs.iter().filter(|(_, r)| r.is_null()).count(), 2); // ref_6c, parameter 1
    }

    #[test]
    fn empty_blocks_and_unknown_values() {
        let mut mem = SliceMemory::new(0x1000);
        let at = root(&mut mem, (0, 0xffff_0000), (0, 0), (0, 0), (0, 0));
        mem.bytes[(at - 0x1000) as usize..][..2].copy_from_slice(&0xffffu16.to_le_bytes());
        let t = Particle::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert_eq!(t.flags, 0xffff, "unknown bits preserved");
        assert!(t.parameters.is_empty() && t.systems.is_empty() && t.postprocess.is_empty());
    }

    #[test]
    fn truncated_or_out_of_bounds_blocks_fail() {
        let mut mem = SliceMemory::new(0x1000);
        let at = root(&mut mem, (3, 0x7000_0000), (0, 0), (0, 0), (0, 0));
        assert!(Particle::decode_at(&mut Ctx::new(&mut mem), at).is_err());
        assert!(
            Particle::decode_at(&mut Ctx::new(&mut mem), at + 8).is_err(),
            "short root"
        );
    }

    #[test]
    fn hostile_counts_exhaust_the_budget() {
        let mut mem = SliceMemory::new(0x1000);
        let at = root(&mut mem, (0x00ff_ffff, 0x1000), (0, 0), (0, 0), (0, 0));
        assert!(Particle::decode_at(&mut Ctx::new(&mut mem), at).is_err());
    }
}
