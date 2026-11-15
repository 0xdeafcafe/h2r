//! `effe` (effect): locations, events, and each event's parts, beams,
//! accelerations and particle systems. Layout and evidence: `docs/effects.md`.
//!
//! Evidence labels in field docs: **V** verified over every instance on the
//! disc (stride tiling, pointers, reference classes), **E** corroborated by
//! the engine code that reads the field, **S** a name or meaning taken from
//! public descriptions with no stronger evidence here. Fields with no
//! evidence are `unknown_<offset>`.
use super::EffectTag;
use super::mem::{Ctx, Property, Rec, TagRef};
use crate::Result;
use crate::wire::{StringId, TagBlock};
use map::FourCC;

/// `effe` root record size (**V**: the smallest child-block pointer of every
/// instance with children sits at +0x30; **E**: the engine addresses events
/// at +0x14/+0x18).
pub const ROOT_SIZE: usize = 0x30;
/// Event record size (**E**: the event-processing and particle-system
/// creation code index events with a 0x38 stride).
pub const EVENT_SIZE: usize = 0x38;
/// Part record size (**E**: stride 0x38 in the part-creation loop).
pub const PART_SIZE: usize = 0x38;
/// Beam record size (**V**: consecutive elements and the first child blob tile
/// exactly at 0x3c on the instances that have beams).
pub const BEAM_SIZE: usize = 0x3c;
/// Acceleration record size (**V**: tiles exactly at 0x14).
pub const ACCELERATION_SIZE: usize = 0x14;
/// Particle-system record size (**E**: 0x38 stride in particle-system creation).
pub const PARTICLE_SYSTEM_SIZE: usize = 0x38;
/// Particle emitter record size (**V**: tiles exactly at 0xb8).
pub const EMITTER_SIZE: usize = 0xb8;

/// A complete effect.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    /// +0x00 flag bits, meanings unknown here (**V**: values < 0x40 only).
    pub flags: u32,
    /// +0x04, -1 or an event index (**V**: always -1, 0 or 1). **S**: loop start event.
    pub loop_start_event: i16,
    /// +0x06 small integer 0..15 (**V**).
    pub unknown_06: u16,
    /// +0x08 float, almost always 0.
    pub unknown_08: f32,
    /// +0x0c block of marker names (4-byte string IDs). **E**: part and
    /// particle-system `location` indices are checked against this count.
    pub locations: Vec<StringId>,
    /// +0x14 block of events.
    pub events: Vec<EffectEvent>,
    /// +0x1c looping sound reference (`lsnd`, or null). **V**.
    pub looping_sound: TagRef,
    /// +0x24 (-1, 0, 1, 2 or 4 on the disc).
    pub unknown_24: i16,
    /// +0x26 always 0.
    pub unknown_26: u16,
    /// +0x28 0 or 10.0 on the disc.
    pub unknown_28: f32,
    /// +0x2c 0 or 50.0 on the disc.
    pub unknown_2c: f32,
}

/// One event of an effect (`0x38` bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct EffectEvent {
    /// +0x00, always 0 on the disc.
    pub unknown_00: u32,
    /// +0x04 (**S**: skip fraction).
    pub skip_fraction: f32,
    /// +0x08..0x10 (**S**: delay bounds, seconds).
    pub delay_bounds: [f32; 2],
    /// +0x10..0x18 (**S**: duration bounds; 1/30 s is the usual default, **V**).
    pub duration_bounds: [f32; 2],
    /// +0x18 block.
    pub parts: Vec<EffectPart>,
    /// +0x20 block (**S**: beams).
    pub beams: Vec<EffectBeam>,
    /// +0x28 block.
    pub accelerations: Vec<EffectAcceleration>,
    /// +0x30 block.
    pub particle_systems: Vec<EffectParticleSystem>,
}

