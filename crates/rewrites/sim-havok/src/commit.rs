//! The contact commit: the contact manager's add (`0x00318c10`, vtable
//! `0x00415074` slot 1), which the add-point call `0x00302c70` makes for a
//! point no slot matches (docs/engine/havok-convex.md, "The contact
//! commit").
//!
//! - [`head`]: the call up to its listener walk. The constraint's point
//!   and property (`0x0031be80`, the island's sizes `0x00315f60`, the ID
//!   map `0x0031be20`), both bodies' point velocities (`0x003198b0`), the
//!   separating speed, friction and restitution, and the event on the
//!   frame. It makes no call out the model does not port.
//! - [`add`]: the whole call. The world's listener walk (`0x00318320`)
//!   and each body's (`0x00315d20`), a rejected contact's removal, then
//!   the impulse seed. The listeners (Halo's contact callback among
//!   them), the removal, the world add a first point needs (`0x0030d2b0`)
//!   and the array growth (`0x002d9160`, `0x002d91d0`) go through
//!   [`Calls`].
//!
//! Exact: SSE under the entry's MXCSR, the x87 under its control word,
//! memory read and written in the guest's order. A float the x87 code
//! loads must be finite.
use crate::fp::Exact;
use crate::jacobian::{Fpu, Fx, Sse, put_vector, shuf, vector};
use crate::mem::Journal;
use crate::solver::{finite, load};
use std::sync::atomic::{AtomicU64, Ordering};
use x87::{F80, Mode};

/// The manager's add.
pub const ADD: u32 = 0x0031_8c10;
/// The world's contact listener walk, cdecl `(world, event)`.
pub const WORLD_WALK: u32 = 0x0031_8320;
/// A body's contact listener walk, cdecl `(entity, event)`.
pub const ENTITY_WALK: u32 = 0x0031_5d20;
/// The return site of [`ADD`]'s call to [`WORLD_WALK`].
pub const WALK_SITE: u32 = 0x0031_8d9b;
/// The world add a constraint's first point makes, thiscall on the world.
pub const WORLD_ADD: u32 = 0x0030_d2b0;
/// Every motion class's `vt[+0x58]`: the velocity at a point.
pub const POINT_VELOCITY: u32 = 0x0031_98b0;
/// The island's `vt[+0x08]`: a constraint's solver sizes added.
pub const ISLAND_SIZES: u32 = 0x0031_5f60;
/// More properties than this and the manager refuses the point.
pub const MAX_PROPERTIES: i32 = 250;

/// `1e-10`-ish: keeps the effective mass finite.
const EPSILON: u32 = 0x0041_5068;
const ONE: u32 = 0x0045_dbc0;
/// `1.3`: the approach speed's scale in property `+0x04`.
const APPROACH: u32 = 0x0045_dff0;
/// `0.2`: the impulse seed's scale.
const SEED: u32 = 0x0045_dc5c;

/// The manager: its world, its constraint, the property count.
pub mod manager {
    pub const WORLD: u32 = 0x08;
    pub const CONSTRAINT: u32 = 0x10;
    pub const PROPERTIES: u32 = 0xa4;
}

/// The contact constraint ([`manager::CONSTRAINT`]).
pub mod constraint {
    pub const ISLAND: u32 = 0x14;
    pub const ID_MAP: u32 = 0x20;
    pub const FLAGS: u32 = 0x50;
    pub const POINTS: u32 = 0x34;
    pub const PROPERTIES: u32 = 0x90;
    pub const POINT_BYTES: u32 = 0x20;
    pub const PROPERTY_BYTES: u32 = 0x1c;
}

/// A collidable's parent (`+0x0c`) and, at the root, its entity (`+0x20`).
pub const COLLIDABLE_PARENT: u32 = 0x0c;
pub const COLLIDABLE_ENTITY: u32 = 0x20;
/// An entity's motion (`+0x3c`), friction (`+0x4c`), restitution
/// (`+0x50`), listeners (`+0x60`, count `+0x64`).
pub mod entity {
    pub const MOTION: u32 = 0x3c;
    pub const FRICTION: u32 = 0x4c;
    pub const RESTITUTION: u32 = 0x50;
    pub const LISTENERS: u32 = 0x60;
}
/// The world's contact listeners (`+0xb4`, count `+0xb8`).
pub const WORLD_LISTENERS: u32 = 0xb4;
/// The step input's restitution scale.
pub const INPUT_RESTITUTION: u32 = 0x30;

