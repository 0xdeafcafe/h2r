//! Entry I's callbacks in Rust, chosen by the address each vtable slot
//! resolves to (docs/engine/collision-queries-entries.md, "Entry I").
//! [`call`] runs one [`GuestCall`] that [`super::CollectorCast::next`]
//! returns, with the registers and stack words it names, and gives back
//! the callee's EAX. A target with no port is
//! `unimplemented!("collision: mopp_ray: ...")` naming its address; no
//! callback runs in the guest.
//!
//! | Slot | Target | Port |
//! |---|---|---|
//! | filter `vt[0]` | `0x002df8f0`, the group filter's ray interface | [`group_filter`] |
//! | collection `vt[+0x38]` (from the filter) | `0x0011ca70`, filter info 0 | [`group_filter`] |
//! | collection `vt[+0x34]` | `0x001ef950`, the level (vtable `0x004560f8`) | [`level_child`]: key types 1, 2, 5 and filtered keys |
//! | collection `vt[+0x34]` | `0x001ee4d0`, an object BSP (vtable `0x00456194`) | [`bsp_child`] |
//! | collection `vt[+0x34]` | `0x00244bb0`, instanced geometry (vtable `0x00459e0c`) | [`instance_child`] |
//! | child `vt[+0x24]` | `0x002df110`, the convex ray with a collector | [`convex_ray`] |
//! | child `vt[+0x24]` | `0x002dae30`, the empty shape (vtable `0x00456134`) | returns |
//! | child `vt[+0x24]` | `0x00244be0`, an instance key's shape (vtable `0x00459de4`) | [`instance_ray`] |
//! | child `vt[+0x24]` | `0x002deb20`, the transform shape (vtable `0x004553f0`) | [`transform_ray`] |
//! | child `vt[+0x24]` | `0x002fc0a0`, a shape wrapping one child | [`wrapper_ray`] |
//! | child `vt[+0x24]` | `0x002df720`, a MOPP tree shape: entry I again | [`tree_ray`] |
//! | convex `vt[+0x20]` | `0x001edf50`, the structure polygon | [`polygon_cast`] |
//! | collector `vt[0]` | `0x001829b0`, Halo's phantom ray collector (vtable `0x00453b88`) | [`phantom_collector`] |
//! | collector `vt[0]` | `0x00244c10`, entry E's instance collector (vtable `0x00459e48`) | [`instance_collector`] |
//!
//! SSE runs through [`Fp::guest`] under the guest's MXCSR, which gains the
//! flags it raises. The polygon's x87 length runs in the guest's precision
//! and declines (`float`) on an operand or result off the normal range;
//! the x87 status word is not modelled, as for the walk. Stack scratch the
//! callees keep below the entry ESP (hit records, the polygon's projected
//! ring) is dead at I's return and stays in Rust; the shapes the children
//! build go into the buffer the walk passes, as retail's do. The shapes
//! that nest (the transform, the wrapper, the tree) keep their locals in
//! guest memory at retail's stack addresses below the callee's ESP
//! ([`GuestCall::esp`]), because the nested walk and its callbacks read
//! them there.
use super::CollectorCast;
use crate::sim_collision::instance::{self, Query};
use crate::sim_collision::{CollisionMemory, layout};
use crate::sim_native::Memory;
use guest_layout::entry::GuestCall;
use runtime::CpuAbi;
use sim_collision::{Bail, CollisionWorld};
use sim_collision::mopp::ray::Precision;
use sim_core::det::{Fp, St, X87};
use sim_havok::mem::{Journal, Words};
use sim_havok::structure::{self, Args, Fallback};
use std::cmp::Ordering;

/// The addresses the slots resolve to, and the data they read.
pub mod target {
    pub const GROUP_FILTER: u32 = 0x002d_f8f0;
    pub const NO_FILTER_INFO: u32 = 0x0011_ca70;
    pub const LEVEL_CHILD: u32 = 0x001e_f950;
    pub const BSP_CHILD: u32 = 0x001e_e4d0;
    pub const INSTANCE_CHILD: u32 = 0x0024_4bb0;
    pub const CONVEX_RAY: u32 = 0x002d_f110;
    pub const EMPTY_RAY: u32 = 0x002d_ae30;
    pub const INSTANCE_RAY: u32 = 0x0024_4be0;
    pub const POLYGON_CAST: u32 = 0x001e_df50;
    pub const PHANTOM_COLLECTOR: u32 = 0x0018_29b0;
    pub const TRANSFORM_RAY: u32 = 0x002d_eb20;
    pub const WRAPPER_RAY: u32 = 0x002f_c0a0;
    pub const TREE_RAY: u32 = 0x002d_f720;
    pub const INSTANCE_COLLECTOR: u32 = 0x0024_4c10;
}

/// Shapes and globals the ports read or write.
pub mod data {
    /// The empty shape `0x001ef950` builds for a key its filter rejects.
    pub const EMPTY_SHAPE: u32 = 0x0045_6134;
    /// Entry E's instance mark epoch; the marks are
    /// [`instance::MARK_BASE`].
    pub const INSTANCE_EPOCH: u32 = 0x004e_7c1c;
    /// The shape `0x00244bb0` builds for an instanced-geometry key.
    pub const INSTANCE_SHAPE: u32 = 0x0045_9de4;
    /// The transform shape `0x002de890` builds for a level key of type 2.
    pub const TRANSFORM_SHAPE: u32 = 0x0045_53f0;
    /// `0x00279740`: the tag cache starts at `TAG_BASE`; when the byte at
    /// `TAG_LOADED` is set it spans `[TAG_SIZE]` bytes.
    pub const TAG_BASE: u32 = 0x8006_1000;
    pub const TAG_LOADED: u32 = 0x0054_76f8;
    pub const TAG_SIZE: u32 = 0x0054_771c;
    /// The instance definitions' collision collection: `+0xb4` of a
    /// definition (stride `0xc8` at structure `+0x13c`), its child tree at
    /// `+0x40`.
    pub const DEFINITION_COLLECTION: u32 = 0xb4;
    pub const COLLECTION_TREE: u32 = 0x40;
    /// The active structure's collision BSP.
    pub const STRUCTURE_BSP: u32 = 0x004e_0340;
    /// The active structure.
    pub const STRUCTURE: u32 = 0x004e_0348;
    /// The breakable-surface bits and the active structure's index into them.
    pub const BREAKABLE_BITS: u32 = 0x004e_d280;
    pub const BREAKABLE_INDEX: u32 = 0x0046_86c4;
    /// Key types 3 and 4: the per-index object table and the object array.
    pub const KEY_OBJECTS: u32 = 0x0051_e9cc;
    pub const OBJECT_ARRAY: u32 = 0x004e_0300;
    /// `1.0`, `-1.0` and the length below which the normal stays unscaled.
    pub const ONE: u32 = 0x0045_dbc0;
    pub const NEG_ONE: u32 = 0x0045_dbcc;
    pub const MIN_LENGTH: u32 = 0x0045_dbdc;
    /// The projection rows, `(2 * axis + flip) * 6` bytes in: `u`, `v` as
    /// `i16`.
    pub const PROJECTION: u32 = 0x0044_0b94;
    /// The polygon cast's projected ring sits `0x40` below its entry ESP,
    /// eight bytes a vertex: a ninth would overwrite its return address.
    pub const MAX_CAST_RING: u32 = 8;
}

