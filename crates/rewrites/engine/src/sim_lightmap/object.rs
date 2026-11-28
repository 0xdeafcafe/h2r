//! The object sample `0x000d1850` and every function under it but the
//! sampler, in Rust (docs/engine/sim-render-split.md, "6. Render scratch
//! and stamps", the tick's record).
//!
//! | Guest | Here | Does |
//! |---|---|---|
//! | `0x000d1850` | [`sample`] | the object's lighting at its centre: the directions' samples, then the sky's for the kinds that take it; AL true when one succeeded |
//! | `0x000d2a50` | [`Run::directions`] | one or five rays from the centre, the first that hits sampled by `0x000d1e10` ([`super::sample`]) |
//! | `0x0014af40` | [`ray`] | a ray through entry J (`0x001697c0`) with up to three restarts, its hit resolved to a lightmap triangle |
//! | `0x0014ac60` | [`ray`] (`structure_hit`) | a structure or instance hit: the triangle under the hit and its barycentric weights |
//! | `0x000d1a30` | [`sky`] | the sky's lights blended into a colour and a direction, the colour the fallback when the sky is occluded |
//! | `0x000e5670`, `0x000bf9a0` | [`Run::unit_kind`], [`Run::model_lighting`] | a biped's byte `+0x3dc`; a model's render model with lighting blocks |
//!
//! Not ported: `0x0014a8e0`, the hit of an object whose render model has
//! lighting blocks (`+0x74`, `+0x7c`): it is `unimplemented!`.
//!
//! The guest's frames sit at the addresses the guest's calls give them,
//! below the caller's ESP: entry J reads and writes its start, delta and
//! record through pointers, and `0x0014ac60` reads a flag and three
//! vertices its frame may not have set, so they are guest memory here
//! too. Everything else is Rust state. SSE operations run under the
//! caller's MXCSR ([`Sse`]) and x87 ones under its control word ([`X87`]).
use super::store::Store;
use super::view::{View, is_token};
use crate::light_constants::{Error, Fpu, Sse, X87};
use crate::sim_native::Memory;
use runtime::{CpuAbi, GuestFault};

mod ray;
mod sky;
#[cfg(test)]
mod tests;

pub use ray::ray;
pub use sky::sky;

pub(crate) const OBJECTS: u32 = 0x004e_0300;
pub(crate) const TAGS: u32 = 0x004e_3b44;
const LIGHTMAP: u32 = 0x004e_0344;
pub(crate) const BSP: u32 = 0x004e_0348;
/// `[0x0045dc70]`: how far below the centre each direction's ray starts.
const PROBE: u32 = 0x0045_dc70;
/// Five directions of 12 bytes (`0x000d2a62`), and the one (`0x000d2a79`).
const FIVE: u32 = 0x0044_06f0;
const ONE_DIRECTION: u32 = 0x0044_06e4;
pub(crate) const NONE: u32 = u32::MAX;

/// Return addresses the guest's calls push, written where they would be.
pub(crate) mod ret {
    pub const DIRECTIONS: u32 = 0x000d_1915;
    pub const SKY: u32 = 0x000d_19f8;
    pub const RAY: u32 = 0x000d_2b55;
    pub const QUERY: u32 = 0x0014_afeb;
    pub const STRUCTURE_QUERY: u32 = 0x0014_b07d;
    pub const STRUCTURE_HIT: u32 = 0x0014_b08c;
    pub const SKY_QUERY: u32 = 0x000d_1c57;
}

/// Entry J with its six arguments at `esp + 4` and a return address at
/// `esp`: AL, the hit. [`collide`] is the simulation's.
pub trait Collide<M> {
    fn query(&mut self, g: &mut M, esp: u32) -> Result<bool, GuestFault>;
}

impl<M, F: FnMut(&mut M, u32) -> Result<bool, GuestFault>> Collide<M> for F {
    fn query(&mut self, g: &mut M, esp: u32) -> Result<bool, GuestFault> {
        self(g, esp)
    }
}

