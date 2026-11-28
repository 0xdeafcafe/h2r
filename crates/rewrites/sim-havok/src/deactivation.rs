//! The deactivation test `0x00316b30` (docs/engine/havok-integrate.md):
//! thiscall on the island, two stack arguments (the out byte, the step),
//! `ret 8`. It returns the out pointer in eax, and `0x0030fef0` reads the
//! byte: 1 deactivates the island.
//!
//! [`test`] runs the entry on a [`Journal`]: both timers, the two period
//! compares, and on the period that fired each entity's deactivator
//! through `entity+0x54`, slot `+0x14` for the first period and `+0x18`
//! for the second. The deactivators are the functions the image installs
//! ([`Deactivator`]); any other slot target declines. Every float
//! operation is one guest instruction: x87 under the control word, packed
//! and scalar SSE under the MXCSR. A non-finite float the x87 code would
//! load declines ([`Fallback::Float`]). On a [`Fallback`] the journal is
//! discarded and nothing has been written.
use crate::fp::Exact;
use crate::jacobian::{above, below, lane, put_vector, shuf, vector, Fpu, Fx, Sse};
use crate::mem::Journal;
use crate::solver::{finite, load};
use std::sync::atomic::{AtomicU64, Ordering};
use x87::Mode;

/// The island's words the entry reads.
pub const ISLAND_WORLD: u32 = 0x20;
/// A signed byte: how many first-period checks in a row found every
/// entity at rest, up to [`COUNTER_LIMIT`].
pub const ISLAND_COUNTER: u32 = 0x2c;
pub const ISLAND_TIMER_A: u32 = 0x34;
pub const ISLAND_TIMER_B: u32 = 0x38;
pub const ISLAND_ENTITIES: u32 = 0x3c;
pub const ISLAND_ENTITY_COUNT: u32 = 0x40;
/// The world's two check periods.
pub const WORLD_PERIOD_A: u32 = 0x30;
pub const WORLD_PERIOD_B: u32 = 0x34;
pub const ENTITY_MOTION: u32 = 0x3c;
pub const ENTITY_DEACTIVATOR: u32 = 0x54;
/// The deactivator's slots for the two periods.
pub const SLOT_A: u32 = 0x14;
pub const SLOT_B: u32 = 0x18;
/// The counter's value at which a first-period pass deactivates.
pub const COUNTER_LIMIT: i8 = 5;
/// The motion's orientation and centre the deactivators compare with.
pub const MOTION_ORIENTATION: u32 = 0x30;
pub const MOTION_CENTER: u32 = 0xb0;

/// More entities than this decline before the loop.
pub const MAX_ENTITIES: u32 = 1 << 16;

/// Halo's object tables (`0x001d1910`): the header array, the object
/// definitions, the tag instances.
pub const OBJECT_HEADERS: u32 = 0x004e_0300;
pub const OBJECT_DEFINITIONS: u32 = 0x0051_e9b8;
pub const TAG_INSTANCES: u32 = 0x004e_3b44;
/// The Halo helper `0x000e5240` tail-calls when its object's flag is 1.
pub const UNPORTED_HELPER: u32 = 0x000e_4050;

/// Which of the two references a deactivator compares with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    A,
    B,
}

impl Period {
    /// Reference centre, reference orientation, radius, angle.
    fn fields(self) -> [u32; 4] {
        match self {
            Self::A => [0x10, 0x20, 0x50, 0x54],
            Self::B => [0x30, 0x40, 0x58, 0x5c],
        }
    }
}

/// The scale on the orientation distance, both periods.
pub const DEACTIVATOR_SCALE: u32 = 0x08;

/// The deactivator functions the image installs, by slot target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deactivator {
    /// `0x003192a0`, both slots of the always-active vtable `0x00415100`:
    /// the entity is never at rest.
    Never,
    /// `0x003144b0` and `0x00314580`, the entity deactivator `0x00414abc`.
    Havok(Period),
    /// `0x001d1870` and `0x001d18c0`, Halo's subclass `0x00455950`: Halo's
    /// object check `0x001d1910`, then the Havok test.
    Halo(Period),
}

impl Deactivator {
    pub const NEVER: u32 = 0x0031_92a0;
    pub const HAVOK_A: u32 = 0x0031_44b0;
    pub const HAVOK_B: u32 = 0x0031_4580;
    pub const HALO_A: u32 = 0x001d_1870;
    pub const HALO_B: u32 = 0x001d_18c0;