/// The event the listeners get, on the frame at `F + 0x5c`.
pub mod event {
    pub const AT: u32 = 0x5c;
    pub const MANAGER: u32 = 0x04;
    pub const A: u32 = 0x08;
    pub const B: u32 = 0x0c;
    /// The entity whose listeners run, `0` for the world's.
    pub const OWNER: u32 = 0x10;
    pub const POINT: u32 = 0x14;
    pub const PROPERTY: u32 = 0x18;
    pub const SPEED: u32 = 0x1c;
    /// `1`: a listener rejects the contact.
    pub const REJECT: u32 = 0x20;
}

/// Why a commit is not answered in Rust. Each is counted ([`fallbacks`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// A word the call reads is unmapped or unaligned, a vector is not
    /// 16-byte aligned, or a store is not plain writable memory.
    Memory,
    /// The constraint holds more than [`MAX_PROPERTIES`] properties.
    Full,
    /// The constraint's first point: the world adds it (`0x0030d2b0`).
    First,
    /// The point, property or ID array is at its capacity.
    Grow,
    /// A motion's `vt[+0x58]` is not [`POINT_VELOCITY`].
    Motion,
    /// The island's `vt[+0x08]` is not [`ISLAND_SIZES`].
    Island,
    /// An unmasked float exception, an x87 stack not empty at the entry,
    /// or a non-finite float the x87 code would load.
    Float,
    /// The guest answered an audited call whose listener walk was not
    /// reached, so it went uncompared.
    Unobserved,
}

impl Fallback {
    pub const ALL: [Fallback; 8] =
        [Self::Memory, Self::Full, Self::First, Self::Grow, Self::Motion, Self::Island, Self::Float, Self::Unobserved];

    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Full => "full",
            Self::First => "first",
            Self::Grow => "grow",
            Self::Motion => "motion",
            Self::Island => "island",
            Self::Float => "float",
            Self::Unobserved => "unobserved",
        }
    }
}

static FALLBACKS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];

/// Count a commit sent to the guest.
pub fn count(reason: Fallback) {
    FALLBACKS[reason as usize].fetch_add(1, Ordering::Relaxed);
}

/// The commit's declines since start, by reason, in [`Fallback::ALL`] order.
pub fn fallbacks() -> [(&'static str, u64); 8] {
    Fallback::ALL.map(|f| (f.name(), FALLBACKS[f as usize].load(Ordering::Relaxed)))
}

/// The calls the commit makes out. `esp` is the stack pointer at the
/// call's entry (at the return address).
pub trait Calls {
    /// `0x0030d2b0`, thiscall on the world, `(constraint)`, `ret 4`.
    fn world_add(&mut self, j: &mut Journal, world: u32, constraint: u32, esp: u32) -> Result<(), Fallback>;
    /// `0x002d9160`, cdecl `(array, capacity, element bytes)`.
    fn reserve(&mut self, j: &mut Journal, array: u32, capacity: u32, bytes: u32, esp: u32) -> Result<(), Fallback>;
    /// `0x002d91d0`, cdecl `(array, element bytes)`.
    fn expand(&mut self, j: &mut Journal, array: u32, bytes: u32, esp: u32) -> Result<(), Fallback>;
    /// A listener's `vt[+0x00]`, thiscall `(event)`, `ret 4`.
    fn listener(&mut self, j: &mut Journal, listener: u32, event: u32, esp: u32) -> Result<(), Fallback>;
    /// The manager's `vt[+0x08]`, thiscall `(id)`, `ret 4`: its eax.
    fn remove(&mut self, j: &mut Journal, manager: u32, id: u32, esp: u32) -> Result<u32, Fallback>;
}

/// [`ADD`]'s arguments: thiscall on `manager`, `(A, B, input, contact)`,
/// `ret 0x10`; `esp` at the entry (at the return address) and the
/// registers the call reads or keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Args {
    pub manager: u32,
    pub a: u32,
    pub b: u32,
    pub input: u32,
    /// The contact: the world point with the distance in `w`, then the
    /// normal.
    pub contact: u32,
    pub esp: u32,
    pub eax: u32,
    pub ebx: u32,
    pub ebp: u32,
    pub esi: u32,
    pub edi: u32,
}