/// The hit record a ray method fills (`+0` normal, `+0x10` key, `+0x14`
/// fraction), kept in Rust.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub normal: [u32; 4],
    pub key: u32,
    pub fraction: u32,
}

fn rd(g: &impl Memory, at: u32) -> Result<u32, Bail> {
    g.u32(at).map_err(|_| Bail::Fault)
}

fn rd_i16(g: &impl Memory, at: u32) -> Result<i32, Bail> {
    Ok(i32::from(g.u16(at).map_err(|_| Bail::Fault)? as i16))
}

fn rd3(g: &impl Memory, at: u32) -> Result<[u32; 3], Bail> {
    Ok([rd(g, at)?, rd(g, at.wrapping_add(4))?, rd(g, at.wrapping_add(8))?])
}

fn rd_u8(g: &impl Memory, at: u32) -> Result<u8, Bail> {
    g.u8(at).map_err(|_| Bail::Fault)
}

fn wr(g: &mut impl CollisionMemory, at: u32, v: &[u8]) -> Result<(), Bail> {
    if !g.writable(at, v.len() as u32) {
        return Err(Bail::Fault);
    }
    g.write_bytes(at, v, 1).map_err(|_| Bail::Fault)
}

fn wr32(g: &mut impl CollisionMemory, at: u32, v: u32) -> Result<(), Bail> {
    wr(g, at, &v.to_le_bytes())
}

/// The target of `this`'s vtable slot.
fn method(g: &impl Memory, this: u32, slot: u32) -> Result<u32, Bail> {
    rd(g, rd(g, this)?.wrapping_add(slot))
}

/// Run one callback. `cpu` brings the MXCSR, the x87 control word and
/// I's entry ESP and takes back the MXCSR and the x87 flags raised; the
/// return is the callee's EAX.
pub fn call(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    cpu: &mut CpuAbi,
    c: &GuestCall,
) -> Result<u32, Bail> {
    let this = c.ecx.ok_or(Bail::Data)?;
    let arg = |k: usize| c.args.get(k).copied().ok_or(Bail::Data);
    let mut fp = Fp::guest(cpu.mxcsr);
    let eax = match c.target {
        target::GROUP_FILTER => group_filter(g, this, arg(0)?, arg(1)?, arg(2)?, arg(3)?)?,
        target::LEVEL_CHILD => level_child(g, arg(0)?, arg(1)?, cpu.mxcsr)?,
        target::BSP_CHILD => bsp_child(g, this, arg(0)?, arg(1)?, cpu.mxcsr)?,
        target::INSTANCE_CHILD => instance_child(g, this, arg(0)?, arg(1)?)?,
        target::EMPTY_RAY
        | target::CONVEX_RAY
        | target::INSTANCE_RAY
        | target::TRANSFORM_RAY
        | target::WRAPPER_RAY
        | target::TREE_RAY => {
            let call = RayCall {
                target: c.target,
                shape: this,
                eax: c.eax,
                input: arg(0)?,
                body: arg(1)?,
                collector: arg(2)?,
                esp: c.esp(cpu.esp),
            };
            shape_ray(g, world, cpu, &mut fp, &call)?
        }
        other => unimplemented!("collision: mopp_ray: callback {other:#010x}"),
    };
    settle(cpu, &fp)?;
    Ok(eax)
}

/// The SSE flags `fp` raised into the CPU, or `float` when it met an
/// operation it does not model.
fn settle(cpu: &mut CpuAbi, fp: &Fp) -> Result<(), Bail> {
    if !fp.exact() {
        return Err(Bail::Float);
    }
    if let Some(m) = fp.mxcsr() {
        cpu.mxcsr = m;
    }
    Ok(())
}

/// A child's ray method (`vt[+0x24]`, `ret 0xc`): `shape` in ECX, the
/// input, body and collector on the stack, `esp` the callee's ESP at its
/// first instruction, `eax` the caller's EAX.
#[derive(Clone, Copy, Debug)]
pub struct RayCall {
    pub target: u32,
    pub shape: u32,
    pub eax: Option<u32>,
    pub input: u32,
    pub body: u32,
    pub collector: u32,
    pub esp: u32,
}

/// One child ray method by its address; the return is its EAX.
pub fn shape_ray(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    cpu: &mut CpuAbi,
    fp: &mut Fp,
    c: &RayCall,
) -> Result<u32, Bail> {
    Ok(match c.target {
        // `ret 0xc`: EAX is what the caller left there.
        target::EMPTY_RAY => c.eax.unwrap_or(c.shape),
        target::CONVEX_RAY => {
            let precision = Precision::from_control(cpu.x87.control)?;
            convex_ray(g, world, cpu, fp, precision, c.shape, c.input, c.body, c.collector)?
        }
        target::INSTANCE_RAY => instance_ray(g, world, cpu, fp, c.body, c.collector)?,
        target::TRANSFORM_RAY => transform_ray(g, world, cpu, fp, c)?,
        target::WRAPPER_RAY => wrapper_ray(g, world, cpu, fp, c)?,
        target::TREE_RAY => tree_ray(g, world, cpu, fp, c)?,
        other => unimplemented!("collision: mopp_ray: child ray {other:#010x}"),
    })
}

/// Call `shape`'s `vt[+0x24]` from a nesting ray: the callee's ESP is
/// `esp`, below the three arguments and the return address.
#[allow(clippy::too_many_arguments)]
fn child_ray(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    cpu: &mut CpuAbi,
    fp: &mut Fp,
    shape: u32,
    eax: u32,
    [input, body, collector]: [u32; 3],
    esp: u32,
) -> Result<u32, Bail> {
    let target = method(g, shape, 0x24)?;
    for (k, v) in [input, body, collector].into_iter().enumerate() {
        wr32(g, esp.wrapping_add(4 + 4 * k as u32), v)?;
    }
    let call = RayCall { target, shape, eax: Some(eax), input, body, collector, esp };
    shape_ray(g, world, cpu, fp, &call)
}

/// The transform `0x002deb20` reads: three columns, then the translation.
fn transform(g: &impl Memory, at: u32) -> Result<[[u32; 4]; 4], Bail> {
    Ok([vec4(g, at)?, vec4(g, at.wrapping_add(0x10))?, vec4(g, at.wrapping_add(0x20))?, vec4(g, at.wrapping_add(0x30))?])
}

