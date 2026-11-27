//! The particle walk `0x00175610` in Rust: one system's emitters and
//! their particles, in the guest's order, each as the five arguments it
//! hands the per-particle callback (docs/graphics/native-particles.md,
//! "Walking a system's particles"). [`walk`] makes them, and the input
//! block's writes on the way, without calling the callback: a GPU
//! particle system takes its births from them.
//!
//! **Code** (`h2 disasm`): `0x00175610(system, location, callback, draw)`
//! (`ret 0x10`), EDI the input block (`particle_emission`'s `state`).
//!
//! 1. A new `system` takes the block's `+0x48` and clears inputs `0x670`;
//!    a new `location` takes `+0x4c` and clears `0x13080`. `0x001751d0`
//!    finds the system's definition (stride 0x38) through the tag of
//!    `system + 0x10`: an `effe`'s event `+0x14` and system `+0xe`, or a
//!    breakable surface's (`bsdt`) system `+0xe` of its `+0x18` block.
//! 2. The emitters: from `location + 4` through `[0x51ec88]` (stride
//!    0x4c, `+4` the first particle, `+8` the next emitter), beside the
//!    definition's emitters (`+0x34`, stride 0xb8). An emitter with no
//!    particle is passed by.
//! 3. Per emitter: its inputs (`+0xb4 & 0x107f0`, `0x00173ba0`), then the
//!    properties `+0xb0` keeps per emitter: bit 0 alpha (`+0x68`), bit 1
//!    colour (`+0x58`, `0x00246d80`), bit 2 size (`+0x48`, times the
//!    scale, 1).
//! 4. Per particle (`[0x51ec84]`, 0x40 bytes, `+4` the next): a new
//!    particle takes `+0x50` and clears inputs `0xf80f`; its inputs
//!    (`+0xb4 & 0xf80f`), the properties the emitter's bits leave per
//!    particle, then the callback with the particle, the block, its
//!    `+0x1c` position, its `+0x28` axis, the size, the tint (alpha,
//!    red, green, blue) and `draw`.
//!
//! Refused by name, before any emitter is evaluated: a system that scales
//! its size by `0x0003eb70` (`system + 0xc` bit 10), a local-space system
//! (definition `+0xc`, transformed by `0x00248450`), a definition the
//! resolver finds through another particle system (`prt3`, `PRTM`), and a
//! callback other than `0x00224550`, whose input caching ([`walk`]
//! models it, `particle_emission`'s step 1) the next particle reads.

use std::collections::BTreeMap;

use crate::function_eval::constant::ONE;
use crate::light_constants::Error;
use crate::memory::GuestMemory;
use crate::particle_emission::{self as pe, CALLBACK_INPUTS, Model};

/// The walk.
pub const ENTRY: u32 = 0x0017_5610;
/// The emitter instances (a pointer to the array header; `+0x44` its data).
pub const EMITTERS: u32 = 0x0051_ec88;
/// The particle instances (likewise).
pub const PARTICLES: u32 = 0x0051_ec84;
/// The tag instances (16 bytes each: group, ..., `+8` the data).
pub const TAGS: u32 = 0x004e_3b44;
/// The emitter's inputs (`+0xb4` masked).
pub const EMITTER_INPUTS: u32 = 0x107f0;
/// Emitters and particles a walk visits before it stops: the guest would
/// run on round a cycle in a list.
pub const LIMIT: usize = 1 << 16;

/// The walk's arguments and EDI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    pub system: u32,
    pub location: u32,
    pub callback: u32,
    pub draw: u32,
    /// EDI: the input block.
    pub state: u32,
}

impl Call {
    /// The walk entered with ESP `esp` (its return address) and EDI `edi`.
    pub fn at(m: &dyn GuestMemory, esp: u32, edi: u32) -> Result<Call, Error> {
        let a = |i: u32| m.u32(esp.wrapping_add(4 + 4 * i));
        Ok(Call { system: a(0)?, location: a(1)?, callback: a(2)?, draw: a(3)?, state: edi })
    }
}

/// One callback's arguments, by value, and what a birth reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Emitted {
    /// The particle record's address.
    pub particle: u32,
    /// Its `+0x1c` and `+0x28` (f32 bits).
    pub position: [u32; 3],
    pub axis: [u32; 3],
    /// The emitter's size and tint (alpha, red, green, blue), f32 bits.
    pub size: u32,
    pub tint: [u32; 4],
    /// The block's sixteen inputs once the callback has cached the
    /// definition's (`particle_mirror::spawn`'s `inputs`).
    pub inputs: [u32; 16],
    /// The particle's `+0x14` and `+0x3c`.
    pub words: u32,
    pub packed: u32,
}