/// A part: creates one other tag's thing (decal, sound, light, lens flare,
/// damage effect, object ...) at a location.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectPart {
    /// +0x00, 0 on the disc. **E**: a four-way switch (0..=3) gates creation.
    pub disposition: u16,
    /// +0x02, 0 or 1 on the disc. **E**: compared with the effect's
    /// underwater flag (1 skips the part underwater, 2 skips it above water).
    pub create_in: u16,
    /// +0x04 index into the effect's `locations` (**E**: negative or out of
    /// range skips the part; **V**: always in range on the disc).
    pub location: i16,
    /// +0x06 flag bits (**E**: bit 0 forces a world axis as the direction,
    /// bit 1 randomises the offset between `radius_modifier_bounds`, bit 2 and
    /// bit 4 change orientation/spawn handling; other bits unknown).
    pub flags: u16,
    /// +0x08 class the engine dispatches on (**E**): `deca`, `snd!`, `ligh`,
    /// `lens`, `jpt!`, `obje`, `MGS2` ... Equals `reference.group` except for
    /// `obje`, where the reference names a concrete object class (**V**).
    pub type_group: FourCC,
    /// +0x0c the tag created (**V**: always non-null).
    pub reference: TagRef,
    /// +0x14, +0x18 (**S**: velocity bounds).
    pub velocity_bounds: [f32; 2],
    /// +0x1c (**S**: velocity cone angle, radians).
    pub velocity_cone_angle: f32,
    /// +0x20, +0x24 (**S**: angular velocity bounds).
    pub angular_velocity_bounds: [f32; 2],
    /// +0x28, +0x2c (**E**: passed to the random-range helper as two bounds;
    /// **S**: radius modifier bounds).
    pub radius_modifier_bounds: [f32; 2],
    /// +0x30 (**E**: bit 5 selects scaling by the effect's first scalar).
    pub a_scalars: u32,
    /// +0x34 (**E**: bit 5 selects scaling by the second scalar; 0 on the disc).
    pub b_scalars: u32,
}

/// An event beam (`0x3c` bytes): a shader reference and six function blobs
/// stored as bare data references (no input header, unlike [`Property`]).
#[derive(Clone, Debug, PartialEq)]
pub struct EffectBeam {
    /// +0x00 shader reference (`shad`, **V**).
    pub shader: TagRef,
    /// +0x08.
    pub unknown_08: u32,
    /// +0x0c..0x3c six function data blobs, bytes retained verbatim.
    pub functions: Vec<Vec<u8>>,
}

/// An acceleration applied by the event (`0x14` bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct EffectAcceleration {
    /// +0x00 (0 on the disc).
    pub create_in: u16,
    /// +0x02 (0 on the disc).
    pub disposition: u16,
    /// +0x04 location index (**V**: within the effect's locations).
    pub location: i16,
    /// +0x06 (0 on the disc).
    pub unknown_06: u16,
    /// +0x08.
    pub acceleration: f32,
    /// +0x0c (**S**: inner cone angle).
    pub inner_cone_angle: f32,
    /// +0x10 (**S**: outer cone angle).
    pub outer_cone_angle: f32,
}

/// A particle system of an event: one particle (`prt3`, or `PRTM` model
/// particle) driven by a list of emitters.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectParticleSystem {
    /// +0x00 the particle: `prt3` (41,973 on the disc) or `PRTM` (782). **E**
    /// (the engine accepts exactly these two classes) and **V**.
    pub particle: TagRef,
    /// +0x08 location index (**E**: the engine reads it as a 32-bit value and
    /// treats -1 as none).
    pub location: i32,
    /// +0x0c (0, 1 or 2). **E**: non-zero is passed as a flag to the creation helper.
    pub unknown_0c: u16,
    /// +0x0e (0 or 1). **E**: passed to a per-location compatibility check.
    pub unknown_0e: u16,
    /// +0x10 (0, 1 or 2).
    pub unknown_10: u16,
    /// +0x12 (0..3). **E**: passed to the location lookup helper.
    pub unknown_12: u16,
    /// +0x14 signed small integer (-10..6 on the disc).
    pub unknown_14: i16,
    /// +0x16 (0 on the disc).
    pub unknown_16: u16,
    /// +0x18..0x24 three floats: (offset, range, 1/range) on every instance (**V**).
    pub fade: [f32; 3],
    /// +0x24..0x30 three floats: (distance, range, 1/range) on every instance (**V**).
    pub lod: [f32; 3],
    /// +0x30 block of emitters.
    pub emitters: Vec<EffectEmitter>,
}