    pub fn of(function: u32) -> Option<Self> {
        Some(match function {
            Self::NEVER => Self::Never,
            Self::HAVOK_A => Self::Havok(Period::A),
            Self::HAVOK_B => Self::Havok(Period::B),
            Self::HALO_A => Self::Halo(Period::A),
            Self::HALO_B => Self::Halo(Period::B),
            _ => return None,
        })
    }
}

/// Why a deactivation test runs the guest. Each is counted ([`fallbacks`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// A word the model needs is unmapped or unaligned, or a store the
    /// answer needs is not plain writable memory.
    Memory,
    /// A deactivator slot whose target is not a [`Deactivator`].
    Deactivator,
    /// Halo's object check reaches `0x000e4050` ([`UNPORTED_HELPER`]).
    Object,
    /// An unmasked float exception, an x87 stack not empty at the entry,
    /// or a non-finite float the x87 code would load.
    Float,
    /// The monitor stream is on.
    Monitor,
    /// The guest answered an audited call whose return could not be
    /// observed, so it went uncompared.
    Unobserved,
}

impl Fallback {
    pub const ALL: [Fallback; 6] = [Self::Memory, Self::Deactivator, Self::Object, Self::Float, Self::Monitor, Self::Unobserved];

    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Deactivator => "deactivator",
            Self::Object => "object",
            Self::Float => "float",
            Self::Monitor => "monitor",
            Self::Unobserved => "unobserved",
        }
    }
}

static FALLBACKS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

/// Count a call sent to the guest.
pub fn count(reason: Fallback) {
    FALLBACKS[reason as usize].fetch_add(1, Ordering::Relaxed);
}

/// The test's declines since start, by reason, in [`Fallback::ALL`] order.
pub fn fallbacks() -> [(&'static str, u64); 6] {
    Fallback::ALL.map(|f| (f.name(), FALLBACKS[f as usize].load(Ordering::Relaxed)))
}

struct Unit {
    mxcsr: u32,
    x: Fx,
}

impl Unit {
    fn new(fpu: Fpu) -> Self {
        Self { mxcsr: fpu.mxcsr, x: Fx { mode: Mode::from_cw(fpu.control) } }
    }
}

fn word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    j.get(at).ok_or(Fallback::Memory)
}

fn byte(j: &Journal, at: u32) -> Result<u8, Fallback> {
    j.byte(at).ok_or(Fallback::Memory)
}

/// A float the x87 code loads: finite, or the call declines.
fn x87_word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    let v = word(j, at)?;
    if finite(v) { Ok(v) } else { Err(Fallback::Float) }
}

fn finite_or_decline(v: u32) -> Result<u32, Fallback> {
    if finite(v) { Ok(v) } else { Err(Fallback::Float) }
}

/// Store one byte: the word around it, with that byte replaced.
pub fn put_byte(j: &mut Journal, at: u32, value: u8) -> Option<()> {
    let aligned = at & !3;
    let shift = 8 * (at & 3);
    let old = j.get(aligned)?;
    j.put(aligned, (old & !(0xff << shift)) | u32::from(value) << shift)
}