/// The call at its listener walk (`call 0x00318320` at `0x00318d96`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Head {
    /// The frame: esp after the three pushes.
    pub frame: u32,
    /// The ID (ebx): `0x0031be80`'s eax.
    pub id: u32,
    pub world: u32,
    pub entity_a: u32,
    pub entity_b: u32,
    pub point: u32,
    pub property: u32,
}

impl Head {
    pub fn event(&self) -> u32 {
        self.frame.wrapping_add(event::AT)
    }

    /// The registers at the walk's entry: eax, ecx, edx, ebx, ebp, esi,
    /// edi; esp is [`Head::walk_esp`].
    pub fn walk_regs(&self, a: &Args) -> [u32; 7] {
        [a.manager, self.event(), self.world, self.id, a.esp.wrapping_sub(4), a.contact, a.b]
    }

    /// esp at the walk's entry: its two arguments and the return to
    /// [`WALK_SITE`] pushed.
    pub fn walk_esp(&self) -> u32 {
        self.frame.wrapping_sub(12)
    }
}

/// How [`add`] ended, with the registers it leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Committed {
    /// Added: eax the property address's high half and the ID, ecx and
    /// edx B's and A's motions.
    Added { eax: u32, ecx: u32, edx: u32 },
    /// A listener rejected it and the manager removed it: eax the
    /// removal's with `0xffff` in the low half.
    Rejected { eax: u32 },
    /// Past [`MAX_PROPERTIES`]: eax the entry's with `0xffff` in the low
    /// half, ecx the manager, edx the property count.
    Full { eax: u32, ecx: u32, edx: u32 },
}

fn word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    j.get(at).ok_or(Fallback::Memory)
}

fn x87_word(j: &Journal, at: u32) -> Result<F80, Fallback> {
    let v = word(j, at)?;
    if finite(v) { Ok(load(v)) } else { Err(Fallback::Float) }
}

fn vec(j: &Journal, at: u32) -> Result<u128, Fallback> {
    vector(j, at).ok_or(Fallback::Memory)
}

fn put(j: &mut Journal, at: u32, v: u128) -> Result<(), Fallback> {
    if at & 15 != 0 {
        return Err(Fallback::Memory);
    }
    put_vector(j, at, v).ok_or(Fallback::Memory)
}

fn put_word(j: &mut Journal, at: u32, v: u32) -> Result<(), Fallback> {
    j.put(at, v).ok_or(Fallback::Memory)
}

/// A 16-bit store: the word's other half kept.
fn put_half(j: &mut Journal, at: u32, v: u16) -> Result<(), Fallback> {
    let base = at & !3;
    if at & 1 != 0 {
        return Err(Fallback::Memory);
    }
    let shift = 8 * (at & 2);
    let old = word(j, base)?;
    put_word(j, base, (old & !(0xffff << shift)) | (u32::from(v) << shift))
}

fn half(j: &Journal, at: u32) -> Result<u16, Fallback> {
    if at & 1 != 0 {
        return Err(Fallback::Memory);
    }
    Ok((word(j, at & !3)? >> (8 * (at & 2))) as u16)
}

/// A byte store: the word's other bytes kept.
fn put_byte(j: &mut Journal, at: u32, v: u8) -> Result<(), Fallback> {
    let base = at & !3;
    let shift = 8 * (at & 3);
    let old = word(j, base)?;
    put_word(j, base, (old & !(0xff << shift)) | (u32::from(v) << shift))
}

fn byte(j: &Journal, at: u32) -> Result<u8, Fallback> {
    j.byte(at).ok_or(Fallback::Memory)
}

/// The root collidable's entity: `+0x0c` followed to null, then `+0x20`.
fn entity_of(j: &Journal, mut c: u32) -> Result<u32, Fallback> {
    // A chain longer than this is not a hierarchy the game builds.
    for _ in 0..64 {
        let parent = word(j, c.wrapping_add(COLLIDABLE_PARENT))?;
        if parent == 0 {
            return word(j, c.wrapping_add(COLLIDABLE_ENTITY));
        }
        c = parent;
    }
    Err(Fallback::Memory)
}

