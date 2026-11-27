//! The convex agent's manifold (docs/engine/havok-convex.md, "The
//! manifold"), each one guest function on a [`Journal`], every float
//! operation one guest instruction: packed and scalar SSE under the MXCSR,
//! x87 under the control word.
//!
//! - [`refresh`] `0x00302eb0`: each point of the manifold placed in the
//!   world by both bodies' transforms, kept as a contact in the
//!   collector's array or dropped through the manager.
//!
//! The add-point call `0x00302c70` stays the guest's. The calls
//! [`refresh`] makes out, the array's reserve and the manager's removal,
//! go through the caller's [`Calls`]. On a [`Fallback`] the journal is
//! discarded and nothing has been written.
use crate::convex::Fallback;
use crate::fp::Exact;
use crate::jacobian::{below, lane, put_vector, splat, vector, with_w, Fpu, Fx, Sse};
use crate::mem::Journal;
use crate::solver::{finite, load};
use x87::Mode;

/// `1.0` in every lane: the point's `1 - w`.
pub const ONES: u32 = 0x0041_2420;

/// A manifold slot, `0x30` bytes from the agent's `+0x30`.
pub mod slot {
    /// The point on A in A's frame, its weight in `w`.
    pub const POINT_A: u32 = 0x00;
    /// The point on B in B's frame; the manager's 16-bit contact ID at `+0x1c`.
    pub const POINT_B: u32 = 0x10;
    pub const ID: u32 = 0x1c;
    /// The contact normal in the world.
    pub const NORMAL: u32 = 0x20;
    pub const BYTES: u32 = 0x30;
}

/// A contact [`refresh`] writes to the collector's array, `0x30` bytes:
/// the world point with the distance in `w`, the normal, the ID (16
/// bits). The rest of the element is not written.
pub mod contact {
    pub const POINT: u32 = 0x00;
    pub const DISTANCE: u32 = 0x0c;
    pub const NORMAL: u32 = 0x10;
    pub const ID: u32 = 0x20;
}

/// The collision input's words: the distance a point is kept within, and
/// the tolerances block whose `+0x08` is the drift radius.
pub const INPUT_KEEP: u32 = 0x08;
pub const INPUT_TOLERANCES: u32 = 0x0c;
pub const TOLERANCE_DRIFT: u32 = 0x08;
/// The collidable's motion; its transform's columns at `+0x20`, `+0x30`,
/// `+0x40` and the translation at `+0x50`.
pub const COLLIDABLE_MOTION: u32 = 0x08;
pub const MOTION_TRANSFORM: u32 = 0x20;
/// The collector array: data, size, capacity (the top bit says the array
/// does not own its data).
pub const ARRAY_DATA: u32 = 0x00;
pub const ARRAY_SIZE: u32 = 0x04;
pub const ARRAY_CAPACITY: u32 = 0x08;

/// [`refresh`]'s arguments, cdecl, in the stack's order; `esp` at the
/// entry (at the return address).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Args {
    pub a: u32,
    pub b: u32,
    pub input: u32,
    /// The lowest distance a point is kept at (binary32 bits).
    pub floor: u32,
    pub slots: u32,
    /// The point count's address (the agent's `+0x18`).
    pub count: u32,
    /// The address of the collector array's pointer.
    pub collector: u32,
    pub manager: u32,
    pub esp: u32,
}

/// The calls [`refresh`] makes out. `esp` is the stack pointer at the
/// call's entry (at the return address). Neither leaves a register the
/// function reads.
pub trait Calls {
    /// `0x002d9160`, cdecl `(array, capacity, 0x30)`: the array grown to
    /// `capacity` elements of `0x30` bytes.
    fn reserve(&mut self, j: &mut Journal, array: u32, capacity: u32, esp: u32) -> Result<(), Fallback>;
    /// The manager's `vt[+0x08]`, thiscall `(id)`, `ret 4`: the point's
    /// contact removed.
    fn remove(&mut self, j: &mut Journal, manager: u32, id: u32, esp: u32) -> Result<(), Fallback>;
}

