//! A particle walk's births (docs/graphics/native-particles.md, "Walking a
//! system's particles"): what [`engine::particle_walk`] hands each
//! callback, as GPU particle system births
//! ([`native_render::particle_mirror`]), batched by [`Key`] in the guest's
//! order. The callback never runs: the host builds each batch's system
//! from `particle_mirror::owned(definition, &batch.key)`
//! (`native_particle_walk`). Pure.

use std::collections::BTreeMap;

use engine::function_eval::constant::BYTE;
use guest_layout::memory::GuestMemory;
use engine::particle_emission::constant::TINT;
use engine::particle_walk::{self as pw, Call, Emitted};
use native_render::particle_mirror::{self as nm, Key};
use native_render::particle_system::Spawn;

/// One system's particles of one walk, in the guest's order.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub key: Key,
    pub spawns: Vec<Spawn>,
}

/// One walk's batches and the input block's writes it makes
/// (`particle_walk::Walk::writes`), for the guest to take in place of
/// running it.
#[derive(Clone, Debug, PartialEq)]
pub struct Births {
    pub batches: Vec<Batch>,
    pub writes: BTreeMap<u32, u8>,
}

/// The walk entered as `call`, under `mxcsr` and `fcw`, as batches. `Err`
/// names why not: the walk's refusal (`particle_walk::Refused::name`), the
/// mirror's (`particle_mirror::Refused::name`) or `fault`.
pub fn births(m: &dyn GuestMemory, call: Call, mxcsr: u32, fcw: u16) -> Result<Births, &'static str> {
    let w = pw::walk(m, call, mxcsr, fcw).map_err(pw::Refused::name)?;
    Ok(Births { batches: batches(m, call.draw, &w.emitted)?, writes: w.writes })
}

/// `emitted`, drawn through `draw`, as batches: a run of particles with
/// one tint and size is one. Nothing is read for no particle.
pub fn batches(m: &dyn GuestMemory, draw: u32, emitted: &[Emitted]) -> Result<Vec<Batch>, &'static str> {
    if emitted.is_empty() {
        return Ok(Vec::new());
    }
    let r = |at: u32| m.u32(at).map_err(|_| "fault");
    let (definition, bitmap) = (r(draw)?, r(draw.wrapping_add(4))?);
    let (flags, class) = (r(definition)?, r(definition.wrapping_add(4))? as u16);
    nm::check(flags, class, f32::from_bits(r(BYTE)?), f32::from_bits(r(TINT)?)).map_err(nm::Refused::name)?;
    let mut out: Vec<Batch> = Vec::new();
    for e in emitted {
        let key = Key { definition, bitmap, tint: e.tint, size: e.size };
        let inputs = e.inputs.map(f32::from_bits);
        let spawn = nm::spawn(&inputs, e.position.map(f32::from_bits), e.words, e.packed, e.particle);
        match out.last_mut() {
            Some(b) if b.key == key => b.spawns.push(spawn),
            _ => out.push(Batch { key, spawns: vec![spawn] }),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const DRAW: u32 = 0x0041_0000;
    const DEF: u32 = 0x0041_0100;

    fn world() -> FlatMemory {
        let mut m = FlatMemory::new(0x0041_0000, 0x0005_0000);
        m.set_u32(DRAW, DEF).unwrap();
        m.set_u32(DRAW + 4, 0x0042_0000).unwrap();
        m.set_u32(BYTE, (1.0f32 / 255.0).to_bits()).unwrap();
        m.set_u32(TINT, 1.0f32.to_bits()).unwrap();
        m
    }

    fn emitted(particle: u32, size: f32) -> Emitted {
        let mut inputs = [0; 16];
        inputs[0] = 0.5f32.to_bits();
        inputs[1] = 1.0f32.to_bits();
        Emitted {
            particle,
            position: [1.0f32, 2.0, 3.0].map(f32::to_bits),
            axis: [0; 3],
            size: size.to_bits(),
            tint: [1.0f32; 4].map(f32::to_bits),
            inputs,
            words: 0x0001_0000,
            packed: 0x00ff_0000,
        }
    }

    #[test]
    fn a_run_of_one_tint_and_size_is_one_batch_in_order() {
        let e = [emitted(0x100, 1.0), emitted(0x140, 1.0), emitted(0x180, 2.0)];
        let b = batches(&world(), DRAW, &e).unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].key, Key { definition: DEF, bitmap: 0x0042_0000, tint: e[0].tint, size: e[0].size });
        let ids: Vec<u32> = b.iter().flat_map(|b| b.spawns.iter().map(|s| s.id)).collect();
        assert_eq!(ids, [0x100, 0x140, 0x180]);
        let s = &b[0].spawns[0];
        assert_eq!((s.position, s.age, s.lifetime), ([1.0, 2.0, 3.0], 1.0, 2.0));
        assert_eq!((s.words, s.tint), (0x0001_0000, 0xff), "red to the low byte");
    }

    #[test]
    fn the_mirrors_refusal_stands_and_no_particle_reads_nothing() {
        let mut m = world();
        m.set_u16(DEF + 4, 2).unwrap();
        assert_eq!(batches(&m, DRAW, &[emitted(0x100, 1.0)]), Err("orientation"));
        assert_eq!(batches(&m, 0x0100_0000, &[]), Ok(Vec::new()));
        assert_eq!(batches(&m, 0x0100_0000, &[emitted(0x100, 1.0)]), Err("fault"));
    }
}