/// `0x003198b0`, thiscall on the motion, `(point, out)`: `w x (p - c) +
/// v` with the cross product's two halves as `mulps` gives them; the
/// partial result is stored to `out` before the sum.
fn point_velocity(j: &mut Journal, sse: &mut Sse, motion: u32, point: u32, out: u32) -> Result<(), Fallback> {
    let vt = word(j, motion)?;
    if word(j, vt.wrapping_add(0x58))? != POINT_VELOCITY {
        return Err(Fallback::Motion);
    }
    let centre = vec(j, motion.wrapping_add(0x70))?;
    let p = vec(j, point)?;
    let d = sse.sub(p, centre);
    let w = vec(j, motion.wrapping_add(0x50))?;
    let right = sse.mul(shuf(w, 0xd2), shuf(d, 0xc9));
    let left = sse.mul(shuf(w, 0xc9), shuf(d, 0xd2));
    let cross = sse.sub(left, right);
    put(j, out, cross)?;
    let v = vec(j, motion.wrapping_add(0x40))?;
    let sum = sse.add(cross, v);
    put(j, out, sum)
}

/// `0x00315f60`, thiscall on the island, `(constraint, sizes)`: the
/// largest of its `+0x08`, `s0` and `s1` (signed) to `+0x08`, `s1`, `s2`
/// and `s3` added to `+0x0c`, `+0x10` and `+0x18`.
fn island_sizes(j: &mut Journal, island: u32, s: [u32; 4]) -> Result<(), Fallback> {
    let vt = word(j, island)?;
    if word(j, vt.wrapping_add(0x08))? != ISLAND_SIZES {
        return Err(Fallback::Island);
    }
    let mut m = s[0];
    let held = word(j, island.wrapping_add(0x08))?;
    if held as i32 > m as i32 {
        m = held;
    }
    put_word(j, island.wrapping_add(0x08), m)?;
    if (m as i32) <= (s[1] as i32) {
        m = s[1];
    }
    let total = word(j, island.wrapping_add(0x0c))?;
    put_word(j, island.wrapping_add(0x08), m)?;
    put_word(j, island.wrapping_add(0x0c), total.wrapping_add(s[1]))?;
    let rows = word(j, island.wrapping_add(0x10))?;
    put_word(j, island.wrapping_add(0x10), rows.wrapping_add(s[2]))?;
    let more = word(j, island.wrapping_add(0x18))?;
    put_word(j, island.wrapping_add(0x18), more.wrapping_add(s[3]))
}

/// `0x0031be20`, thiscall on the ID map, `(index)`: the last free (`0xff`)
/// byte takes the index; none free, a byte appended. Returns the ID.
fn map_id(j: &mut Journal, map: u32, index: u8, esp: u32, calls: &mut dyn Calls) -> Result<u32, Fallback> {
    let count = word(j, map.wrapping_add(4))?;
    let data = word(j, map)?;
    let mut i = count.wrapping_sub(1) as i32;
    while i >= 0 {
        if byte(j, data.wrapping_add(i as u32))? == 0xff {
            put_byte(j, data.wrapping_add(i as u32), index)?;
            return Ok(i as u32);
        }
        i -= 1;
    }
    let capacity = word(j, map.wrapping_add(8))? & 0x7fff_ffff;
    if count == capacity {
        // `push esi` and two pushes and the return address.
        calls.expand(j, map, 1, esp.wrapping_sub(16))?;
    }
    let count = word(j, map.wrapping_add(4))?;
    let data = word(j, map)?;
    put_byte(j, data.wrapping_add(count), index)?;
    let count = word(j, map.wrapping_add(4))?.wrapping_add(1);
    put_word(j, map.wrapping_add(4), count)?;
    Ok(count.wrapping_sub(1))
}