/// The registers at the return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
}

fn word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    j.get(at).ok_or(Fallback::Memory)
}

/// A float the x87 code loads: finite, or the call declines.
fn x87_word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    let v = word(j, at)?;
    if finite(v) { Ok(v) } else { Err(Fallback::Float) }
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

/// `((c0 x + c1 y) + (c2 z + t))` over the motion's transform, in
/// `0x00302f30`'s operation order.
fn place(j: &Journal, sse: &mut Sse, collidable: u32, p: u128) -> Result<u128, Fallback> {
    let m = word(j, collidable.wrapping_add(COLLIDABLE_MOTION))?.wrapping_add(MOTION_TRANSFORM);
    let t = vec(j, m.wrapping_add(0x30))?;
    let c2 = vec(j, m.wrapping_add(0x20))?;
    let z = sse.mul(c2, splat(lane(p, 2)));
    let zt = sse.add(z, t);
    let c1 = vec(j, m.wrapping_add(0x10))?;
    let y = sse.mul(c1, splat(lane(p, 1)));
    let c0 = vec(j, m)?;
    let x = sse.mul(c0, splat(lane(p, 0)));
    let xy = sse.add(x, y);
    Ok(sse.add(xy, zt))
}

/// `0x00302eb0`, cdecl, eight arguments ([`Args`]). The collector's
/// array is reserved for every point and its size raised by the count.
/// Each point is placed in the world through both transforms; its
/// distance `d` is `(A - B) . n`. A point is kept when `d` is below the
/// input's keep distance and above the floor, and B moved along the
/// normal by `d` lies within the drift radius of A (`|A - (B + d n)|^2`
/// below the radius squared); the contact written is `w A + (1 - w) B`
/// with `d` in `w`, the normal, the ID. Otherwise the manager removes its
/// contact, the last slot moves into its place, and the count and the
/// array's size drop by one.
pub fn refresh(j: &mut Journal, a: &Args, fpu: Fpu, calls: &mut dyn Calls) -> Result<Exit, Fallback> {
    if !fpu.masked() {
        return Err(Fallback::Float);
    }
    let x = Fx { mode: Mode::from_cw(fpu.control) };
    let mut sse = Sse { e: Exact::new(fpu.mxcsr) };
    // `push ebp; and esp,-16; sub esp,0x24`, then three pushes.
    let frame = (a.esp.wrapping_sub(4) & !15).wrapping_sub(0x24).wrapping_sub(12);
    let tolerances = word(j, a.input.wrapping_add(INPUT_TOLERANCES))?;
    let drift = x87_word(j, tolerances.wrapping_add(TOLERANCE_DRIFT))?;
    // 0x00302ec7: `fld st; fmul st,st(1); fstp`, the square rounded at the store.
    let drift2 = x.st(x.mul(load(drift), load(drift)));
    let array = word(j, a.collector)?;
    let size = word(j, array.wrapping_add(ARRAY_SIZE))? as i32;
    let capacity = (word(j, array.wrapping_add(ARRAY_CAPACITY))? & 0x7fff_ffff) as i32;
    let count = word(j, a.count)? as i32;
    let needed = count.wrapping_add(size);
    if capacity < needed {
        let doubled = capacity.wrapping_add(capacity);
        let grown = if needed < doubled { doubled } else { needed };
        // The array pointer comes back from the frame, not the collector.
        calls.reserve(j, array, grown as u32, frame.wrapping_sub(16))?;
    }
    let data = word(j, array.wrapping_add(ARRAY_DATA))?;
    put_word(j, array.wrapping_add(ARRAY_SIZE), needed as u32)?;
    let mut out = data.wrapping_add((size as u32).wrapping_mul(slot::BYTES));
    let count = word(j, a.count)? as i32;
    if count <= 0 {
        done(&sse)?;
        return Ok(Exit { eax: count as u32, ecx: array, edx: data });
    }
    let mut kept = 0i32;
    loop {
        let s = a.slots.wrapping_add((kept as u32).wrapping_mul(slot::BYTES));
        let pa = vec(j, s.wrapping_add(slot::POINT_A))?;
        let world_a = place(j, &mut sse, a.a, pa)?;
        let pb = vec(j, s.wrapping_add(slot::POINT_B))?;
        let world_b = place(j, &mut sse, a.b, pb)?;
        let n = vec(j, s.wrapping_add(slot::NORMAL))?;
        let gap = sse.sub(world_a, world_b);
        let along = sse.mul(gap, n);
        let d = sse.hsum(along);
        let moved = sse.mul(splat(d), n);
        let moved = sse.add(world_b, moved);
        let drift_v = sse.sub(world_a, moved);
        let drift_v = sse.mul(drift_v, drift_v);
        let drifted = sse.hsum(drift_v);
        done(&sse)?;
        let keep_within = x87_word(j, a.input.wrapping_add(INPUT_KEEP))?;
        if !finite(d) || !finite(drifted) || !finite(a.floor) {
            return Err(Fallback::Float);
        }
        // 0x00302ff1, 0x00303043, 0x00303051: `d < keep`, `d > floor`,
        // `drift^2 < radius^2`, each ordered.
        let keep = below(load(d), load(keep_within))
            && below(load(a.floor), load(d))
            && below(load(drifted), load(drift2));
        let exit = if keep {
            let weight = lane(pa, 3);
            let wa = sse.mul(splat(weight), world_a);
            let rest = sse.sub(vec(j, ONES)?, splat(weight));
            let wb = sse.mul(rest, world_b);
            let point = sse.add(wb, wa);
            done(&sse)?;
            if out & 15 != 0 {
                return Err(Fallback::Memory);
            }
            put(j, out.wrapping_add(contact::POINT), with_w(point, d))?;
            put(j, out.wrapping_add(contact::NORMAL), vec(j, s.wrapping_add(slot::NORMAL))?)?;
            let id = word(j, s.wrapping_add(slot::ID))? & 0xffff;
            let old = word(j, out.wrapping_add(contact::ID))?;
            put_word(j, out.wrapping_add(contact::ID), old & 0xffff_0000 | id)?;
            kept = kept.wrapping_add(1);
            out = out.wrapping_add(slot::BYTES);
            Exit { eax: weight, ecx: kept as u32, edx: kept as u32 }
        } else {
            let id = word(j, s.wrapping_add(slot::ID))? & 0xffff;
            // `push id; call [vt+8]`: one argument and the return address.
            calls.remove(j, a.manager, id, frame.wrapping_sub(8))?;
            let last = (word(j, a.count)? as i32).wrapping_sub(1);
            put_word(j, a.count, last as u32)?;
            let from = a.slots.wrapping_add((last as u32).wrapping_mul(slot::BYTES));
            for off in [0, 0x10, 0x20] {
                let v = vec(j, from.wrapping_add(off))?;
                put(j, s.wrapping_add(off), v)?;
            }
            let array = word(j, a.collector)?;
            let size = word(j, array.wrapping_add(ARRAY_SIZE))?;
            put_word(j, array.wrapping_add(ARRAY_SIZE), size.wrapping_sub(1))?;
            Exit { eax: array, ecx: kept as u32, edx: a.slots }
        };
        if kept >= word(j, a.count)? as i32 {
            return Ok(exit);
        }
    }
}