/// The whole entry for `island` and the step `{dt, 1/dt}` at `step`. The
/// out byte is stored at `out` when given; the integrate phase's model
/// passes none, since the byte lands in its own dead frame. Returns the
/// out byte: whether the island deactivates.
pub fn test(j: &mut Journal, island: u32, out: Option<u32>, step: u32, fpu: Fpu) -> Result<bool, Fallback> {
    if !fpu.masked() {
        return Err(Fallback::Float);
    }
    let u = Unit::new(fpu);
    let at = |off: u32| island.wrapping_add(off);
    // 0x00316b30: both timers advance by dt.
    let dt = x87_word(j, step)?;
    let timer_a = x87_word(j, at(ISLAND_TIMER_A))?;
    let world = word(j, at(ISLAND_WORLD))?;
    let a = u.x.st(u.x.add(load(dt), load(timer_a)));
    j.put(at(ISLAND_TIMER_A), a).ok_or(Fallback::Memory)?;
    let timer_b = x87_word(j, at(ISLAND_TIMER_B))?;
    let b = u.x.st(u.x.add(load(timer_b), load(dt)));
    j.put(at(ISLAND_TIMER_B), b).ok_or(Fallback::Memory)?;
    // 0x00316b51: each period fires when its timer is above it, ordered.
    let a = x87_word(j, at(ISLAND_TIMER_A))?;
    let period_a = x87_word(j, world.wrapping_add(WORLD_PERIOD_A))?;
    let fires_a = above(load(a), load(period_a));
    let b = finite_or_decline(b)?;
    let period_b = x87_word(j, world.wrapping_add(WORLD_PERIOD_B))?;
    let fires_b = above(load(b), load(period_b));
    let deactivate = if fires_a {
        let a = x87_word(j, at(ISLAND_TIMER_A))?;
        let period = x87_word(j, world.wrapping_add(WORLD_PERIOD_A))?;
        j.put(at(ISLAND_TIMER_A), u.x.st(u.x.sub(load(a), load(period)))).ok_or(Fallback::Memory)?;
        if at_rest(j, &u, island, SLOT_A)? {
            // 0x00316bc6: deactivate on the sixth pass in a row.
            let counter = byte(j, at(ISLAND_COUNTER))? as i8;
            let (next, out) = if counter < COUNTER_LIMIT { (counter.wrapping_add(1), false) } else { (0, true) };
            put_byte(j, at(ISLAND_COUNTER), next as u8).ok_or(Fallback::Memory)?;
            out
        } else {
            false
        }
    } else if fires_b {
        let period = x87_word(j, world.wrapping_add(WORLD_PERIOD_B))?;
        j.put(at(ISLAND_TIMER_B), u.x.st(u.x.sub(load(b), load(period)))).ok_or(Fallback::Memory)?;
        at_rest(j, &u, island, SLOT_B)?
    } else {
        false
    };
    if let Some(out) = out {
        put_byte(j, out, u8::from(deactivate)).ok_or(Fallback::Memory)?;
    }
    Ok(deactivate)
}

/// `0x00316b88`: every entity's deactivator through `slot`, in array
/// order, none skipped. True when the island has no entity.
fn at_rest(j: &mut Journal, u: &Unit, island: u32, slot: u32) -> Result<bool, Fallback> {
    let count = word(j, island.wrapping_add(ISLAND_ENTITY_COUNT))? as i32;
    if count <= 0 {
        return Ok(true);
    }
    let mut all = true;
    let mut i: u32 = 0;
    loop {
        if i >= MAX_ENTITIES {
            return Err(Fallback::Memory);
        }
        let entities = word(j, island.wrapping_add(ISLAND_ENTITIES))?;
        let entity = word(j, entities.wrapping_add(4 * i))?;
        let deactivator = word(j, entity.wrapping_add(ENTITY_DEACTIVATOR))?;
        if deactivator == 0 {
            all = false;
        } else {
            let vt = word(j, deactivator)?;
            let function = word(j, vt.wrapping_add(slot))?;
            let class = Deactivator::of(function).ok_or(Fallback::Deactivator)?;
            if !run(j, u, class, deactivator, entity)? {
                all = false;
            }
        }
        i += 1;
        // The count is read again after every call.
        if (i as i32) >= word(j, island.wrapping_add(ISLAND_ENTITY_COUNT))? as i32 {
            return Ok(all);
        }
    }
}

/// One deactivator call: its out byte.
fn run(j: &mut Journal, u: &Unit, class: Deactivator, this: u32, entity: u32) -> Result<bool, Fallback> {
    match class {
        Deactivator::Never => Ok(false),
        Deactivator::Havok(period) => havok(j, u, period, this, entity),
        Deactivator::Halo(period) => {
            if halo_awake(j, this)? {
                Ok(false)
            } else {
                havok(j, u, period, this, entity)
            }
        }
    }
}