/// Entry J natively (`sim_collision::query::query`), under `cpu`'s float
/// state with ESP at `esp`. A declined plan has no answer here: it is
/// named and stops.
pub fn collide(cpu: &CpuAbi, g: &mut runtime::Guest, esp: u32) -> Result<bool, GuestFault> {
    use crate::sim_collision::{WORLD, query, take_bail};
    let world: &crate::sim_collision::CollisionWorld = &WORLD;
    let mut c = *cpu;
    c.esp = esp;
    world.observe_environment(sim_collision::Entry::Query, c.mxcsr, c.x87.control);
    let _ = take_bail();
    match query::query(&mut c, g, world)? {
        Some(_) => Ok(c.eax as u8 != 0),
        None => {
            let why = take_bail().map_or("declined", |(_, why)| why.name());
            unimplemented!("lightmap: the object sample's ray: collision query: {why}")
        }
    }
}

/// A float operation the port stops on, or the memory fault it hit.
pub(crate) fn fp<T>(r: Result<T, Error>) -> Result<T, GuestFault> {
    r.map_err(|e| match e {
        Error::Memory(f) => GuestFault {
            addr: f.addr,
            len: f.len as usize,
            access: if f.write {
                runtime::guest::Access::Write
            } else {
                runtime::guest::Access::Read
            },
            reason: runtime::guest::FaultReason::Unmapped,
        },
        e => unimplemented!("lightmap: the object sample's float operation stopped: {e:?}"),
    })
}

/// One call tree's state: guest memory, the store over it for reads, the
/// float state and entry J.
pub struct Run<'a, M: Memory, J: Collide<M>> {
    pub g: &'a mut M,
    pub store: &'a Store,
    pub(crate) sse: Sse,
    pub(crate) control: u16,
    pub j: J,
}

impl<'a, M: Memory, J: Collide<M>> Run<'a, M, J> {
    pub fn new(g: &'a mut M, store: &'a Store, fpu: Fpu, j: J) -> Self {
        Self {
            g,
            store,
            sse: Sse { mxcsr: fpu.mxcsr },
            control: fpu.x87_control,
            j,
        }
    }