/// `0x0031be80`, thiscall on the constraint, `(point out, property out)`,
/// `esp` at its entry: the island's sizes for one more point, a point
/// appended, a property inserted at the old point count (the ones past
/// it moved up), then the ID. Returns (ID, point, property).
fn add_point(j: &mut Journal, k: u32, esp: u32, calls: &mut dyn Calls) -> Result<(u32, u32, u32), Fallback> {
    use self::constraint::*;
    // `sub esp,0x24` and four pushes.
    let s = esp.wrapping_sub(0x34);
    let n = word(j, k.wrapping_add(PROPERTIES + 4))?;
    let (more, rows) = if n == 1 { (0x50, 8) } else { (0x30, 4) };
    let rest = if n == 1 { 2 } else { 1 };
    let sizes = [n.wrapping_mul(0x30).wrapping_add(0xb0), more, rows, rest];
    let island = word(j, k.wrapping_add(ISLAND))?;
    island_sizes(j, island, sizes)?;
    let flags = word(j, k.wrapping_add(FLAGS))?;
    put_word(j, k.wrapping_add(FLAGS), flags | 6)?;
    let p = word(j, k.wrapping_add(POINTS + 4))?;
    let capacity = word(j, k.wrapping_add(POINTS + 8))? & 0x7fff_ffff;
    let want = p.wrapping_add(1);
    if (capacity as i32) < (want as i32) {
        let grown = if (want as i32) < (capacity.wrapping_add(capacity) as i32) { capacity.wrapping_add(capacity) } else { want };
        calls.reserve(j, k.wrapping_add(POINTS), grown, POINT_BYTES, s.wrapping_sub(16))?;
    }
    put_word(j, k.wrapping_add(POINTS + 4), want)?;
    let points = word(j, k.wrapping_add(POINTS))?;
    let n = word(j, k.wrapping_add(PROPERTIES + 4))?;
    let point = (p << 5).wrapping_add(points);
    let above = n.wrapping_sub(p);
    let props = word(j, k.wrapping_add(PROPERTIES + 8))? & 0x7fff_ffff;
    let need = n.wrapping_add(1);
    if (props as i32) < (need as i32) {
        let grown = if (need as i32) < (props.wrapping_add(props) as i32) { props.wrapping_add(props) } else { need };
        calls.reserve(j, k.wrapping_add(PROPERTIES), grown, PROPERTY_BYTES, s.wrapping_sub(16))?;
    }
    let base = word(j, k.wrapping_add(PROPERTIES))?;
    let at = base.wrapping_add(p.wrapping_mul(PROPERTY_BYTES));
    if above.wrapping_sub(1) as i32 >= 0 {
        if above as i32 > MAX_PROPERTIES + 1 {
            return Err(Fallback::Memory);
        }
        // From the top down: each property past the old point count up one.
        let mut dst = at.wrapping_add(above.wrapping_mul(PROPERTY_BYTES));
        for _ in 0..above {
            let src = dst.wrapping_sub(PROPERTY_BYTES);
            for w in 0..7 {
                let v = word(j, src.wrapping_add(4 * w))?;
                put_word(j, dst.wrapping_add(4 * w), v)?;
            }
            dst = src;
        }
    }
    put_word(j, k.wrapping_add(PROPERTIES + 4), need)?;
    let property = word(j, k.wrapping_add(PROPERTIES))?.wrapping_add(p.wrapping_mul(PROPERTY_BYTES));
    put_word(j, property, 0)?;
    put_word(j, property.wrapping_add(4), 0)?;
    put_half(j, property.wrapping_add(0x18), 0)?;
    if p as i32 > 0 && half(j, property.wrapping_sub(4))? == 0 {
        put_half(j, property.wrapping_add(0x18), 1)?;
    }
    let point_out = word(j, esp.wrapping_add(4))?;
    put_word(j, point_out, point)?;
    let property_out = word(j, esp.wrapping_add(8))?;
    put_word(j, property_out, property)?;
    // One push and the return address.
    let id = map_id(j, k.wrapping_add(ID_MAP), p as u8, s.wrapping_sub(8), calls)?;
    Ok((id, point, property))
}