/// The walk's answer: the callbacks in order and every byte of the input
/// block it and the callbacks' input caching write. The callback's
/// sequence store (`draw + 8`) is not among them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Walk {
    pub emitted: Vec<Emitted>,
    pub writes: BTreeMap<u32, u8>,
}

/// Why a walk is not modelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// `system + 0xc` bit 10: the size scale is `0x0003eb70`'s.
    Scale,
    /// The definition's `+0xc`: positions go through `0x00248450`.
    LocalSpace,
    /// The resolver's group is a particle system's (`prt3`, `PRTM`): its
    /// definition comes through a vtable.
    Nested(u32),
    /// The callback is not `0x00224550`.
    Callback(u32),
    /// More than [`LIMIT`] emitters and particles.
    Unbounded,
    /// A fault, an unmasked exception or tag data the evaluators refuse.
    Eval(Error),
}

impl From<Error> for Refused {
    fn from(e: Error) -> Self {
        Refused::Eval(e)
    }
}

impl From<crate::memory::MemFault> for Refused {
    fn from(f: crate::memory::MemFault) -> Self {
        Refused::Eval(f.into())
    }
}

impl Refused {
    /// The counter name (`particle_walk_refused_<name>`).
    pub fn name(self) -> &'static str {
        match self {
            Refused::Scale => "scale",
            Refused::LocalSpace => "local_space",
            Refused::Nested(_) => "nested",
            Refused::Callback(_) => "callback",
            Refused::Unbounded => "unbounded",
            Refused::Eval(e) => pe::refusal(e),
        }
    }
}

const EFFE: u32 = 0x6566_6665;
const PRTM: u32 = 0x5052_544d;
const PRT3: u32 = 0x7072_7433;
/// Breakable surfaces: their `+0x18` block holds the systems directly.
const BSDT: u32 = 0x6273_6474;

/// `0x001751d0`: the system's definition, 0 for a group it does not read.
fn definition(x: &Model<'_>, system: u32) -> Result<u32, Refused> {
    let tags = x.u32(TAGS)?;
    let tag = x.u32(system.wrapping_add(0x10))?;
    let entry = |i: u32| tags.wrapping_add(i.wrapping_mul(16));
    let group = x.u32(entry(tag as u16 as i16 as i32 as u32))?;
    let data = || x.u32(entry(tag & 0xffff).wrapping_add(8));
    let index = || -> Result<u32, Refused> { Ok(u32::from(x.u16(system.wrapping_add(0xe))?).wrapping_mul(0x38)) };
    Ok(match group {
        EFFE => {
            let event = x.u32(system.wrapping_add(0x14))?.wrapping_mul(0x38);
            let events = x.u32(data()?.wrapping_add(0x18))?;
            x.u32(events.wrapping_add(event).wrapping_add(0x34))?.wrapping_add(index()?)
        }
        BSDT => x.u32(data()?.wrapping_add(0x18))?.wrapping_add(index()?),
        PRTM | PRT3 => return Err(Refused::Nested(group)),
        _ => 0,
    })
}

/// The emitter properties of one callback: alpha, colour, size.
#[derive(Clone, Copy)]
struct Properties {
    alpha: u32,
    rgb: [u32; 3],
    size: u32,
}

impl Properties {
    /// Those whose `+0xb0` bit is `once` (set: per emitter), of the
    /// emitter definition `e`, the size times the scale `one`.
    fn evaluate(&mut self, x: &mut Model<'_>, e: u32, state: u32, once: bool, one: u32) -> Result<(), Refused> {
        let per = |x: &Model<'_>, bit: u32| -> Result<bool, Refused> { Ok((x.u32(e.wrapping_add(0xb0))? & bit != 0) == once) };
        if per(&*x, 1)? {
            self.alpha = x.property(state, e.wrapping_add(0x68))?;
        }
        if per(&*x, 2)? {
            self.rgb = x.colour(state, e.wrapping_add(0x58))?;
        }
        if per(&*x, 4)? {
            let v = x.property(state, e.wrapping_add(0x48))?;
            self.size = x.mul(v, one)?;
        }
        Ok(())
    }
}