    fn view(&self) -> View<'_, M> {
        View::new(&*self.g, self.store)
    }
    pub(crate) fn u8(&self, a: u32) -> Result<u8, GuestFault> {
        self.view().u8(a)
    }
    pub(crate) fn u16(&self, a: u32) -> Result<u16, GuestFault> {
        self.view().u16(a)
    }
    /// `movsx r32, word`.
    pub(crate) fn i16(&self, a: u32) -> Result<u32, GuestFault> {
        Ok(self.u16(a)? as i16 as i32 as u32)
    }
    pub(crate) fn u32(&self, a: u32) -> Result<u32, GuestFault> {
        self.view().u32(a)
    }
    pub(crate) fn vec3(&self, a: u32) -> Result<[u32; 3], GuestFault> {
        Ok([self.u32(a)?, self.u32(a.wrapping_add(4))?, self.u32(a.wrapping_add(8))?])
    }
    pub(crate) fn set_u8(&mut self, a: u32, v: u8) -> Result<(), GuestFault> {
        self.g.set_u8(a, v)
    }
    pub(crate) fn set_u16(&mut self, a: u32, v: u16) -> Result<(), GuestFault> {
        self.g.set_u16(a, v)
    }
    pub(crate) fn set_u32(&mut self, a: u32, v: u32) -> Result<(), GuestFault> {
        self.g.set_u32(a, v)
    }
    pub(crate) fn set_vec3(&mut self, a: u32, v: [u32; 3]) -> Result<(), GuestFault> {
        for (i, w) in v.into_iter().enumerate() {
            self.set_u32(a.wrapping_add(4 * i as u32), w)?;
        }
        Ok(())
    }
    pub(crate) fn mul(&mut self, a: u32, b: u32) -> Result<u32, GuestFault> {
        fp(self.sse.mul(a, b))
    }
    pub(crate) fn add(&mut self, a: u32, b: u32) -> Result<u32, GuestFault> {
        fp(self.sse.add(a, b))
    }
    pub(crate) fn sub(&mut self, a: u32, b: u32) -> Result<u32, GuestFault> {
        fp(self.sse.sub(a, b))
    }
    pub(crate) fn div(&mut self, a: u32, b: u32) -> Result<u32, GuestFault> {
        fp(self.sse.div(a, b))
    }
    /// `comiss a, b; ja`.
    pub(crate) fn above(&mut self, a: u32, b: u32) -> Result<bool, GuestFault> {
        fp(self.sse.above(a, b))
    }
    /// `comiss a, b; jb`.
    pub(crate) fn below(&mut self, a: u32, b: u32) -> Result<bool, GuestFault> {
        fp(self.sse.below(a, b))
    }
    pub(crate) fn x87(&self) -> X87 {
        X87::new(self.control)
    }

    /// The object's body: `[[OBJECTS] + 0x44] + (handle & 0xffff) * 12 + 8`.
    pub(crate) fn body(&self, object: u32) -> Result<u32, GuestFault> {
        let headers = self.u32(self.u32(OBJECTS)?.wrapping_add(0x44))?;
        self.u32(headers.wrapping_add((object & 0xffff) * 12 + 8))
    }

    /// A tag's address from its datum.
    pub(crate) fn tag(&self, datum: u32) -> Result<u32, GuestFault> {
        self.u32(self.u32(TAGS)?.wrapping_add(((datum & 0xffff) << 4) + 8))
    }

    /// The guard `0x000d1853`, `0x000d2a86` and `0x0014af6e` open with: a
    /// lightmap with groups, a structure, and the first group's bitmap
    /// set and built for this structure (its `+4` the structure's `+8`).
    pub(crate) fn lit(&self) -> Result<bool, GuestFault> {
        let lightmap = self.u32(LIGHTMAP)?;
        if lightmap == 0 || (self.u32(lightmap.wrapping_add(0x80))? as i32) <= 0 {
            return Ok(false);
        }
        let bsp = self.u32(BSP)?;
        if bsp == 0 {
            return Ok(false);
        }
        let group = self.u32(lightmap.wrapping_add(0x84))?;
        if self.u32(group.wrapping_add(0x1c))? == NONE {
            return Ok(false);
        }
        Ok(self.u32(group.wrapping_add(4))? == self.u32(bsp.wrapping_add(8))?)
    }

    /// `0x000e5670(eax = object)`: the body's byte `+0x3dc`.
    pub fn unit_kind(&self, object: u32) -> Result<u8, GuestFault> {
        self.u8(self.body(object)?.wrapping_add(0x3dc))
    }

    /// `0x000bf9a0(ecx = object, render, collision)`: the object's model
    /// (definition `+0x38`); when it has a render model (`+4`) and a
    /// collision model (`+0xc`), both are stored and AL is whether the
    /// render model has lighting blocks (`+0x74` and `+0x7c` counts above
    /// 0). With no model or either -1 nothing is stored.
    pub fn model_lighting(&mut self, object: u32, render: u32, collision: u32) -> Result<bool, GuestFault> {
        let definition = self.tag(self.u32(self.body(object)?)?)?;
        let model = self.u32(definition.wrapping_add(0x38))?;
        if model == NONE {
            return Ok(false);
        }
        let model = self.tag(model)?;
        let render_model = self.u32(model.wrapping_add(4))?;
        if render_model == NONE || self.u32(model.wrapping_add(0xc))? == NONE {
            return Ok(false);
        }
        let blocks = self.tag(render_model)?;
        let lit = (self.u32(blocks.wrapping_add(0x74))? as i32) > 0 && (self.u32(blocks.wrapping_add(0x7c))? as i32) > 0;
        self.set_u32(render, render_model)?;
        let collision_model = self.u32(model.wrapping_add(0xc))?;
        self.set_u32(collision, collision_model)?;
        Ok(lit)
    }

    /// `0x000d1e10(args)` by the port, its stores outside its frame
    /// committed as the hook commits them ([`super::answer`]).
    fn sampler(&mut self, args: [u32; 4]) -> Result<u32, GuestFault> {
        let sampled = super::sample::sample(&self.view(), args)?;
        for (addr, bytes) in sampled.writes.iter().filter(|(a, _)| !is_token(*a)) {
            self.g.write_bytes(*addr, bytes, 1)?;
        }
        Ok(sampled.eax)
    }

    /// `0x000d2a50` called at `entry` (its return address there), EBX
    /// `centre`; arguments the texture cache flags, the object, the ray's
    /// byte, the output and the direction table. EAX: 2 when no ray hit,
    /// else the last hit's `0x000d1e10` result; the loop stops at the
    /// first that is not 2.
    pub fn directions(&mut self, entry: u32, centre: u32, args: [u32; 5]) -> Result<u32, GuestFault> {
        let [flags, object, byte, out, table] = args;
        let mut result = 2;
        let (mut direction, count) = if flags & 1 != 0 {
            (FIVE, 5)
        } else if flags & 4 != 0 {
            (table, 1)
        } else {
            (ONE_DIRECTION, 1)
        };
        if !self.lit()? {
            return Ok(result);
        }
        // The frame: the ray's start at -0x50, the J start at -0x44, the
        // hit at -0x38; the ray's call pushes four words below -0x60.
        let point = entry.wrapping_sub(0x50);
        let start = entry.wrapping_sub(0x44);
        let hit = entry.wrapping_sub(0x38);
        let ray_entry = entry.wrapping_sub(0x74);
        for _ in 0..count {
            if result != 2 {
                break;
            }
            let probe = self.u32(PROBE)?;
            let c = self.vec3(centre)?;
            let d = self.vec3(direction)?;
            let mut p = [0; 3];
            for i in 0..3 {
                let along = self.mul(d[i], probe)?;
                p[i] = self.sub(c[i], along)?;
            }
            self.set_vec3(point, p)?;
            for (i, w) in [ret::RAY, direction, object, 1, hit].into_iter().enumerate() {
                self.set_u32(ray_entry.wrapping_add(4 * i as u32), w)?;
            }
            if ray(self, ray_entry, byte as u8, point, start, [direction, object, 1, hit])? {
                result = self.sampler([hit, out, flags, byte])?;
            }
            direction = direction.wrapping_add(12);
        }
        Ok(result)
    }
}