/// [`ADD`] to its listener walk: the frame, the constraint's new point
/// and property, the event. `Ok(Err(eax))`: past [`MAX_PROPERTIES`], the
/// call returns at once.
pub fn head(j: &mut Journal, a: &Args, fpu: Fpu, calls: &mut dyn Calls) -> Result<Result<Head, Committed>, Fallback> {
    if !fpu.masked() {
        return Err(Fallback::Float);
    }
    let x = Fx { mode: Mode::from_cw(fpu.control) };
    let mut sse = Sse { e: Exact::new(fpu.mxcsr) };
    let m = a.manager;
    // `push ebp; mov ebp,esp; and esp,-16; sub esp,0x74`, then three pushes.
    let ebp = a.esp.wrapping_sub(4);
    let f = (ebp & !15).wrapping_sub(0x80);
    put_word(j, ebp, a.ebp)?;
    put_word(j, f.wrapping_add(8), a.ebx)?;
    let n = word(j, m.wrapping_add(manager::PROPERTIES))?;
    put_word(j, f.wrapping_add(4), a.esi)?;
    put_word(j, f, a.edi)?;
    put_word(j, f.wrapping_add(0x28), m)?;
    if n as i32 > MAX_PROPERTIES {
        return Ok(Err(Committed::Full { eax: a.eax | 0xffff, ecx: m, edx: n }));
    }
    let entity_a = entity_of(j, a.a)?;
    put_word(j, f.wrapping_add(0x2c), entity_a)?;
    let entity_b = entity_of(j, a.b)?;
    put_word(j, f.wrapping_add(0x1c), entity_b)?;
    let k = m.wrapping_add(manager::CONSTRAINT);
    let world = word(j, m.wrapping_add(manager::WORLD))?;
    if n == 0 {
        // One push and the return address.
        calls.world_add(j, world, k, f.wrapping_sub(8))?;
    }
    // Two pushes and the return address.
    let at = f.wrapping_sub(12);
    put_word(j, at.wrapping_add(4), f.wrapping_add(0x24))?;
    put_word(j, at.wrapping_add(8), f.wrapping_add(0x18))?;
    let (id, point, property) = add_point(j, k, at, calls)?;
    let motion_a = word(j, entity_a.wrapping_add(entity::MOTION))?;
    point_velocity(j, &mut sse, motion_a, a.contact, f.wrapping_add(0x30))?;
    let motion_b = word(j, word(j, f.wrapping_add(0x1c))?.wrapping_add(entity::MOTION))?;
    point_velocity(j, &mut sse, motion_b, a.contact, f.wrapping_add(0x40))?;
    let va = vec(j, f.wrapping_add(0x30))?;
    let vb = vec(j, f.wrapping_add(0x40))?;
    let normal = vec(j, a.contact.wrapping_add(0x10))?;
    let rel = sse.sub(vb, va);
    let along = sse.mul(rel, normal);
    let speed = sse.hsum(along);
    put_word(j, f.wrapping_add(0x20), speed)?;
    if sse.e.trapped {
        return Err(Fallback::Float);
    }
    // 0x00318d1c: friction then restitution, `sqrt(B * A)` on the x87.
    let ea = entity_of(j, a.a)?;
    let eb = entity_of(j, a.b)?;
    let friction = x.mul(x87_word(j, eb.wrapping_add(entity::FRICTION))?, x87_word(j, ea.wrapping_add(entity::FRICTION))?);
    put_half(j, f.wrapping_add(event::AT), id as u16)?;
    put_word(j, f.wrapping_add(0x68), a.b)?;
    let friction = x87::sqrt(friction, x.mode).0;
    put_word(j, f.wrapping_add(0x7c), 0)?;
    let prop = word(j, f.wrapping_add(0x18))?;
    put_word(j, prop.wrapping_add(8), x.st(friction))?;
    let restitution = x.mul(x87_word(j, eb.wrapping_add(entity::RESTITUTION))?, x87_word(j, ea.wrapping_add(entity::RESTITUTION))?);
    put_word(j, f.wrapping_add(0x64), a.a)?;
    let restitution = x87::sqrt(restitution, x.mode).0;
    put_word(j, word(j, f.wrapping_add(0x18))?.wrapping_add(0x0c), x.st(restitution))?;
    // 0x00318d55: the contact copied to the point, its distance through
    // the x87.
    let pt = word(j, f.wrapping_add(0x24))?;
    let distance = x87_word(j, a.contact.wrapping_add(0x0c))?;
    let c0 = vec(j, a.contact)?;
    put(j, pt, c0)?;
    let c1 = vec(j, a.contact.wrapping_add(0x10))?;
    put(j, pt.wrapping_add(0x10), c1)?;
    put_word(j, pt.wrapping_add(0x0c), x.st(distance))?;
    let pt = word(j, f.wrapping_add(0x24))?;
    let prop = word(j, f.wrapping_add(0x18))?;
    put_word(j, f.wrapping_add(0x70), pt)?;
    let speed = word(j, f.wrapping_add(0x20))?;
    put_word(j, f.wrapping_add(0x74), prop)?;
    put_word(j, f.wrapping_add(0x78), speed)?;
    let world = word(j, m.wrapping_add(manager::WORLD))?;
    // The walk's two arguments, then (in the call) the return address.
    put_word(j, f.wrapping_sub(4), f.wrapping_add(event::AT))?;
    put_word(j, f.wrapping_sub(8), world)?;
    put_word(j, f.wrapping_add(0x60), m)?;
    put_word(j, f.wrapping_sub(12), WALK_SITE)?;
    Ok(Ok(Head { frame: f, id, world, entity_a, entity_b, point, property }))
}