/// `0x002deb20`'s packed inverse of `p`: `d = p - t`, then lane `j` of
/// `(r0 * d.x + r1 * d.y) + r2 * d.z` with row `j` the column `j` (lane 3
/// repeats column 2).
fn untransform(fp: &mut Fp, t: &[[u32; 4]; 4], p: [u32; 4]) -> [u32; 4] {
    let d: [u32; 4] = std::array::from_fn(|k| fp.sub(p[k], t[3][k]));
    std::array::from_fn(|j| {
        let row = t[j.min(2)];
        let y = fp.mul(row[1], d[1]);
        let x = fp.mul(row[0], d[0]);
        let z = fp.mul(row[2], d[2]);
        let xy = fp.add(x, y);
        fp.add(xy, z)
    })
}

/// `0x002da450` (ecx out, `a`, `b`): `out[k] = (a0 * b[k].x + a1 * b[k].y)
/// + a2 * b[k].z` for the three columns and the translation of `b`, then
/// the translation gains `a`'s.
fn compose(fp: &mut Fp, a: &[[u32; 4]; 4], b: &[[u32; 4]; 4]) -> [[u32; 4]; 4] {
    let mut out = [[0; 4]; 4];
    for (k, o) in out.iter_mut().enumerate() {
        *o = std::array::from_fn(|l| {
            let z = fp.mul(a[2][l], b[k][2]);
            let y = fp.mul(a[1][l], b[k][1]);
            let x = fp.mul(a[0][l], b[k][0]);
            let xy = fp.add(x, y);
            fp.add(xy, z)
        });
    }
    out[3] = std::array::from_fn(|l| fp.add(out[3][l], a[3][l]));
    out
}

fn wr_vec4(g: &mut impl CollisionMemory, at: u32, v: [u32; 4]) -> Result<(), Bail> {
    for (k, w) in v.into_iter().enumerate() {
        wr32(g, at.wrapping_add(4 * k as u32), w)?;
    }
    Ok(())
}

/// `0x002deb20`, `ret 0xc`: the transform shape's ray. `this+0x10` is the
/// child's transform, `this+0x0c` the child. Its frame (`S`, ESP after the
/// aligned 0xa8 bytes and ESI): the ray in the child's space at `S+0x1c`
/// (both ends inverse-transformed, the input's `+0x20` and `+0x24` kept),
/// a body at `S+0x0c` (the child, the body's key, the motion at `S+0x4c`,
/// the body) whose motion copies the body's first 0x20 bytes and composes
/// its transform (`[body+8] + 0x20`) with the shape's (`0x002da450`). The
/// child's ray runs with them and the same collector.
pub fn transform_ray(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    cpu: &mut CpuAbi,
    fp: &mut Fp,
    c: &RayCall,
) -> Result<u32, Bail> {
    let aligned = c.esp.wrapping_sub(4) & !15;
    let s = aligned.wrapping_sub(0xac);
    let local = transform(g, c.shape.wrapping_add(0x10))?;
    let from = untransform(fp, &local, vec4(g, c.input)?);
    let to = untransform(fp, &local, vec4(g, c.input.wrapping_add(0x10))?);
    let info = rd(g, c.input.wrapping_add(0x20))?;
    let filter = rd(g, c.input.wrapping_add(0x24))?;
    let motion = rd(g, c.body.wrapping_add(8))?;
    let head = [vec4(g, motion)?, vec4(g, motion.wrapping_add(0x10))?];
    let composed = compose(fp, &transform(g, motion.wrapping_add(0x20))?, &local);
    let input = s.wrapping_add(0x1c);
    wr_vec4(g, input, from)?;
    wr_vec4(g, input.wrapping_add(0x10), to)?;
    wr32(g, input.wrapping_add(0x20), info)?;
    wr32(g, input.wrapping_add(0x24), filter)?;
    let moved = s.wrapping_add(0x4c);
    for (k, v) in head.into_iter().chain(composed).enumerate() {
        wr_vec4(g, moved.wrapping_add(0x10 * k as u32), v)?;
    }
    let child = rd(g, c.shape.wrapping_add(0x0c))?;
    let key = rd(g, c.body.wrapping_add(4))?;
    let body = s.wrapping_add(0x0c);
    for (k, v) in [child, key, moved, c.body].into_iter().enumerate() {
        wr32(g, body.wrapping_add(4 * k as u32), v)?;
    }
    child_ray(g, world, cpu, fp, child, input, [input, body, c.collector], s.wrapping_sub(0x14))
}

/// `0x002fc0a0`, `ret 0xc`: a shape that hands the ray to its child
/// (`this+0x0c`) with a body 0x10 below its ESP: the child, the body's key
/// and `+8`, the body.
pub fn wrapper_ray(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    cpu: &mut CpuAbi,
    fp: &mut Fp,
    c: &RayCall,
) -> Result<u32, Bail> {
    let frame = c.esp.wrapping_sub(0x10);
    let child = rd(g, c.shape.wrapping_add(0x0c))?;
    let info = rd(g, c.body.wrapping_add(8))?;
    let key = rd(g, c.body.wrapping_add(4))?;
    for (k, v) in [child, key, info, c.body].into_iter().enumerate() {
        wr32(g, frame.wrapping_add(4 * k as u32), v)?;
    }
    // `mov eax, [ecx]; call [eax+0x24]`: EAX is the child's vtable.
    let table = rd(g, child)?;
    child_ray(g, world, cpu, fp, child, table, [c.input, frame, c.collector], frame.wrapping_sub(0x10))
}

/// `0x002df720`, `ret 0xc`: a MOPP tree shape's ray. It builds the VM
/// 0x70 below its aligned frame with no filter (`0x002df670`, `+0x44`
/// zero) and calls entry I with the shape's collection (`+0x0c`) and code
/// (`+0x10`) and its own three arguments: I runs again in Rust
/// ([`CollectorCast`]), below this frame. The return is I's EAX.
pub fn tree_ray(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    cpu: &mut CpuAbi,
    fp: &mut Fp,
    c: &RayCall,
) -> Result<u32, Bail> {
    let aligned = c.esp.wrapping_sub(4) & !15;
    let vm = aligned.wrapping_sub(0x70);
    wr32(g, vm.wrapping_add(0x44), 0)?;
    let collection = rd(g, c.shape.wrapping_add(0x0c))?;
    let code = rd(g, c.shape.wrapping_add(0x10))?;
    let esp = aligned.wrapping_sub(0x98);
    for (k, v) in [collection, code, c.input, c.body, c.collector].into_iter().enumerate() {
        wr32(g, esp.wrapping_add(4 + 4 * k as u32), v)?;
    }
    // The nested walk starts from the flags raised so far.
    settle(cpu, fp)?;
    let mut nested = *cpu;
    (nested.esp, nested.ecx, nested.eax) = (esp, vm, code);
    let mut cast = CollectorCast::begin(&nested, g, world)?;
    cast.run(g, &mut nested)?;
    cpu.mxcsr = nested.mxcsr;
    cpu.x87.status = nested.x87.status;
    *fp = Fp::guest(cpu.mxcsr);
    Ok(nested.eax)
}