/// `0x000d1850` called at `entry` (its return address there), EDI
/// `object`, the arguments the ray's byte and the 0x14-byte output: AL,
/// true unless every sample it took failed (`0x000d1a13`: the failure
/// byte clear). With no lit structure it takes none and gives true.
pub fn sample<M: Memory, J: Collide<M>>(
    r: &mut Run<'_, M, J>,
    entry: u32,
    object: u32,
    [byte, out]: [u32; 2],
) -> Result<bool, GuestFault> {
    let mut failed = false;
    if !r.lit()? {
        return Ok(!failed);
    }
    let body = r.body(object)?;
    let definition = r.tag(r.u32(body)?)?;
    let mut flags = 0;
    if r.u32(body.wrapping_add(4))? >> 13 & 1 != 0 {
        flags = 1;
    }
    if r.u8(definition.wrapping_add(2))? >> 1 & 1 != 0 {
        flags |= 1;
    }
    let centre = body.wrapping_add(0x30);
    // `sub esp, 8` and three pushes, then five arguments and the call.
    let frame = entry.wrapping_sub(8);
    let directions = frame.wrapping_sub(0x24);
    let args = |flags| [flags, object, byte, out, 0];
    push_args(r, directions, ret::DIRECTIONS, &args(flags))?;
    let first = r.directions(directions, centre, args(flags))?;
    if !matches!(first, 1..=3) {
        return Ok(!failed);
    }
    failed = true;
    let mut last = first;
    if flags & 1 != 0 {
        // `0x000d1943`: again without bit 0; the failure byte stays set.
        push_args(r, directions, ret::DIRECTIONS, &args(flags & !1))?;
        last = r.directions(directions, centre, args(flags & !1))?;
    }
    if last == 3 {
        return Ok(!failed);
    }
    let kind = r.u16(definition)?;
    let takes_sky = match kind {
        1 => {
            let definition = r.tag(r.u32(r.body(object)?)?)?;
            let shift = r.u8(definition.wrapping_add(0x1f0))?;
            (1u32 << (shift & 31)) as u8 & 0x28 != 0
        }
        0 => r.unit_kind(object)? == 2,
        0xc => {
            let definition = r.tag(r.u32(r.body(object)?)?)?;
            r.u32(definition.wrapping_add(0xd4))? >> 4 & 1 != 0
        }
        _ => false,
    };
    if !takes_sky {
        return Ok(!failed);
    }
    let sky_entry = frame.wrapping_sub(0x1c);
    push_args(r, sky_entry, ret::SKY, &[object, centre, out])?;
    failed = matches!(sky(r, sky_entry, [object, centre, out])?, 1..=3);
    Ok(!failed)
}

/// A call's return address at `entry` and its arguments above.
pub fn push_args<M: Memory, J: Collide<M>>(
    r: &mut Run<'_, M, J>,
    entry: u32,
    ret: u32,
    args: &[u32],
) -> Result<(), GuestFault> {
    r.set_u32(entry, ret)?;
    for (i, a) in args.iter().enumerate() {
        r.set_u32(entry.wrapping_add(4 + 4 * i as u32), *a)?;
    }
    Ok(())
}