/// A particle emitter (`0xb8` bytes): a movement reference and nine
/// properties (functions of effect/emitter inputs) plus scalar parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectEmitter {
    /// +0x00 `pmov` reference (**V**: always `pmov`).
    pub movement: TagRef,
    /// Properties at +0x08, +0x18, +0x28, +0x38, +0x48, +0x58, +0x68 (the first
    /// seven, 16 bytes each), then +0x7c and +0x8c (**V**: all nine decode on
    /// every instance). Meanings per slot are open (see docs).
    pub properties: Vec<Property>,
    /// +0x78 emission shape selector (0, 2, 3, 4, 5 on the disc). **E**: the
    /// emitter initialiser `0x00249170` switches on it (jump table `0x00249a74`).
    pub emission_shape: u32,
    /// +0x9c..0xb0 five floats.
    pub unknown_9c: [f32; 5],
    /// +0xb0.
    pub unknown_b0: u32,
    /// +0xb4.
    pub unknown_b4: u32,
}

/// Byte offsets of the nine emitter properties.
pub const EMITTER_PROPERTY_OFFSETS: [usize; 9] =
    [0x08, 0x18, 0x28, 0x38, 0x48, 0x58, 0x68, 0x7c, 0x8c];

fn bounds(r: &Rec<'_>, o: usize) -> [f32; 2] {
    [r.f32(o), r.f32(o + 4)]
}

impl EffectTag for Effect {
    const GROUP: [u8; 4] = *b"effe";

    fn decode_at(ctx: &mut Ctx<'_>, address: u32) -> Result<Self> {
        let root = ctx.mem.read(address, ROOT_SIZE)?;
        let r = Rec::exact(&root, ROOT_SIZE)?;
        let locations = ctx
            .block(r.block(0x0c), 4)?
            .iter()
            .map(|b| Ok(StringId(Rec::exact(b, 4)?.u32(0))))
            .collect::<Result<Vec<_>>>()?;
        let events = ctx
            .block(r.block(0x14), EVENT_SIZE)?
            .iter()
            .map(|b| EffectEvent::decode(Rec::exact(b, EVENT_SIZE)?, ctx))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            flags: r.u32(0),
            loop_start_event: r.i16(4),
            unknown_06: r.u16(6),
            unknown_08: r.f32(8),
            locations,
            events,
            looping_sound: r.tag_ref(0x1c),
            unknown_24: r.i16(0x24),
            unknown_26: r.u16(0x26),
            unknown_28: r.f32(0x28),
            unknown_2c: r.f32(0x2c),
        })
    }

    fn references(&self) -> Vec<(String, TagRef)> {
        let mut out = vec![("looping_sound".to_string(), self.looping_sound)];
        for (e, ev) in self.events.iter().enumerate() {
            for (i, p) in ev.parts.iter().enumerate() {
                out.push((format!("events[{e}].parts[{i}].reference"), p.reference));
            }
            for (i, b) in ev.beams.iter().enumerate() {
                out.push((format!("events[{e}].beams[{i}].shader"), b.shader));
            }
            for (i, ps) in ev.particle_systems.iter().enumerate() {
                out.push((
                    format!("events[{e}].particle_systems[{i}].particle"),
                    ps.particle,
                ));
                for (j, em) in ps.emitters.iter().enumerate() {
                    out.push((
                        format!("events[{e}].particle_systems[{i}].emitters[{j}].movement"),
                        em.movement,
                    ));
                }
            }
        }
        out
    }

    fn functions(&self) -> Vec<(String, &[u8])> {
        let mut out = Vec::new();
        for (e, ev) in self.events.iter().enumerate() {
            for (i, b) in ev.beams.iter().enumerate() {
                for (k, f) in b.functions.iter().enumerate() {
                    out.push((
                        format!("events[{e}].beams[{i}].functions[{k}]"),
                        f.as_slice(),
                    ));
                }
            }
            for (i, ps) in ev.particle_systems.iter().enumerate() {
                for (j, em) in ps.emitters.iter().enumerate() {
                    for (k, p) in em.properties.iter().enumerate() {
                        out.push((
                            format!(
                                "events[{e}].particle_systems[{i}].emitters[{j}].properties[{k}]"
                            ),
                            p.data.as_slice(),
                        ));
                    }
                }
            }
        }
        out
    }
}