/// `0x003144b0` / `0x00314580`: at rest when the centre moved less than
/// the radius and the orientation less than the angle since the reference;
/// otherwise the reference moves to the motion's state.
fn havok(j: &mut Journal, u: &Unit, period: Period, this: u32, entity: u32) -> Result<bool, Fallback> {
    let [center, orientation, radius, angle] = period.fields();
    let mut sse = Sse { e: Exact::new(u.mxcsr) };
    let field = |off: u32| this.wrapping_add(off);
    let motion = word(j, entity.wrapping_add(ENTITY_MOTION))?;
    let r = x87_word(j, field(radius))?;
    let r2 = u.x.mul(load(r), load(r));
    let vec = |j: &Journal, at: u32| vector(j, at).ok_or(Fallback::Memory);
    let moved = sse.sub(vec(j, field(center))?, vec(j, motion.wrapping_add(MOTION_CENTER))?);
    let moved = sse.mul(moved, moved);
    let distance = finite_or_decline(sse.hsum(moved))?;
    let q = vec(j, motion.wrapping_add(MOTION_ORIENTATION))?;
    let turned = sse.sub(vec(j, field(orientation))?, q);
    let turned = sse.mul(turned, turned);
    let pairs = sse.add(shuf(turned, 0x4e), turned);
    let turn = lane(sse.add(pairs, shuf(pairs, 0xb1)), 0);
    if sse.e.trapped {
        return Err(Fallback::Float);
    }
    // 0x00314528: `test ah,5; jp` continues only when strictly below.
    if below(load(distance), r2) {
        let a = x87_word(j, field(angle))?;
        let turn = finite_or_decline(turn)?;
        let k = x87_word(j, field(DEACTIVATOR_SCALE))?;
        let scaled = u.x.mul(load(turn), load(k));
        let a2 = u.x.mul(load(a), load(a));
        if above(a2, scaled) {
            return Ok(true);
        }
    }
    // 0x00314555: the orientation first, then the centre, read again.
    put_vector(j, field(orientation), q).ok_or(Fallback::Memory)?;
    let motion = word(j, entity.wrapping_add(ENTITY_MOTION))?;
    let c = vec(j, motion.wrapping_add(MOTION_CENTER))?;
    put_vector(j, field(center), c).ok_or(Fallback::Memory)?;
    Ok(false)
}

/// `0x001d1910`: Halo's object keeps the entity awake. Reads only.
fn halo_awake(j: &Journal, this: u32) -> Result<bool, Fallback> {
    let headers = word(j, word(j, OBJECT_HEADERS)?.wrapping_add(0x44))?;
    let handle = word(j, this.wrapping_add(0x60))?;
    let header = headers.wrapping_add((handle & 0xffff).wrapping_mul(12));
    let object = word(j, header.wrapping_add(8))?;
    let definition = word(j, object.wrapping_add(0xb4))? & 0xffff;
    let definitions = word(j, word(j, OBJECT_DEFINITIONS)?.wrapping_add(0x44))?;
    let nodes = word(j, definitions.wrapping_add(definition.wrapping_mul(0xa0)).wrapping_add(0x70))?;
    let node = word(j, this.wrapping_add(0x64))?;
    let pinned = byte(j, nodes.wrapping_add(node.wrapping_mul(0x60)).wrapping_add(0x44))? == 1;
    match byte(j, header.wrapping_add(3))? {
        0 => {
            if byte(j, object.wrapping_add(0x34b))? == 1 {
                Ok(false)
            } else if pinned {
                Ok(true)
            } else {
                // 0x000e5240: flag 1 tail-calls 0x000e4050.
                if byte(j, object.wrapping_add(0x3dc))? == 1 { Err(Fallback::Object) } else { Ok(false) }
            }
        }
        1 => Ok(pinned || moving(j, object)?),
        _ => Ok(pinned),
    }
}