/// `0x002df8f0`, `ret 0x10`: the group filter's ray-collection test.
/// `this` is the filter's ray interface (filter `+0x10`), whose bit table
/// starts at `+0x0c`. The key's filter info comes from the collection's
/// `vt[+0x38]`, the ray's from input `+0x20`. Writes the verdict byte at
/// `out` and returns `out`.
pub fn group_filter(
    g: &mut impl CollisionMemory,
    this: u32,
    out: u32,
    input: u32,
    collection: u32,
    key: u32,
) -> Result<u32, Bail> {
    let info = match method(g, collection, 0x38)? {
        // `xor eax, eax; ret 4`.
        target::NO_FILTER_INFO => 0,
        other => unimplemented!("collision: mopp_ray: filter info {other:#010x} (key {key:#x})"),
    };
    let ray = rd(g, input.wrapping_add(0x20))?;
    let pass = if (info ^ ray) & 0xffff_0000 == 0 && ray & 0xffff_0000 != 0 {
        false
    } else {
        let bits = rd(g, this.wrapping_add(0x0c + 4 * (ray & 31)))?;
        bits & (1 << (info & 31)) != 0
    };
    wr(g, out, &[u8::from(pass)])?;
    Ok(out)
}

/// `0x00184000`: whether breakable surface `index` of instance `instance`
/// (`-1` the structure) is intact.
fn breakable_intact(g: &impl Memory, index: u32, instance: u32) -> Result<bool, Bail> {
    if index == u32::MAX {
        return Ok(true);
    }
    let structure = rd_i16(g, data::BREAKABLE_INDEX)? as u32;
    let table = rd(g, data::BREAKABLE_BITS)?;
    let base = if instance == u32::MAX {
        (structure << 5).wrapping_add(table).wrapping_add(1)
    } else {
        (structure << 10)
            .wrapping_add(table)
            .wrapping_add(instance)
            .wrapping_add(0x201)
    };
    let word = rd(g, base.wrapping_add(((index as i32 >> 5) as u32).wrapping_mul(4)))?;
    Ok(word & (1 << (index & 31)) != 0)
}

/// `0x001ef3e0` (edx the key): whether the level's key names something
/// still there. Type 1 a structure surface, intact unless breakable and
/// broken; 2 always; 3 and 4 an object key; 5 an instance surface.
pub fn level_key(g: &impl Memory, key: u32) -> Result<bool, Bail> {
    let index = key & 0xffff;
    let part = (key >> 16) & 0x1fff;
    match key >> 29 {
        1 => {
            let bsp = rd(g, data::STRUCTURE_BSP)?;
            let surface = rd(g, bsp.wrapping_add(0x2c))?.wrapping_add(index.wrapping_mul(8));
            if rd_u8(g, surface.wrapping_add(4))? & 8 == 0 {
                return Ok(true);
            }
            let breakable = u32::from(rd_u8(g, surface.wrapping_add(5))?);
            breakable_intact(g, breakable, u32::MAX)
        }
        2 => Ok(true),
        3 | 4 => {
            let object = rd(g, rd(g, data::KEY_OBJECTS)?.wrapping_add(index.wrapping_mul(4)))?;
            if object == u32::MAX {
                return Ok(false);
            }
            let array = rd(g, rd(g, data::OBJECT_ARRAY)?.wrapping_add(0x44))?;
            let entry = array.wrapping_add((object & 0xffff).wrapping_mul(12));
            if rd_u8(g, entry.wrapping_add(2))? & 0x10 != 0 {
                return Ok(false);
            }
            let datum = rd(g, entry.wrapping_add(8))?;
            let offset = rd_i16(g, datum.wrapping_add(0x11a))? as u32;
            let region = g
                .u8(offset.wrapping_add(datum).wrapping_add(part & 31))
                .map_err(|_| Bail::Fault)? as i8;
            Ok(i32::from(region) == ((part >> 5) & 0xff) as i32)
        }
        5 => {
            let structure = rd(g, data::STRUCTURE)?;
            let instances = rd(g, structure.wrapping_add(0x144))?;
            let definition = rd_i16(g, instances.wrapping_add(index.wrapping_mul(0x58)).wrapping_add(0x34))?;
            let definitions = rd(g, structure.wrapping_add(0x13c))?;
            let bsp = (definition as u32).wrapping_mul(0xc8).wrapping_add(definitions);
            let surfaces = rd(g, bsp.wrapping_add(0x9c))?;
            let surface = surfaces.wrapping_add(part.wrapping_mul(8));
            if rd_u8(g, surface.wrapping_add(4))? & 8 == 0 {
                return Ok(true);
            }
            let breakable = u32::from(rd_u8(g, surface.wrapping_add(5))?);
            breakable_intact(g, breakable, index)
        }
        // Types 0, 6 and 7 index past retail's jump table.
        _ => Err(Bail::Data),
    }
}

/// The empty shape a rejected key gets: references 1, then the vtable.
fn empty_shape(g: &mut impl CollisionMemory, buffer: u32) -> Result<u32, Bail> {
    if buffer == 0 {
        return Ok(0);
    }
    wr(g, buffer.wrapping_add(6), &1u16.to_le_bytes())?;
    wr32(g, buffer.wrapping_add(8), 0)?;
    wr32(g, buffer, data::EMPTY_SHAPE)?;
    Ok(buffer)
}

struct View<'a, M>(&'a M);