/// `0x00318320` (`array` [`WORLD_LISTENERS`], owner `0`) or `0x00315d20`
/// (`array` [`entity::LISTENERS`], owner the entity), cdecl `(object,
/// event)`, `esp` at the entry: the owner to the event, each listener from
/// the last to the first (a null one skipped), then the null entries
/// removed from the last to the first.
fn walk(j: &mut Journal, object: u32, array: u32, owner: u32, ev: u32, esp: u32, calls: &mut dyn Calls) -> Result<(), Fallback> {
    put_word(j, ev.wrapping_add(event::OWNER), owner)?;
    // Three pushes, then the listener's argument and the return address.
    let call_esp = esp.wrapping_sub(20);
    let data = object.wrapping_add(array);
    let size = data.wrapping_add(4);
    let mut i = word(j, size)?.wrapping_sub(1) as i32;
    while i >= 0 {
        let listener = word(j, word(j, data)?.wrapping_add(4 * i as u32))?;
        if listener != 0 {
            calls.listener(j, listener, ev, call_esp)?;
        }
        i -= 1;
    }
    let mut i = word(j, size)?.wrapping_sub(1) as i32;
    while i >= 0 {
        if word(j, word(j, data)?.wrapping_add(4 * i as u32))? == 0 {
            let left = word(j, size)?.wrapping_sub(1);
            put_word(j, size, left)?;
            let mut at = i;
            if at < left as i32 {
                loop {
                    let base = word(j, data)?;
                    let next = word(j, base.wrapping_add(4 * at as u32 + 4))?;
                    put_word(j, base.wrapping_add(4 * at as u32), next)?;
                    at += 1;
                    if at >= word(j, size)? as i32 {
                        break;
                    }
                }
            }
        }
        i -= 1;
    }
    Ok(())
}

/// [`ADD`] whole: [`head`], the world's listener walk, each body's when
/// it has listeners, then a rejected contact's removal or the property's
/// approach speed, its target and the impulse seed.
pub fn add(j: &mut Journal, a: &Args, fpu: Fpu, calls: &mut dyn Calls) -> Result<Committed, Fallback> {
    let h = match head(j, a, fpu, calls)? {
        Ok(h) => h,
        Err(full) => return Ok(full),
    };
    tail(j, a, &h, fpu, calls)
}