impl EffectEvent {
    fn decode(r: Rec<'_>, ctx: &mut Ctx<'_>) -> Result<Self> {
        Ok(Self {
            unknown_00: r.u32(0),
            skip_fraction: r.f32(4),
            delay_bounds: bounds(&r, 8),
            duration_bounds: bounds(&r, 0x10),
            parts: block_of(ctx, r.block(0x18), PART_SIZE, EffectPart::decode)?,
            beams: block_of(ctx, r.block(0x20), BEAM_SIZE, EffectBeam::decode)?,
            accelerations: block_of(ctx, r.block(0x28), ACCELERATION_SIZE, |r, _| {
                Ok(EffectAcceleration::decode(r))
            })?,
            particle_systems: block_of(
                ctx,
                r.block(0x30),
                PARTICLE_SYSTEM_SIZE,
                EffectParticleSystem::decode,
            )?,
        })
    }
}

fn block_of<T>(
    ctx: &mut Ctx<'_>,
    block: TagBlock,
    stride: usize,
    decode: impl Fn(Rec<'_>, &mut Ctx<'_>) -> Result<T>,
) -> Result<Vec<T>> {
    ctx.block(block, stride)?
        .iter()
        .map(|b| decode(Rec::exact(b, stride)?, ctx))
        .collect()
}

impl EffectPart {
    fn decode(r: Rec<'_>, _: &mut Ctx<'_>) -> Result<Self> {
        Ok(Self {
            disposition: r.u16(0),
            create_in: r.u16(2),
            location: r.i16(4),
            flags: r.u16(6),
            type_group: r.fourcc(8),
            reference: r.tag_ref(0x0c),
            velocity_bounds: bounds(&r, 0x14),
            velocity_cone_angle: r.f32(0x1c),
            angular_velocity_bounds: bounds(&r, 0x20),
            radius_modifier_bounds: bounds(&r, 0x28),
            a_scalars: r.u32(0x30),
            b_scalars: r.u32(0x34),
        })
    }
}

impl EffectBeam {
    fn decode(r: Rec<'_>, ctx: &mut Ctx<'_>) -> Result<Self> {
        let functions = (0..6)
            .map(|i| ctx.data(r.data(0x0c + i * 8)))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            shader: r.tag_ref(0),
            unknown_08: r.u32(8),
            functions,
        })
    }
}

impl EffectAcceleration {
    fn decode(r: Rec<'_>) -> Self {
        Self {
            create_in: r.u16(0),
            disposition: r.u16(2),
            location: r.i16(4),
            unknown_06: r.u16(6),
            acceleration: r.f32(8),
            inner_cone_angle: r.f32(0x0c),
            outer_cone_angle: r.f32(0x10),
        }
    }
}