impl<M: Memory> Words for View<'_, M> {
    fn get(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// The structure polygon builder `0x001edd00` into `a.shape`
/// (`sim_havok::structure::build`).
fn polygon(g: &mut impl CollisionMemory, a: &Args, mxcsr: u32) -> Result<u32, Bail> {
    let writes = {
        let view = View(&*g);
        let mut j = Journal::new(&view);
        structure::build(&mut j, a, mxcsr).map_err(|f| match f {
            Fallback::Memory => Bail::Fault,
            Fallback::Ring => Bail::Data,
            Fallback::Float | Fallback::Unobserved => Bail::Float,
        })?;
        j.finish()
    };
    for (at, v) in writes {
        wr32(g, at, v)?;
    }
    Ok(a.shape)
}

/// `0x00279740`: whether `at` is in the tag cache (signed compares, as
/// retail's).
fn in_tags(g: &impl Memory, at: u32) -> Result<bool, Bail> {
    let base = data::TAG_BASE as i32;
    if (at as i32) < base {
        return Ok(at == data::TAG_BASE);
    }
    let size = if rd_u8(g, data::TAG_LOADED)? != 0 { rd(g, data::TAG_SIZE)? } else { 0 };
    let end = size.wrapping_add(data::TAG_BASE) as i32;
    Ok(at as i32 <= end)
}

/// `0x00181f80` (ecx the instance, eax `out`): the instance's placement as
/// a transform, three columns from `+4`, `+0x10`, `+0x1c` and the position
/// at `+0x28`, each with a zero fourth lane.
fn instance_transform(g: &mut impl CollisionMemory, instance: u32, out: u32) -> Result<(), Bail> {
    for (k, from) in [4u32, 0x10, 0x1c, 0x28].into_iter().enumerate() {
        let at = out.wrapping_add(0x10 * k as u32);
        for l in 0..3 {
            wr32(g, at.wrapping_add(4 * l), rd(g, instance.wrapping_add(from + 4 * l))?)?;
        }
        wr32(g, at.wrapping_add(12), 0)?;
    }
    Ok(())
}

/// The instance `index` of the active structure and its definition's
/// collision collection.
fn instance_collection(g: &impl Memory, index: u32) -> Result<(u32, u32), Bail> {
    let structure = rd(g, data::STRUCTURE)?;
    let instance = index.wrapping_mul(0x58).wrapping_add(rd(g, structure.wrapping_add(0x144))?);
    let definition = rd_i16(g, instance.wrapping_add(0x34))? as u32;
    let definitions = rd(g, structure.wrapping_add(0x13c))?;
    let collection = rd(
        g,
        definition
            .wrapping_mul(0xc8)
            .wrapping_add(definitions)
            .wrapping_add(data::DEFINITION_COLLECTION),
    )?;
    Ok((instance, collection))
}

/// `0x001ef950`, `ret 8`: the level collection's child for `key` in
/// `buffer`. A rejected key gets the empty shape; type 1 the surface's
/// polygon (flag 1, owner -1, tail the key); type 2 a transform shape
/// (`0x002de890`) over the instance definition's tree with the instance's
/// placement (`0x00181f80`); type 5 (`0x001ee500`) the definition
/// surface's polygon under the instance's transform (flag 0, the
/// collection's owner, tail the key). The rest (`0x001ef070`) have no port.
pub fn level_child(g: &mut impl CollisionMemory, key: u32, buffer: u32, mxcsr: u32) -> Result<u32, Bail> {
    if !level_key(&*g, key)? {
        return empty_shape(g, buffer);
    }
    let index = key & 0xffff;
    let part = (key >> 16) & 0x1fff;
    match key >> 29 {
        1 if buffer == 0 => Ok(0),
        1 => {
            let a = Args {
                shape: buffer,
                transform: 0,
                bsp: rd(g, data::STRUCTURE_BSP)?,
                flag: 1,
                owner: u32::MAX,
                surface: key & 0xffff,
                tail: key,
            };
            polygon(g, &a, mxcsr)
        }
        2 => {
            let (instance, collection) = instance_collection(&*g, index)?;
            let child = collection.wrapping_add(data::COLLECTION_TREE);
            if buffer != 0 {
                // `0x002de890`: references 1, the child, the identity
                // (`movaps`).
                if buffer & 15 != 0 {
                    return Err(Bail::Fault);
                }
                wr32(g, buffer.wrapping_add(0x0c), child)?;
                wr(g, buffer.wrapping_add(6), &1u16.to_le_bytes())?;
                wr32(g, buffer.wrapping_add(8), 0)?;
                wr32(g, buffer, data::TRANSFORM_SHAPE)?;
                let one = 1f32.to_bits();
                for k in 0..4 {
                    let column: [u32; 4] = std::array::from_fn(|l| if l == k && k < 3 { one } else { 0 });
                    wr_vec4(g, buffer.wrapping_add(0x10 + 0x10 * k as u32), column)?;
                }
                let refs = g.u16(child.wrapping_add(6)).map_err(|_| Bail::Fault)?;
                wr(g, child.wrapping_add(6), &refs.wrapping_add(1).to_le_bytes())?;
            }
            // The local reference is dropped again; a child that reaches
            // zero outside the tag cache is freed by its destructor.
            let refs = g.u16(child.wrapping_add(6)).map_err(|_| Bail::Fault)?.wrapping_sub(1);
            wr(g, child.wrapping_add(6), &refs.to_le_bytes())?;
            if refs == 0 {
                if !in_tags(&*g, child)? {
                    unimplemented!("collision: mopp_ray: level child 0x001ef950 frees shape {child:#010x} (vt[0])");
                }
                wr(g, child.wrapping_add(6), &1u16.to_le_bytes())?;
            }
            instance_transform(g, instance, buffer.wrapping_add(0x10))?;
            Ok(buffer)
        }
        5 if buffer == 0 => Ok(0),
        5 => {
            let (instance, collection) = instance_collection(&*g, index)?;
            let a = Args {
                shape: buffer,
                transform: instance,
                bsp: rd(g, collection.wrapping_add(0x0c))?,
                flag: 0,
                owner: rd(g, collection.wrapping_add(0x30))?,
                surface: part,
                tail: key,
            };
            polygon(g, &a, mxcsr)
        }
        kind => unimplemented!("collision: mopp_ray: level child 0x001ef950 key type {kind} ({key:#010x})"),
    }
}

/// `0x001ee4d0`, `ret 8`: an object BSP collection's child, the polygon of
/// surface `key` of the BSP at `this+0x0c`, owner `this+0x30`.
pub fn bsp_child(g: &mut impl CollisionMemory, this: u32, key: u32, buffer: u32, mxcsr: u32) -> Result<u32, Bail> {
    if buffer == 0 {
        return Ok(0);
    }
    let a = Args {
        shape: buffer,
        transform: 0,
        bsp: rd(g, this.wrapping_add(0x0c))?,
        flag: 0,
        owner: rd(g, this.wrapping_add(0x30))?,
        surface: key,
        tail: 0,
    };
    polygon(g, &a, mxcsr)
}

/// `0x00244bb0`, `ret 8`: the instanced-geometry collection's child, a
/// shape naming the collection's `+0x0c` and the key.
pub fn instance_child(g: &mut impl CollisionMemory, this: u32, key: u32, buffer: u32) -> Result<u32, Bail> {
    if buffer == 0 {
        return Ok(0);
    }
    let owner = rd(g, this.wrapping_add(0x0c))?;
    wr32(g, buffer.wrapping_add(0x0c), owner)?;
    wr(g, buffer.wrapping_add(6), &1u16.to_le_bytes())?;
    wr32(g, buffer.wrapping_add(8), 0)?;
    wr32(g, buffer, data::INSTANCE_SHAPE)?;
    wr32(g, buffer.wrapping_add(0x10), key)?;
    Ok(buffer)
}

/// `0x00244be0`, `ret 0xc`: an instance key's ray hands the collector a
/// hit at fraction one; the collector tests the instance.
pub fn instance_ray(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    cpu: &mut CpuAbi,
    fp: &mut Fp,
    body: u32,
    collector: u32,
) -> Result<u32, Bail> {
    let hit = Hit { normal: [0; 4], key: 0, fraction: rd(g, data::ONE)? };
    collect(g, world, cpu, fp, collector, body, &hit)
}

/// A collector's `vt[0]` (`addRayHit`: body, hit; `ret 8`).
fn collect(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    cpu: &mut CpuAbi,
    fp: &mut Fp,
    collector: u32,
    body: u32,
    hit: &Hit,
) -> Result<u32, Bail> {
    match method(g, collector, 0)? {
        target::PHANTOM_COLLECTOR => phantom_collector(g, fp, collector, body, hit),
        target::INSTANCE_COLLECTOR => instance_collector(g, world, cpu, fp, collector, body),
        other => unimplemented!("collision: mopp_ray: ray collector {other:#010x}"),
    }
}

/// `0x00244c10`, `ret 8`: entry E's collector. `this+4` the early-out
/// fraction, `+8` the query flags, `+0x0c` the BSP filter, `+0x10` and
/// `+0x14` the start and delta, `+0x18` the hit byte, `+0x1c` the result.
/// The mark `[0x4e7c20 + 4 * i]`, `i` the key's signed low half, equal to
/// the epoch `[0x4e7c1c]` skips the key; otherwise it takes the epoch
/// before `0x001691a0` tests the placement the full key names at the
/// result's `+4` ([`instance::placement`]). A hit writes the result, sets
/// the hit byte and lowers the fraction: to zero under flag `0x10000000`,
/// else to the result's when that is strictly lower. EAX is the key for a
/// marked key, the result after a lowered fraction and otherwise
/// `0x001691a0`'s AL (its upper bytes are not modelled; no caller of a
/// collector reads EAX).
pub fn instance_collector(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    cpu: &mut CpuAbi,
    fp: &mut Fp,
    this: u32,
    body: u32,
) -> Result<u32, Bail> {
    let key = rd(g, body.wrapping_add(4))?;
    let epoch = rd(g, data::INSTANCE_EPOCH)?;
    let mark = instance::MARK_BASE.wrapping_add((key as u16 as i16 as i32 as u32).wrapping_mul(4));
    if rd(g, mark)? == epoch {
        return Ok(key);
    }
    let result = rd(g, this.wrapping_add(0x1c))?;
    wr32(g, mark, epoch)?;
    let flags = rd(g, this.wrapping_add(8))?;
    let (start, delta) = (rd(g, this.wrapping_add(0x10))?, rd(g, this.wrapping_add(0x14))?);
    let q = Query {
        cluster: 0,
        start: rd3(&*g, start)?,
        delta: rd3(&*g, delta)?,
        flags,
        filter: rd(g, this.wrapping_add(0x0c))?,
        limit: rd(g, result.wrapping_add(4))?,
    };
    let structure = rd(g, layout::ACTIVE_STRUCTURE)?;
    let one = rd(g, layout::ONE)?;
    let mut x87 = 0u16;
    let hit = instance::placement(g, world, &q, structure, key, q.limit, one, cpu.x87.control, fp, &mut x87)?;
    cpu.x87.status |= x87 & 0x3f;
    let Some(hit) = hit else {
        return Ok(0);
    };
    let mut writes = Vec::new();
    hit.writes(|at, v| writes.push((result.wrapping_add(at), v.to_vec())));
    for (at, v) in writes {
        wr(g, at, &v)?;
    }
    if flags & 0x1000_0000 != 0 {
        wr32(g, this.wrapping_add(4), 0)?;
        wr(g, this.wrapping_add(0x18), &[1])?;
        return Ok(1);
    } else {
        let fraction = rd(g, result.wrapping_add(4))?;
        if gt(fp, rd(g, this.wrapping_add(4))?, fraction) {
            wr32(g, this.wrapping_add(4), fraction)?;
        }
    }
    wr(g, this.wrapping_add(0x18), &[1])?;
    Ok(result)
}

fn vec4(g: &impl Memory, at: u32) -> Result<[u32; 4], Bail> {
    // `movaps`.
    if at & 15 != 0 {
        return Err(Bail::Fault);
    }
    let mut out = [0; 4];
    for (k, o) in out.iter_mut().enumerate() {
        *o = rd(g, at.wrapping_add(4 * k as u32))?;
    }
    Ok(out)
}

/// `0x002df110`, `ret 0xc`: a convex shape's ray with a collector. The
/// shape's `vt[+0x20]` casts with the collector's fraction (`+4`); on a hit
/// the normal is rotated by the body's transform (`[body+8] + 0x20`, three
/// columns) and the hit, with the body's key, goes to the collector.
#[allow(clippy::too_many_arguments)]
pub fn convex_ray(
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
    cpu: &mut CpuAbi,
    fp: &mut Fp,
    precision: Precision,
    shape: u32,
    input: u32,
    body: u32,
    collector: u32,
) -> Result<u32, Bail> {
    let fraction = rd(g, collector.wrapping_add(4))?;
    let hit = match method(g, shape, 0x20)? {
        target::POLYGON_CAST => polygon_cast(&*g, fp, precision, shape, input, fraction)?,
        other => unimplemented!("collision: mopp_ray: convex ray cast {other:#010x}"),
    };
    let Some(mut hit) = hit else { return Ok(0) };
    hit.key = rd(g, body.wrapping_add(4))?;
    let basis = rd(g, body.wrapping_add(8))?.wrapping_add(0x20);
    let cols = [vec4(g, basis)?, vec4(g, basis.wrapping_add(0x10))?, vec4(g, basis.wrapping_add(0x20))?];
    let n = hit.normal;
    let mut out = [0; 4];
    for (lane, o) in out.iter_mut().enumerate() {
        let z = fp.mul(cols[2][lane], n[2]);
        let y = fp.mul(cols[1][lane], n[1]);
        let x = fp.mul(cols[0][lane], n[0]);
        let xy = fp.add(x, y);
        *o = fp.add(xy, z);
    }
    hit.normal = out;
    collect(g, world, cpu, fp, collector, body, &hit)
}

/// `ucomiss`: a NaN operand declines rather than modelling which raise IE.
fn ucomiss(fp: &mut Fp, a: u32, b: u32) -> Result<Ordering, Bail> {
    let nan = |x: u32| x & 0x7fff_ffff > 0x7f80_0000;
    if nan(a) || nan(b) {
        return Err(Bail::Float);
    }
    fp.comiss(a, b).ok_or(Bail::Float)
}

fn gt(fp: &mut Fp, a: u32, b: u32) -> bool {
    fp.comiss(a, b) == Some(Ordering::Greater)
}

/// A binary32 value the x87 sequence may take: zero or normal.
fn plain32(bits: u32) -> bool {
    let e = (bits >> 23) & 0xff;
    bits & 0x7fff_ffff == 0 || (e != 0 && e != 0xff)
}

/// `fld; fmul` the three squares, `faddp` them as `(z² + y²) + x²`,
/// `fsqrt`, then `fst m32`: the stored length and `|st0|`. Declines a
/// result the host formats would not round as the x87 does.
fn x87_length(precision: Precision, [x, y, z]: [u32; 3]) -> Result<(u32, Magnitude), Bail> {
    if ![x, y, z].iter().all(|&b| plain32(b)) {
        return Err(Bail::Float);
    }
    match precision {
        Precision::Single => {
            let f = f32::from_bits;
            let ok = |r: f32, nonzero: bool| plain32(r.to_bits()) && (r != 0.0 || !nonzero);
            let (sz, sy, sx) = (f(z) * f(z), f(y) * f(y), f(x) * f(x));
            for (s, v) in [(sz, z), (sy, y), (sx, x)] {
                if !ok(s, f(v) != 0.0) {
                    return Err(Bail::Float);
                }
            }
            let sum = (sz + sy) + sx;
            if !plain32(sum.to_bits()) {
                return Err(Bail::Float);
            }
            let r = sum.sqrt();
            Ok((r.to_bits(), Magnitude::Host(f64::from(r.abs()))))
        }
        Precision::Double => {
            // Squares of binary32 values are exact and in range in binary64.
            let f = |b: u32| f64::from(f32::from_bits(b));
            let r = ((f(z) * f(z) + f(y) * f(y)) + f(x) * f(x)).sqrt();
            let stored = r as f32;
            if !plain32(stored.to_bits()) || (stored == 0.0 && r != 0.0) {
                return Err(Bail::Float);
            }
            Ok((stored.to_bits(), Magnitude::Host(r.abs())))
        }
        Precision::Extended => {
            // On the x87 model: the 64-bit sum can round where binary64's
            // would not.
            let fpu = X87::new(EXTENDED_CONTROL);
            let square = |b: u32| fpu.mul(fpu.load32(b), fpu.load32(b));
            let r = fpu.sqrt(fpu.add(fpu.add(square(z), square(y)), square(x)));
            let stored = fpu.store32(r);
            let nonzero = [x, y, z].iter().any(|&b| b & 0x7fff_ffff != 0);
            if !plain32(stored) || (stored & 0x7fff_ffff == 0 && nonzero) {
                return Err(Bail::Float);
            }
            Ok((stored, Magnitude::Wide(fpu.abs(r))))
        }
    }
}

/// The masked, round-to-nearest control word at 64-bit precision.
const EXTENDED_CONTROL: u16 = 0x037f;

/// `|st0|` after [`x87_length`], in the format its precision left it.
enum Magnitude {
    Host(f64),
    Wide(St),
}

impl Magnitude {
    /// `min <= |st0|` as the `fcom` that follows decides it.
    fn at_least(&self, min: u32) -> bool {
        match *self {
            Magnitude::Host(m) => f64::from(f32::from_bits(min)) <= m,
            Magnitude::Wide(m) => {
                let fpu = X87::new(EXTENDED_CONTROL);
                matches!(fpu.compare(fpu.load32(min), m, false), Some(Ordering::Less | Ordering::Equal))
            }
        }
    }
}

/// `0x00120850` (eax the vector): the axis of the largest magnitude, z
/// before y before x on ties.
fn major_axis(fp: &mut Fp, n: [u32; 3]) -> usize {
    let [x, y, z] = n.map(|v| v & 0x7fff_ffff);
    // `fcomip`: `jb` is taken on less or unordered.
    let at_least = |a: u32, b: u32| f32::from_bits(a) >= f32::from_bits(b);
    if at_least(z, y) && fp.comiss(z, x).is_some_and(|o| o != Ordering::Less) {
        2
    } else if fp.comiss(y, x).is_some_and(|o| o != Ordering::Less) {
        1
    } else {
        0
    }
}

/// `0x0023a220` (edx the ring, ebx the point; count, tolerance; `ret 8`)
/// with tolerance 0: no edge has the point strictly on its right.
fn inside(fp: &mut Fp, ring: &[[u32; 2]], p: [u32; 2]) -> bool {
    let limit = fp.sub(0, 0);
    let n = ring.len();
    for i in 0..n {
        let a = ring[i];
        let b = ring[if i + 1 >= n { 0 } else { i + 1 }];
        let dv = fp.sub(p[1], a[1]);
        let du = fp.sub(p[0], a[0]);
        let eu = fp.sub(b[0], a[0]);
        let ev = fp.sub(b[1], a[1]);
        let l = fp.mul(dv, eu);
        let r = fp.mul(du, ev);
        let c = fp.sub(l, r);
        if gt(fp, limit, c) {
            return false;
        }
    }
    true
}

/// `0x001edf50` (out, input, hit; `ret 0xc`): the structure polygon's ray.
/// The plane comes from the first three vertices (`(v1 - v0) x (v2 - v0)`),
/// normalised unless its x87 length is below `[0x0045dbdc]`. A ray
/// parallel to it has fraction 0. The fraction must be below the hit's and
/// within `[0, 1]`, and the hit point, projected by the normal's major
/// axis and sign (`0x00440b94`), inside the ring. A hit has the normal and
/// the fraction.
pub fn polygon_cast(
    g: &impl Memory,
    fp: &mut Fp,
    precision: Precision,
    shape: u32,
    input: u32,
    fraction: u32,
) -> Result<Option<Hit>, Bail> {
    let w = |off: u32| rd(g, input.wrapping_add(off));
    let (from, to) = ([w(0)?, w(4)?, w(8)?], [w(0x10)?, w(0x14)?, w(0x18)?]);
    let vertex = |i: u32| -> Result<[u32; 4], Bail> {
        let at = shape.wrapping_add(0x20 + 16 * i);
        Ok([rd(g, at)?, rd(g, at.wrapping_add(4))?, rd(g, at.wrapping_add(8))?, 0])
    };
    let (v0, v1, v2) = (vertex(0)?, vertex(1)?, vertex(2)?);
    let dz = fp.sub(to[2], from[2]);
    let dx = fp.sub(to[0], from[0]);
    let dy = fp.sub(to[1], from[1]);
    let e1x = fp.sub(v1[0], v0[0]);
    let e1z = fp.sub(v1[2], v0[2]);
    let e2x = fp.sub(v2[0], v0[0]);
    let e2z = fp.sub(v2[2], v0[2]);
    let e2y = fp.sub(v2[1], v0[1]);
    let a = fp.mul(e2y, e1z);
    let b = fp.mul(e1z, e2x);
    let e1y = fp.sub(v1[1], v0[1]);
    let c = fp.mul(e2z, e1y);
    let d = fp.mul(e1y, e2x);
    let mut nx = fp.sub(c, a);
    let e = fp.mul(e2y, e1x);
    let mut nz = fp.sub(e, d);
    let f = fp.mul(e2z, e1x);
    let mut ny = fp.sub(b, f);
    let (length, magnitude) = x87_length(precision, [nx, ny, nz])?;
    let min = rd(g, data::MIN_LENGTH)?;
    if !plain32(min) {
        return Err(Bail::Float);
    }
    if magnitude.at_least(min) {
        let inverse = fp.div(rd(g, data::ONE)?, length);
        nx = fp.mul(inverse, nx);
        ny = fp.mul(ny, inverse);
        nz = fp.mul(nz, inverse);
    }
    let px = fp.sub(from[0], v0[0]);
    let py = fp.sub(from[1], v0[1]);
    let pz = fp.sub(from[2], v0[2]);
    let sz = fp.mul(nz, dz);
    let sy = fp.mul(ny, dy);
    let s = fp.add(sz, sy);
    let sx = fp.mul(nx, dx);
    let denominator = fp.add(s, sx);
    let t = if ucomiss(fp, denominator, 0)? == Ordering::Equal {
        0
    } else {
        let qz = fp.mul(nz, pz);
        let qy = fp.mul(ny, py);
        let q = fp.add(qz, qy);
        let neg_one = rd(g, data::NEG_ONE)?;
        let qx = fp.mul(nx, px);
        let q = fp.add(q, qx);
        let scale = fp.div(neg_one, denominator);
        fp.mul(q, scale)
    };
    if !gt(fp, fraction, t) {
        return Ok(None);
    }
    let one = rd(g, data::ONE)?;
    let clamped = if gt(fp, 0, t) {
        0
    } else if gt(fp, t, one) {
        one
    } else {
        t
    };
    if ucomiss(fp, clamped, t)? != Ordering::Equal {
        return Ok(None);
    }
    let n = [nx, ny, nz];
    let axis = major_axis(fp, n);
    let flip = u32::from(gt(fp, n[axis], 0));
    let row = data::PROJECTION.wrapping_add((flip + 2 * axis as u32) * 6);
    let (u, v) = (rd_i16(g, row)?, rd_i16(g, row.wrapping_add(2))?);
    if !(0..3).contains(&u) || !(0..3).contains(&v) {
        return Err(Bail::Data);
    }
    let hx = fp.mul(t, dx);
    let hx = fp.add(hx, from[0]);
    let hy = fp.mul(dy, t);
    let hy = fp.add(hy, from[1]);
    let hz = fp.mul(dz, t);
    let hz = fp.add(hz, from[2]);
    let point = [hx, hy, hz];
    let count = rd(g, shape.wrapping_add(0x14))?;
    if count > data::MAX_CAST_RING {
        return Err(Bail::Data);
    }
    let mut ring = Vec::with_capacity(count as usize);
    for i in 0..count {
        let at = shape.wrapping_add(0x20 + 16 * i);
        ring.push([rd(g, at.wrapping_add(4 * u as u32))?, rd(g, at.wrapping_add(4 * v as u32))?]);
    }
    if !inside(fp, &ring, [point[u as usize], point[v as usize]]) {
        return Ok(None);
    }
    Ok(Some(Hit { normal: [nx, ny, nz, 0], key: 0, fraction: t }))
}

/// `0x001829b0`, `ret 8`: Halo's phantom ray collector. `this+8` is the
/// result (`+0x0c` the ray's direction, `+0x18` fraction, `+0x1c` normal),
/// `this+0x0c` an object to ignore (`-1` none). A hit below one and the
/// result's fraction, not on the ignored object (the root body's owner at
/// `+0x20` when `+0x18` is 1, `0x001cf280`), and facing the ray is kept:
/// the hit byte `+0x10`, the result and the early-out fraction `+4`.
pub fn phantom_collector(
    g: &mut impl CollisionMemory,
    fp: &mut Fp,
    this: u32,
    body: u32,
    hit: &Hit,
) -> Result<u32, Bail> {
    let one = rd(g, data::ONE)?;
    if !gt(fp, one, hit.fraction) {
        return Ok(this);
    }
    let f = hit.fraction;
    let out = rd(g, this.wrapping_add(8))?;
    if !gt(fp, rd(g, out.wrapping_add(0x18))?, f) {
        return Ok(this);
    }
    let ignored = rd(g, this.wrapping_add(0x0c))?;
    if ignored != u32::MAX {
        // `0x00183da0`: up the parents at `+0x0c`.
        let mut root = body;
        loop {
            let parent = rd(g, root.wrapping_add(0x0c))?;
            if parent == 0 {
                break;
            }
            root = parent;
        }
        let owner = if rd(g, root.wrapping_add(0x18))? == 1 {
            rd(g, root.wrapping_add(0x20))?
        } else {
            0
        };
        if owner != 0 && object_of(g, owner)? == ignored {
            return Ok(this);
        }
    }
    let n = hit.normal;
    let z = fp.mul(rd(g, out.wrapping_add(0x14))?, n[2]);
    let y = fp.mul(rd(g, out.wrapping_add(0x10))?, n[1]);
    let x = fp.mul(rd(g, out.wrapping_add(0x0c))?, n[0]);
    let zy = fp.add(z, y);
    let facing = fp.add(zy, x);
    if !gt(fp, 0, facing) {
        return Ok(this);
    }
    wr(g, this.wrapping_add(0x10), &[1])?;
    wr32(g, out.wrapping_add(0x18), f)?;
    let out = rd(g, this.wrapping_add(8))?;
    for (k, &c) in n[..3].iter().enumerate() {
        wr32(g, out.wrapping_add(0x1c + 4 * k as u32), c)?;
    }
    wr32(g, this.wrapping_add(4), f)?;
    Ok(this)
}

/// `0x001cf280` (ecx the owner): the word after the first `0x2001` tag in
/// the owner's `{+0x30 address, +0x34 count}` pairs, else 0.
fn object_of(g: &impl Memory, owner: u32) -> Result<u32, Bail> {
    let count = rd(g, owner.wrapping_add(0x34))? as i32;
    let pairs = rd(g, owner.wrapping_add(0x30))?;
    for i in 0..count.max(0) as u32 {
        let at = pairs.wrapping_add(8 * i);
        if rd(g, at)? == 0x2001 {
            return rd(g, at.wrapping_add(4));
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests;