/// [`ADD`] from its listener walk to the return, after [`head`].
pub fn tail(j: &mut Journal, a: &Args, h: &Head, fpu: Fpu, calls: &mut dyn Calls) -> Result<Committed, Fallback> {
    let x = Fx { mode: Mode::from_cw(fpu.control) };
    let f = h.frame;
    let ev = h.event();
    walk(j, h.world, WORLD_LISTENERS, 0, ev, h.walk_esp(), calls)?;
    let ea = word(j, f.wrapping_add(0x2c))?;
    if word(j, ea.wrapping_add(entity::LISTENERS + 4))? != 0 {
        walk(j, ea, entity::LISTENERS, ea, ev, h.walk_esp(), calls)?;
    }
    let eb = word(j, f.wrapping_add(0x1c))?;
    if word(j, eb.wrapping_add(entity::LISTENERS + 4))? != 0 {
        walk(j, eb, entity::LISTENERS, eb, ev, h.walk_esp(), calls)?;
    }
    if word(j, f.wrapping_add(0x7c))? == 1 {
        let m = word(j, f.wrapping_add(0x28))?;
        // One push and the return address.
        let eax = calls.remove(j, m, h.id, f.wrapping_sub(8))?;
        return Ok(Committed::Rejected { eax: eax | 0xffff });
    }
    // 0x00318df2: the approach speed and the target, on the x87.
    let prop = word(j, f.wrapping_add(0x18))?;
    let speed = x87_word(j, f.wrapping_add(0x20))?;
    let t = x.mul(x87_word(j, prop.wrapping_add(0x0c))?, x87_word(j, a.input.wrapping_add(INPUT_RESTITUTION))?);
    let t = x.mul(t, speed);
    let t = x.mul(t, x87_word(j, APPROACH)?);
    put_word(j, prop.wrapping_add(4), x.st(t))?;
    let prop = word(j, f.wrapping_add(0x18))?;
    let target = x.add(t, x87_word(j, a.contact.wrapping_add(0x0c))?);
    put_word(j, prop.wrapping_add(0x14), x.st(target))?;
    let motion_b = word(j, eb.wrapping_add(entity::MOTION))?;
    let motion_a = word(j, ea.wrapping_add(entity::MOTION))?;
    let mass = x.add(x87_word(j, motion_b.wrapping_add(0x2c))?, x87_word(j, motion_a.wrapping_add(0x2c))?);
    let prop = word(j, f.wrapping_add(0x18))?;
    let mass = x.add(mass, x87_word(j, EPSILON)?);
    let one = x87_word(j, ONE)?;
    let effective = x.div(one, mass);
    let bounce = x.add(x87_word(j, prop.wrapping_add(0x0c))?, one);
    let seed = x.mul(effective, bounce);
    let seed = x.mul(seed, speed);
    let seed = x.mul(seed, x87_word(j, SEED)?);
    put_word(j, prop, x.st(seed))?;
    Ok(Committed::Added { eax: (prop & 0xffff_0000) | (h.id & 0xffff), ecx: motion_b, edx: motion_a })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::Words;
    use std::collections::BTreeMap;

    struct Mem(BTreeMap<u32, u32>);

    impl Words for Mem {
        fn get(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied()
        }
    }

    #[test]
    fn declines_are_counted_by_reason_in_order() {
        let before = fallbacks();
        count(Fallback::First);
        let after = fallbacks();
        assert_eq!(after.map(|f| f.0), ["memory", "full", "first", "grow", "motion", "island", "float", "unobserved"]);
        assert!(after[2].1 > before[2].1);
    }

    #[test]
    fn half_and_byte_stores_keep_the_rest_of_the_word() {
        let mem = Mem(BTreeMap::from([(0x100, 0xaabb_ccdd)]));
        let mut j = Journal::new(&mem);
        put_half(&mut j, 0x102, 0x1234).unwrap();
        assert_eq!(j.get(0x100), Some(0x1234_ccdd));
        put_byte(&mut j, 0x101, 0x56).unwrap();
        assert_eq!(j.get(0x100), Some(0x1234_56dd));
        assert_eq!(half(&j, 0x100), Ok(0x56dd));
        assert_eq!(put_half(&mut j, 0x101, 0), Err(Fallback::Memory));
    }

    /// The world's walk removes the null entries a listener left, the
    /// later ones moving down, and calls the others last to first.
    #[test]
    fn the_walk_calls_last_to_first_and_drops_nulls() {
        struct Log(Vec<u32>);
        impl Calls for Log {
            fn world_add(&mut self, _: &mut Journal, _: u32, _: u32, _: u32) -> Result<(), Fallback> {
                unreachable!()
            }
            fn reserve(&mut self, _: &mut Journal, _: u32, _: u32, _: u32, _: u32) -> Result<(), Fallback> {
                unreachable!()
            }
            fn expand(&mut self, _: &mut Journal, _: u32, _: u32, _: u32) -> Result<(), Fallback> {
                unreachable!()
            }
            fn listener(&mut self, j: &mut Journal, listener: u32, _: u32, _: u32) -> Result<(), Fallback> {
                self.0.push(listener);
                // The second listener clears its own entry.
                if listener == 0x22 {
                    j.put(0x1004, 0).unwrap();
                }
                Ok(())
            }
            fn remove(&mut self, _: &mut Journal, _: u32, _: u32, _: u32) -> Result<u32, Fallback> {
                unreachable!()
            }
        }
        let world = 0x2000;
        let mem = Mem(BTreeMap::from([
            (world + WORLD_LISTENERS, 0x1000),
            (world + WORLD_LISTENERS + 4, 4),
            (0x1000, 0x11),
            (0x1004, 0x22),
            (0x1008, 0),
            (0x100c, 0x44),
            (0x3010, 0x99),
        ]));
        let mut j = Journal::new(&mem);
        let mut log = Log(Vec::new());
        walk(&mut j, world, WORLD_LISTENERS, 0, 0x3000, 0x8000, &mut log).unwrap();
        assert_eq!(log.0, [0x44, 0x22, 0x11]);
        assert_eq!(j.get(world + WORLD_LISTENERS + 4), Some(2));
        assert_eq!((j.get(0x1000), j.get(0x1004)), (Some(0x11), Some(0x44)));
        assert_eq!(j.get(0x3010), Some(0), "the owner");
    }
}