impl EffectParticleSystem {
    pub(crate) fn decode(r: Rec<'_>, ctx: &mut Ctx<'_>) -> Result<Self> {
        Ok(Self {
            particle: r.tag_ref(0),
            location: r.u32(8) as i32,
            unknown_0c: r.u16(0x0c),
            unknown_0e: r.u16(0x0e),
            unknown_10: r.u16(0x10),
            unknown_12: r.u16(0x12),
            unknown_14: r.i16(0x14),
            unknown_16: r.u16(0x16),
            fade: [r.f32(0x18), r.f32(0x1c), r.f32(0x20)],
            lod: [r.f32(0x24), r.f32(0x28), r.f32(0x2c)],
            emitters: block_of(ctx, r.block(0x30), EMITTER_SIZE, EffectEmitter::decode)?,
        })
    }
}

impl EffectEmitter {
    fn decode(r: Rec<'_>, ctx: &mut Ctx<'_>) -> Result<Self> {
        let properties = EMITTER_PROPERTY_OFFSETS
            .iter()
            .map(|&o| Property::decode(Rec(&r.0[o..o + Property::SIZE]), ctx))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            movement: r.tag_ref(0),
            properties,
            emission_shape: r.u32(0x78),
            unknown_9c: [
                r.f32(0x9c),
                r.f32(0xa0),
                r.f32(0xa4),
                r.f32(0xa8),
                r.f32(0xac),
            ],
            unknown_b0: r.u32(0xb0),
            unknown_b4: r.u32(0xb4),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::SliceMemory;
    use map::TagId;

    const BASE: u32 = 0x8000_0000;

    fn put32(v: &mut Vec<u8>, x: u32) {
        v.extend(x.to_le_bytes());
    }
    fn putf(v: &mut Vec<u8>, x: f32) {
        v.extend(x.to_le_bytes());
    }
    fn tag_ref(v: &mut Vec<u8>, group: &[u8; 4], id: u32) {
        put32(v, u32::from_be_bytes(*group));
        put32(v, id);
    }
    /// A part record referencing `deca`.
    fn part(location: i16) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(0u16.to_le_bytes());
        v.extend(1u16.to_le_bytes());
        v.extend(location.to_le_bytes());
        v.extend(2u16.to_le_bytes());
        put32(&mut v, u32::from_be_bytes(*b"deca"));
        tag_ref(&mut v, b"deca", 0xe001_0007);
        for x in [1.0f32, 2.0, 0.5, 0.0, 0.0, 1.0, 1.5] {
            putf(&mut v, x);
        }
        put32(&mut v, 0x20);
        put32(&mut v, 0);
        assert_eq!(v.len(), PART_SIZE);
        v
    }
    /// A property record whose function data is `data`, stored at `mem`.
    fn property(mem: &mut SliceMemory, data: &[u8]) -> Vec<u8> {
        let addr = mem.push(data);
        let mut v = Vec::new();
        for x in [4i16, 5, 2, 7] {
            v.extend(x.to_le_bytes());
        }
        put32(&mut v, data.len() as u32);
        put32(&mut v, addr);
        v
    }
    fn emitter(mem: &mut SliceMemory) -> Vec<u8> {
        let mut v = Vec::new();
        tag_ref(&mut v, b"pmov", 0xe002_0001);
        let func = [0u8; 20];
        let mut props: Vec<Vec<u8>> = (0..9).map(|_| property(mem, &func)).collect();
        for p in props.drain(..7) {
            v.extend(p);
        }
        put32(&mut v, 3);
        for p in props {
            v.extend(p);
        }
        for x in [0.5f32, 0.0, 0.0, 0.0, 0.0] {
            putf(&mut v, x);
        }
        put32(&mut v, 2);
        put32(&mut v, 9);
        assert_eq!(v.len(), EMITTER_SIZE);
        v
    }
    fn block(count: u32, address: u32) -> Vec<u8> {
        let mut v = Vec::new();
        put32(&mut v, count);
        put32(&mut v, address);
        v
    }
    /// Effect with one location and one event: 2 parts, 1 acceleration, 1 particle
    /// system with 1 emitter. Everything lives outside the root, like aliased data.
    fn build() -> (SliceMemory, u32) {
        let mut mem = SliceMemory::new(BASE);
        let loc = mem.push(&0x0501_0042u32.to_le_bytes());
        let parts_addr = mem.push(&[part(0), part(0)].concat());
        let mut acc = Vec::new();
        acc.extend([0u8; 4]);
        acc.extend(0i16.to_le_bytes());
        acc.extend([0u8; 2]);
        for x in [2.0f32, 0.0, 180.0] {
            putf(&mut acc, x);
        }
        let acc_addr = mem.push(&acc);
        let em = emitter(&mut mem);
        let em_addr = mem.push(&em);
        let mut ps = Vec::new();
        tag_ref(&mut ps, b"prt3", 0xe003_0001);
        put32(&mut ps, 0);
        for x in [0u16, 0, 0, 0, 0, 0] {
            ps.extend(x.to_le_bytes());
        }
        for x in [-0.1f32, 3.0, 1.0 / 3.0, 30.0, 10.0, 0.1] {
            putf(&mut ps, x);
        }
        ps.extend(block(1, em_addr));
        assert_eq!(ps.len(), PARTICLE_SYSTEM_SIZE);
        let ps_addr = mem.push(&ps);
        let mut ev = Vec::new();
        put32(&mut ev, 0);
        for x in [0.25f32, 0.0, 0.0, 1.0 / 30.0, 1.0 / 30.0] {
            putf(&mut ev, x);
        }
        ev.extend(block(2, parts_addr));
        ev.extend(block(0, 0xdead_beef));
        ev.extend(block(1, acc_addr));
        ev.extend(block(1, ps_addr));
        assert_eq!(ev.len(), EVENT_SIZE);
        let ev_addr = mem.push(&ev);
        let mut root = Vec::new();
        put32(&mut root, 2);
        root.extend((-1i16).to_le_bytes());
        root.extend(0u16.to_le_bytes());
        putf(&mut root, 0.0);
        root.extend(block(1, loc));
        root.extend(block(1, ev_addr));
        tag_ref(&mut root, b"lsnd", u32::MAX);
        root.extend((-1i16).to_le_bytes());
        root.extend(0u16.to_le_bytes());
        putf(&mut root, 10.0);
        putf(&mut root, 50.0);
        assert_eq!(root.len(), ROOT_SIZE);
        let root_addr = mem.push(&root);
        (mem, root_addr)
    }