/// `0x000efb60`: any of the object's tag-flagged speeds is not zero
/// (`ucomiss` against zero, unordered counting as not zero).
fn moving(j: &Journal, object: u32) -> Result<bool, Fallback> {
    let tag = word(j, object)? & 0xffff;
    let data = word(j, word(j, TAG_INSTANCES)?.wrapping_add(tag << 4).wrapping_add(8))?;
    let nonzero = |at: u32| -> Result<bool, Fallback> {
        // The Pentium III has no DAZ: a denormal is not zero.
        let bits = word(j, object.wrapping_add(at))?;
        Ok(f32::from_bits(bits) != 0.0)
    };
    if byte(j, data.wrapping_add(0x1ec))? & 1 != 0 && (nonzero(0x354)? || byte(j, object.wrapping_add(0x38b))? != 0xff) {
        return Ok(true);
    }
    let flags = word(j, data.wrapping_add(0x1ec))?;
    for (bit, at) in [(1, 0x35c), (2, 0x25c), (3, 0x260), (5, 0x358)] {
        if flags >> bit & 1 != 0 && nonzero(at)? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::Words;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Heap(HashMap<u32, u32>);

    impl Words for Heap {
        fn get(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied().or(Some(0))
        }
    }

    const ISLAND: u32 = 0x2000;
    const WORLD: u32 = 0x9000;
    const STEP: u32 = 0x8000;
    const OUT: u32 = 0x7ff0;
    const FPU: Fpu = Fpu { mxcsr: 0x1f80, control: 0x027f };

    fn f(v: f32) -> u32 {
        v.to_bits()
    }

    /// An island of `n` entities with always-active deactivators.
    fn heap(n: u32, a: f32, b: f32) -> Heap {
        let mut h = Heap::default();
        let vt = 0x5000;
        h.0.insert(vt + SLOT_A, Deactivator::NEVER);
        h.0.insert(vt + SLOT_B, Deactivator::NEVER);
        for (at, v) in [
            (STEP, f(0.25)),
            (ISLAND + ISLAND_WORLD, WORLD),
            (ISLAND + ISLAND_TIMER_A, f(a)),
            (ISLAND + ISLAND_TIMER_B, f(b)),
            (ISLAND + ISLAND_ENTITIES, 0x3000),
            (ISLAND + ISLAND_ENTITY_COUNT, n),
            (WORLD + WORLD_PERIOD_A, f(1.0)),
            (WORLD + WORLD_PERIOD_B, f(4.0)),
        ] {
            h.0.insert(at, v);
        }
        for i in 0..n {
            let (entity, d) = (0x6000 + 0x100 * i, 0x7000 + 0x100 * i);
            h.0.insert(0x3000 + 4 * i, entity);
            h.0.insert(entity + ENTITY_DEACTIVATOR, d);
            h.0.insert(d, vt);
        }
        h
    }

    #[test]
    fn the_timers_advance_and_the_first_period_counts_passes() {
        // No entity: every pass is at rest, so the counter climbs to five,
        // then the sixth pass deactivates and clears it.
        let h = heap(0, 0.9, 0.0);
        for (counter, deactivates, next) in [(0u8, false, 1u8), (4, false, 5), (5, true, 0)] {
            let mut j = Journal::new(&h);
            put_byte(&mut j, ISLAND + ISLAND_COUNTER, counter).unwrap();
            assert_eq!(test(&mut j, ISLAND, Some(OUT), STEP, FPU), Ok(deactivates));
            assert_eq!(j.byte(ISLAND + ISLAND_COUNTER), Some(next));
            assert_eq!(j.byte(OUT), Some(u8::from(deactivates)));
            assert_eq!(j.get(ISLAND + ISLAND_TIMER_A), Some(f(0.9f32 + 0.25 - 1.0)));
            assert_eq!(j.get(ISLAND + ISLAND_TIMER_B), Some(f(0.25)));
        }
    }

    #[test]
    fn an_awake_entity_or_no_deactivator_keeps_the_island() {
        let mut h = heap(2, 0.9, 0.0);
        let mut j = Journal::new(&h);
        assert_eq!(test(&mut j, ISLAND, Some(OUT), STEP, FPU), Ok(false));
        assert_eq!(j.byte(ISLAND + ISLAND_COUNTER), Some(0), "the counter is left");
        h.0.insert(0x6000 + ENTITY_DEACTIVATOR, 0);
        assert_eq!(test(&mut Journal::new(&h), ISLAND, None, STEP, FPU), Ok(false));
    }

    #[test]
    fn the_second_period_deactivates_at_once() {
        let h = heap(0, 0.0, 3.9);
        let mut j = Journal::new(&h);
        assert_eq!(test(&mut j, ISLAND, Some(OUT), STEP, FPU), Ok(true));
        assert_eq!(j.get(ISLAND + ISLAND_TIMER_B), Some(f(3.9f32 + 0.25 - 4.0)));
        assert_eq!(j.byte(ISLAND + ISLAND_COUNTER), Some(0));
    }

    #[test]
    fn unknown_deactivators_non_finite_timers_and_unmasked_floats_decline() {
        let mut h = heap(1, 0.9, 0.0);
        h.0.insert(0x5000 + SLOT_A, 0x0012_3456);
        assert_eq!(test(&mut Journal::new(&h), ISLAND, None, STEP, FPU), Err(Fallback::Deactivator));
        let mut h = heap(1, 0.9, 0.0);
        h.0.insert(ISLAND + ISLAND_TIMER_A, f(f32::INFINITY));
        assert_eq!(test(&mut Journal::new(&h), ISLAND, None, STEP, FPU), Err(Fallback::Float));
        let h = heap(1, 0.9, 0.0);
        let unmasked = Fpu { mxcsr: 0x1f00, control: 0x027f };
        assert_eq!(test(&mut Journal::new(&h), ISLAND, None, STEP, unmasked), Err(Fallback::Float));
    }

    #[test]
    fn declines_are_counted_by_reason_in_order() {
        let names = fallbacks().map(|(n, _)| n);
        assert_eq!(names, ["memory", "deactivator", "object", "float", "monitor", "unobserved"]);
        let before = fallbacks()[2].1;
        count(Fallback::Object);
        assert!(fallbacks()[2].1 > before);
    }
}