fn done(sse: &Sse) -> Result<(), Fallback> {
    if sse.e.trapped { Err(Fallback::Float) } else { Ok(()) }
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

    const A: u32 = 0x1000;
    const B: u32 = 0x1100;
    const MOTION_A: u32 = 0x2000;
    const MOTION_B: u32 = 0x2100;
    const INPUT: u32 = 0x3000;
    const TOLERANCES: u32 = 0x3100;
    const SLOTS: u32 = 0x4000;
    const COUNT: u32 = 0x4200;
    const COLLECTOR: u32 = 0x4300;
    const ARRAY: u32 = 0x4400;
    const DATA: u32 = 0x5000;
    const MANAGER: u32 = 0x6000;
    const FPU: Fpu = Fpu { mxcsr: 0x1f80, control: 0x027f };

    fn f(v: f32) -> u32 {
        v.to_bits()
    }

    fn put4(h: &mut Heap, at: u32, v: [f32; 4]) {
        for (i, x) in v.into_iter().enumerate() {
            h.0.insert(at + 4 * i as u32, f(x));
        }
    }

    /// A identity at the origin, B identity at +1 in y; keep within 0.5,
    /// floor -1, drift radius 0.25; an array of 4 holding one contact.
    fn heap(points: &[([f32; 4], [f32; 4], u32)]) -> Heap {
        let mut h = Heap::default();
        put4(&mut h, ONES, [1.0; 4]);
        h.0.insert(A + COLLIDABLE_MOTION, MOTION_A);
        h.0.insert(B + COLLIDABLE_MOTION, MOTION_B);
        for (m, t) in [(MOTION_A, [0.0, 0.0, 0.0, 0.0]), (MOTION_B, [0.0, 1.0, 0.0, 0.0])] {
            let base = m + MOTION_TRANSFORM;
            put4(&mut h, base, [1.0, 0.0, 0.0, 0.0]);
            put4(&mut h, base + 0x10, [0.0, 1.0, 0.0, 0.0]);
            put4(&mut h, base + 0x20, [0.0, 0.0, 1.0, 0.0]);
            put4(&mut h, base + 0x30, t);
        }
        h.0.insert(INPUT + INPUT_KEEP, f(0.5));
        h.0.insert(INPUT + INPUT_TOLERANCES, TOLERANCES);
        h.0.insert(TOLERANCES + TOLERANCE_DRIFT, f(0.25));
        h.0.insert(COLLECTOR, ARRAY);
        h.0.insert(ARRAY + ARRAY_DATA, DATA);
        h.0.insert(ARRAY + ARRAY_SIZE, 1);
        h.0.insert(ARRAY + ARRAY_CAPACITY, 4);
        h.0.insert(COUNT, points.len() as u32);
        for (i, (pa, pb, id)) in points.iter().enumerate() {
            let s = SLOTS + slot::BYTES * i as u32;
            put4(&mut h, s, *pa);
            put4(&mut h, s + slot::POINT_B, *pb);
            h.0.insert(s + slot::ID, 0x3f00_0000 | id);
            put4(&mut h, s + slot::NORMAL, [0.0, 1.0, 0.0, 0.0]);
        }
        h.0.insert(DATA + slot::BYTES + contact::ID, 0xabcd_0000);
        h
    }

    fn args() -> Args {
        Args { a: A, b: B, input: INPUT, floor: f(-1.0), slots: SLOTS, count: COUNT, collector: COLLECTOR, manager: MANAGER, esp: 0x7_0000 }
    }

    #[derive(Default)]
    struct Log {
        removed: Vec<u32>,
        reserved: Vec<u32>,
    }

    impl Calls for Log {
        fn reserve(&mut self, j: &mut Journal, array: u32, capacity: u32, _: u32) -> Result<(), Fallback> {
            self.reserved.push(capacity);
            j.put(array + ARRAY_DATA, 0x8000).ok_or(Fallback::Memory)?;
            j.put(array + ARRAY_CAPACITY, capacity).ok_or(Fallback::Memory)
        }
        fn remove(&mut self, _: &mut Journal, manager: u32, id: u32, _: u32) -> Result<(), Fallback> {
            assert_eq!(manager, MANAGER);
            self.removed.push(id);
            Ok(())
        }
    }

    #[test]
    fn a_point_that_holds_is_written_as_a_contact() {
        // A's point at y 1.25 (weight 0.5), B's at y 0 (world y 1): d = 0.2.
        let h = heap(&[([0.0, 1.25, 0.0, 0.5], [0.0, 0.0, 0.0, 0.0], 7)]);
        let mut j = Journal::new(&h);
        let mut log = Log::default();
        let exit = refresh(&mut j, &args(), FPU, &mut log).unwrap();
        assert_eq!(exit, Exit { eax: f(0.5), ecx: 1, edx: 1 });
        assert!(log.removed.is_empty() && log.reserved.is_empty());
        let w = j.finish();
        let out = DATA + slot::BYTES;
        assert_eq!(w[&(ARRAY + ARRAY_SIZE)], 2);
        // 0.5 (0, 1.25) + 0.5 (0, 1) = (0, 1.125), d = 0.25 in w.
        assert_eq!([0, 4, 8, 12].map(|o| w[&(out + o)]), [0, f(1.125), 0, f(0.25)]);
        assert_eq!(w[&(out + contact::NORMAL + 4)], f(1.0));
        assert_eq!(w[&(out + contact::ID)], 0xabcd_0007);
    }

    #[test]
    fn a_point_past_any_limit_is_removed_and_the_last_takes_its_slot() {
        // Slot 0 too far apart (d = 0.75), slot 1 drifted sideways by 0.5,
        // slot 2 holds.
        let h = heap(&[
            ([0.0, 1.75, 0.0, 1.0], [0.0; 4], 1),
            ([0.5, 1.0, 0.0, 1.0], [0.0; 4], 2),
            ([0.0, 1.0, 0.0, 1.0], [0.0; 4], 3),
        ]);
        let mut j = Journal::new(&h);
        let mut log = Log::default();
        let exit = refresh(&mut j, &args(), FPU, &mut log).unwrap();
        // Slot 0 drops; slot 2 moves in and holds; slot 1 drops.
        assert_eq!(log.removed, [1, 2]);
        assert_eq!(exit, Exit { eax: ARRAY, ecx: 1, edx: SLOTS });
        let w = j.finish();
        assert_eq!(w[&COUNT], 1);
        assert_eq!(w[&(ARRAY + ARRAY_SIZE)], 2);
        assert_eq!(w[&(SLOTS + slot::ID)], 0x3f00_0003);
        assert_eq!(w[&(DATA + slot::BYTES + contact::ID)], 0xabcd_0003);
    }

    #[test]
    fn a_short_array_is_reserved_first_and_floats_decline() {
        let mut h = heap(&[([0.0, 1.0, 0.0, 1.0], [0.0; 4], 1), ([0.0, 1.0, 0.0, 1.0], [0.0; 4], 2)]);
        h.0.insert(ARRAY + ARRAY_CAPACITY, 0x8000_0002);
        let mut j = Journal::new(&h);
        let mut log = Log::default();
        refresh(&mut j, &args(), FPU, &mut log).unwrap();
        // Needed 3 against 2: the larger of 4 and 3.
        assert_eq!(log.reserved, [4]);
        assert!(j.finish().contains_key(&(0x8000 + slot::BYTES)));
        let h = heap(&[([0.0, 1.0, 0.0, 1.0], [0.0; 4], 1)]);
        let unmasked = Fpu { mxcsr: 0x1f00, ..FPU };
        assert_eq!(refresh(&mut Journal::new(&h), &args(), unmasked, &mut Log::default()), Err(Fallback::Float));
        let mut h = heap(&[([0.0, 1.0, 0.0, 1.0], [0.0; 4], 1)]);
        h.0.insert(TOLERANCES + TOLERANCE_DRIFT, f(f32::INFINITY));
        assert_eq!(refresh(&mut Journal::new(&h), &args(), FPU, &mut Log::default()), Err(Fallback::Float));
    }

    #[test]
    fn an_empty_manifold_only_reserves() {
        let h = heap(&[]);
        let mut j = Journal::new(&h);
        let exit = refresh(&mut j, &args(), FPU, &mut Log::default()).unwrap();
        assert_eq!(exit, Exit { eax: 0, ecx: ARRAY, edx: DATA });
        assert_eq!(j.finish().into_iter().collect::<Vec<_>>(), vec![(ARRAY + ARRAY_SIZE, 1)]);
    }
}