    #[test]
    fn decodes_a_full_effect() {
        let (mut mem, root) = build();
        let e = Effect::decode_at(&mut Ctx::new(&mut mem), root).unwrap();
        assert_eq!(e.locations, vec![StringId(0x0501_0042)]);
        assert_eq!(e.loop_start_event, -1);
        assert!(e.looping_sound.is_null());
        let ev = &e.events[0];
        assert_eq!(ev.skip_fraction, 0.25);
        assert_eq!(ev.parts.len(), 2);
        assert_eq!(ev.parts[0].type_group, FourCC(*b"deca"));
        assert_eq!(ev.parts[0].a_scalars, 0x20);
        assert_eq!(ev.parts[0].velocity_bounds, [1.0, 2.0]);
        assert!(ev.beams.is_empty());
        assert_eq!(ev.accelerations[0].outer_cone_angle, 180.0);
        let ps = &ev.particle_systems[0];
        assert_eq!(ps.particle.group, FourCC(*b"prt3"));
        assert_eq!(ps.lod, [30.0, 10.0, 0.1]);
        let em = &ps.emitters[0];
        assert_eq!(em.movement.group, FourCC(*b"pmov"));
        assert_eq!(em.properties.len(), 9);
        assert_eq!(em.properties[8].input, 4);
        assert_eq!(em.emission_shape, 3);
        assert_eq!(em.unknown_b4, 9);
        let refs = e.references();
        assert_eq!(refs.len(), 1 + 2 + 1 + 1);
        assert_eq!(refs[1].0, "events[0].parts[0].reference");
        assert_eq!(refs[1].1.id, TagId(0xe001_0007));
    }