/// The walk entered as `call`, under `mxcsr` and the x87 control word
/// `fcw`, to its return. Nothing is written to `m`.
pub fn walk(m: &dyn GuestMemory, call: Call, mxcsr: u32, fcw: u16) -> Result<Walk, Refused> {
    let mut x = Model::new(m, mxcsr, fcw)?;
    let Call { system, location, callback, draw, state } = call;
    let def = definition(&x, system)?;
    if system != x.u32(state + 0x48)? {
        let c = x.u32(state + 0x44)?;
        x.m.set_u32(state + 0x48, system)?;
        x.m.set_u32(state + 0x44, c & 0xffff_f98f)?;
    }
    if location != x.u32(state + 0x4c)? {
        let c = x.u32(state + 0x44)?;
        x.m.set_u32(state + 0x4c, location)?;
        x.m.set_u32(state + 0x44, c & 0xfffe_cf7f)?;
    }
    let mut emitted = Vec::new();
    let mut emitter = x.u32(location.wrapping_add(4))?;
    let mut k = 0u32;
    let mut steps = 0usize;
    while emitter != u32::MAX {
        steps += 1;
        let base = x.u32(x.u32(EMITTERS)?.wrapping_add(0x44))?;
        let instance = base.wrapping_add((emitter & 0xffff).wrapping_mul(0x4c));
        let mut particle = x.u32(instance.wrapping_add(4))?;
        let e = x.u32(def.wrapping_add(0x34))?.wrapping_add(k);
        if particle != u32::MAX {
            let one = x.u32(ONE)?;
            let mut props = Properties { alpha: 0, rgb: [0; 3], size: one };
            if (x.u16(system.wrapping_add(0xc))? >> 10) & 1 != 0 {
                return Err(Refused::Scale);
            }
            let mask = x.u32(e.wrapping_add(0xb4))?;
            if x.u16(def.wrapping_add(0xc))? != 0 {
                return Err(Refused::LocalSpace);
            }
            if callback != pe::ENTRY {
                return Err(Refused::Callback(callback));
            }
            x.cache(state, mask & EMITTER_INPUTS)?;
            props.evaluate(&mut x, e, state, true, one)?;
            while particle != u32::MAX {
                steps += 1;
                if steps > LIMIT {
                    return Err(Refused::Unbounded);
                }
                let records = x.u32(x.u32(PARTICLES)?.wrapping_add(0x44))?;
                let p = records.wrapping_add((particle & 0xffff) << 6);
                let v3 = |x: &Model<'_>, at: u32| -> Result<[u32; 3], Refused> {
                    Ok([x.u32(at)?, x.u32(at.wrapping_add(4))?, x.u32(at.wrapping_add(8))?])
                };
                let position = v3(&x, p.wrapping_add(0x1c))?;
                let axis = v3(&x, p.wrapping_add(0x28))?;
                if p != x.u32(state + 0x50)? {
                    let c = x.u32(state + 0x44)?;
                    x.m.set_u32(state + 0x50, p)?;
                    x.m.set_u32(state + 0x44, c & 0xffff_07f0)?;
                }
                x.cache(state, mask & CALLBACK_INPUTS)?;
                particle = x.u32(p.wrapping_add(4))?;
                props.evaluate(&mut x, e, state, false, one)?;
                // The callback's own caching (its step 1).
                let wanted = x.u32(x.u32(draw)?.wrapping_add(0x98))? & CALLBACK_INPUTS;
                x.cache(state, wanted)?;
                let mut inputs = [0; 16];
                for (i, v) in inputs.iter_mut().enumerate() {
                    *v = x.u32(state + 4 * i as u32)?;
                }
                emitted.push(Emitted {
                    particle: p,
                    position,
                    axis,
                    size: props.size,
                    tint: [props.alpha, props.rgb[0], props.rgb[1], props.rgb[2]],
                    inputs,
                    words: x.u32(p.wrapping_add(0x14))?,
                    packed: x.u32(p.wrapping_add(0x3c))?,
                });
            }
        }
        if steps > LIMIT {
            return Err(Refused::Unbounded);
        }
        emitter = x.u32(instance.wrapping_add(8))?;
        k = k.wrapping_add(0xb8);
    }
    Ok(Walk { emitted, writes: x.m.writes })
}

#[cfg(test)]
mod tests;