    #[test]
    fn empty_blocks_ignore_their_address() {
        let (mut mem, root) = build();
        // The event's beam block has count 0 and a bogus address: must not be read.
        assert!(Effect::decode_at(&mut Ctx::new(&mut mem), root).is_ok());
    }

    #[test]
    fn out_of_bounds_block_is_an_error() {
        let (mut mem, root) = build();
        // Point the root's event block outside the memory.
        let off = (root - BASE) as usize + 0x18;
        mem.bytes[off..off + 4].copy_from_slice(&0x9000_0000u32.to_le_bytes());
        assert!(matches!(
            Effect::decode_at(&mut Ctx::new(&mut mem), root),
            Err(crate::Error::Bounds { .. })
        ));
    }

    #[test]
    fn truncated_root_is_an_error() {
        let mut mem = SliceMemory::new(BASE);
        let addr = mem.push(&[0u8; 0x20]);
        assert!(Effect::decode_at(&mut Ctx::new(&mut mem), addr).is_err());
    }

    #[test]
    fn element_count_over_budget_is_rejected() {
        let (mut mem, root) = build();
        let off = (root - BASE) as usize + 0x14;
        mem.bytes[off..off + 4].copy_from_slice(&0x00ff_ffffu32.to_le_bytes());
        let mut ctx = Ctx::new(&mut mem);
        assert!(Effect::decode_at(&mut ctx, root).is_err());
    }

    #[test]
    fn aliased_blocks_share_bytes_but_not_budget() {
        // Two parts pointing at the same bytes decode twice and are charged twice.
        let (mut mem, root) = build();
        let mut ctx = Ctx::new(&mut mem);
        ctx.budget.elements = 3; // locations(1) + events(1) + parts(2) needs more
        assert!(Effect::decode_at(&mut ctx, root).is_err());
    }

    #[test]
    fn unknown_values_are_preserved() {
        let (mut mem, root) = build();
        let off = (root - BASE) as usize;
        mem.bytes[off..off + 4].copy_from_slice(&0xdead_0003u32.to_le_bytes());
        let e = Effect::decode_at(&mut Ctx::new(&mut mem), root).unwrap();
        assert_eq!(e.flags, 0xdead_0003);
    }

    /// A beam is exactly 0x3c bytes: the memory ends at the last byte of a
    /// one-beam block, so any larger stride reads out of bounds, and the
    /// shader reference must sit at its +0 offset.
    #[test]
    fn beam_stride_is_pinned_by_exact_bounds() {
        fn put(b: &mut [u8], o: usize, v: u32) {
            b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        }
        let a = 0x1000u32;
        let mut b = vec![0u8; 0x30 + 0x38 + 6 * 20 + 0x3c];
        put(&mut b, 0x14, 1);
        put(&mut b, 0x18, a + 0x30);
        put(&mut b, 0x30 + 0x20, 1);
        put(&mut b, 0x30 + 0x24, a + 0x68 + 120);
        let beam = 0x68 + 120;
        put(&mut b, beam, u32::from_be_bytes(*b"shad"));
        put(&mut b, beam + 4, 0xe100_0001);
        for i in 0..6 {
            put(&mut b, beam + 0xc + i * 8, 20);
            put(&mut b, beam + 0x10 + i * 8, a + 0x68 + i as u32 * 20);
        }
        let mut m = SliceMemory { base: a, bytes: b };
        let fx = Effect::decode_at(&mut Ctx::new(&mut m), a).unwrap();
        assert_eq!(fx.events[0].beams[0].functions.len(), 6);
        assert_eq!(fx.references()[1].1.id.0, 0xe100_0001);
    }
}
