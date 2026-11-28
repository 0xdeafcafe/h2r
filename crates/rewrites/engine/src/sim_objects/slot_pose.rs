//! The pose slot callees (docs/engine/object-update.md, "Slot pose
//! callees"): the `vehi` and `devi` descriptors' slot `+6c`, which the
//! pose dispatcher `109050` calls `(object, s0, s1, s2)`; both `ret 10`.
//!
//! **`f0f50` (vehicle).** Packs `s1`, `s2`, `s0` into a 12-byte frame
//! and calls the vehicle pose `f0760(object, 0xf06e0, &frame)` (`ret c`;
//! `0xf06e0` is the per-channel callback it passes, O). `f0760` stays a
//! guest call: about 2 KB of animation channel setup.
//!
//! **`107000` (device).** Reads the object's definition, its model
//! (definition `+38`) and the model's animation graph (model `+4`); with
//! either none, or no animation block (body `+12a` none), it returns.
//! Otherwise it stores the graph's definition over its own `object`
//! argument and, for each of the two channels at body `+18c` and `+1ac`
//! whose handle and `+6` word are not none, calls the channel evaluate
//! `1c6f60(1.0, s0, s1, s2)` (`ebx` the channel, `ecx` the graph
//! definition or what the first call left; `ret 10`).

use super::alu::{cmp16, shl};
use super::{Cx, Next, Result, unowned};
use crate::datum::layout::DATA;
use crate::points;
use crate::sim_physics::{arithmetic, logic, pop, push};
use runtime::{CpuAbi, Guest};

const NONE: u32 = u32::MAX;
/// The vehicle pose `f0f50` hands its frame to; `ret c`.
const VEHICLE_POSE_CHANNELS: u32 = 0x000f_0760;
/// The per-channel callback `f0f50` passes it.
const VEHICLE_POSE_CALLBACK: u32 = 0x000f_06e0;
/// The channel evaluate `(channel: ebx, weight, s0, s1, s2: stack)`;
/// `ret 10`.
const CHANNEL_EVALUATE: u32 = 0x001c_6f60;
/// binary32 1.0, the weight `107000` pushes.
const ONE: u32 = 0x3f80_0000;

/// After `f0760`.
pub(super) const VEHICLE_SITES: [u32; 1] = [0xf0f7d];
/// After the first and the second channel evaluate.
pub(super) const DEVICE_SITES: [u32; 2] = [0x10709a, 0x1070c4];

fn at(base: u32, off: u32) -> u32 {
    base.wrapping_add(off)
}

/// `f0f50 (object, s0, s1, s2)`; `ret 10`.
pub(super) fn vehicle_pose(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::VEHICLE_POSE.base {
        c.esp = arithmetic(c, c.esp, 0xc, true);
        let frame = c.esp;
        c.eax = g.u32(at(frame, 0x18))?;
        c.ecx = g.u32(at(frame, 0x1c))?;
        c.edx = g.u32(at(frame, 0x14))?;
        g.set_u32(frame, c.eax)?;
        c.eax = frame;
        push(c, g, c.eax)?;
        g.set_u32(at(frame, 4), c.ecx)?;
        c.ecx = g.u32(at(frame, 0x10))?;
        push(c, g, VEHICLE_POSE_CALLBACK)?;
        push(c, g, c.ecx)?;
        g.set_u32(at(frame, 8), c.edx)?;
        return Ok(Next::Call { target: VEHICLE_POSE_CHANNELS, site: VEHICLE_SITES[0] });
    }
    if pc == VEHICLE_SITES[0] {
        c.esp = arithmetic(c, c.esp, 0xc, false);
        return Ok(Next::Ret { pop: 0x10 });
    }
    Err(unowned(pc))
}

/// `and eax, ffff; shl eax, 4; mov r, [eax+ecx+8]`: a tag's definition.
fn definition(c: &mut CpuAbi, g: &Guest, tag: u32) -> Result<u32> {
    let index = tag & 0xffff;
    logic(c, index, 1 << 31);
    let offset = shl(c, index, 4, 32);
    c.eax = offset;
    g.u32(at(at(offset, c.ecx), 8))
}

/// `10709d..1070c4`: the second channel, then the exit.
fn second_channel(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(at(c.esi, 0x1ac))?;
    arithmetic(c, c.eax, c.edx, true);
    c.ebx = at(c.esi, 0x1ac);
    if c.eax != c.edx {
        let word = u32::from(g.u16(at(c.ebx, 6))?);
        cmp16(c, word, c.edx);
        if word != 0xffff {
            c.eax = g.u32(at(c.esp, 0x18))?;
            push(c, g, c.edi)?;
            push(c, g, c.ebp)?;
            push(c, g, c.eax)?;
            push(c, g, ONE)?;
            return Ok(Next::Call { target: CHANNEL_EVALUATE, site: DEVICE_SITES[1] });
        }
    }
    device_exit(c, g)
}

/// `1070c4`: `pop edi; pop ebp; pop ebx`, then [`device_ret`].
fn device_exit(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    device_ret(c, g)
}

/// `1070c7`: `pop esi; ret 10`.
fn device_ret(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.esi = pop(c, g)?;
    Ok(Next::Ret { pop: 0x10 })
}

/// `107000 (object, s0, s1, s2)`; `ret 10`.
pub(super) fn device_pose(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == DEVICE_SITES[0] {
        c.edx = NONE;
        logic(c, c.edx, 1 << 31);
        return second_channel(c, g);
    }
    if pc == DEVICE_SITES[1] {
        return device_exit(c, g);
    }
    if pc != points::DEVICE_POSE.base {
        return Err(unowned(pc));
    }
    c.eax = g.u32(at(c.esp, 4))?;
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.edx = g.u32(at(c.ecx, DATA))?;
    c.ecx = g.u32(points::TAG_ENTRIES.base)?;
    c.eax &= 0xffff;
    logic(c, c.eax, 1 << 31);
    c.eax = c.eax.wrapping_mul(3);
    push(c, g, c.esi)?;
    c.esi = g.u32(at(c.edx, c.eax.wrapping_mul(4).wrapping_add(8)))?;
    // The object's definition, then its model (`+38`).
    let tag = g.u32(c.esi)?;
    let def = definition(c, g, tag)?;
    c.eax = g.u32(at(def, 0x38))?;
    c.edx = NONE;
    logic(c, c.edx, 1 << 31);
    arithmetic(c, c.eax, c.edx, true);
    if c.eax == NONE {
        return device_ret(c, g);
    }
    // The model's animation graph (`+4`).
    let model = definition(c, g, c.eax)?;
    c.eax = g.u32(at(model, 4))?;
    arithmetic(c, c.eax, c.edx, true);
    if c.eax == NONE {
        return device_ret(c, g);
    }
    c.eax &= 0xffff;
    logic(c, c.eax, 1 << 31);
    c.eax = shl(c, c.eax, 4, 32);
    let block = u32::from(g.u16(at(c.esi, 0x12a))?);
    cmp16(c, block, c.edx);
    c.ecx = g.u32(at(at(c.eax, c.ecx), 8))?;
    g.set_u32(at(c.esp, 8), c.ecx)?;
    if block == 0xffff {
        return device_ret(c, g);
    }
    // The first channel (`+18c`).
    c.eax = g.u32(at(c.esi, 0x18c))?;
    arithmetic(c, c.eax, c.edx, true);
    push(c, g, c.ebx)?;
    c.ebx = at(c.esi, 0x18c);
    push(c, g, c.ebp)?;
    c.ebp = g.u32(at(c.esp, 0x18))?;
    push(c, g, c.edi)?;
    c.edi = g.u32(at(c.esp, 0x20))?;
    if c.eax != c.edx {
        let word = u32::from(g.u16(at(c.ebx, 6))?);
        cmp16(c, word, c.edx);
        if word != 0xffff {
            c.edx = g.u32(at(c.esp, 0x18))?;
            push(c, g, c.edi)?;
            push(c, g, c.ebp)?;
            push(c, g, c.edx)?;
            push(c, g, ONE)?;
            return Ok(Next::Call { target: CHANNEL_EVALUATE, site: DEVICE_SITES[0] });
        }
    }
    second_channel(c, g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const STACK: u32 = 0x0070_0000;

    fn guest() -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0, 0x0080_0000, Permissions::RW).unwrap();
        g
    }

    /// The return address and `(object, s0, s1, s2)` at `STACK`.
    fn frame(g: &mut Guest, object: u32) -> CpuAbi {
        for (i, v) in [0xdead_0000, object, 0x5000, 0x6000, 0x7000].into_iter().enumerate() {
            g.set_u32(STACK + 4 * i as u32, v).unwrap();
        }
        CpuAbi { esp: STACK, ebx: 0xb0, ebp: 0xb1, esi: 0xb2, edi: 0xb3, ..Default::default() }
    }

    #[test]
    fn the_vehicle_pose_hands_f0760_a_frame_of_s1_s2_s0() {
        let mut g = guest();
        let mut c = frame(&mut g, 0x8000_0001);
        let next = vehicle_pose(&mut Cx { cpu: &mut c, g: &mut g }, points::VEHICLE_POSE.base).unwrap();
        assert_eq!(next, Next::Call { target: VEHICLE_POSE_CHANNELS, site: VEHICLE_SITES[0] });
        let frame = STACK - 0xc;
        assert_eq!(c.esp, frame - 0xc);
        assert_eq!([g.u32(c.esp).unwrap(), g.u32(c.esp + 4).unwrap(), g.u32(c.esp + 8).unwrap()], [0x8000_0001, VEHICLE_POSE_CALLBACK, frame]);
        assert_eq!([g.u32(frame).unwrap(), g.u32(frame + 4).unwrap(), g.u32(frame + 8).unwrap()], [0x6000, 0x7000, 0x5000]);
        assert_eq!((c.eax, c.ecx, c.edx), (frame, 0x8000_0001, 0x5000));
        // `f0760` pops its three arguments; the body frees the frame.
        c.esp = frame;
        let next = vehicle_pose(&mut Cx { cpu: &mut c, g: &mut g }, VEHICLE_SITES[0]).unwrap();
        assert_eq!(next, Next::Ret { pop: 0x10 });
        assert_eq!(c.esp, STACK);
    }

    /// Slot 1's body at `0x3000`, definition tag 1 (`0x6000`) whose model
    /// is tag 2 (`0x6100`) whose graph is tag 3 (`0x6200`).
    fn device(g: &mut Guest, model: u32, graph: u32, block: u16, channels: [(u32, u16); 2]) -> u32 {
        let (objects, data, body, tags) = (0x1000, 0x2000, 0x3000, 0x5000);
        g.set_u32(points::OBJECTS.base, objects).unwrap();
        g.set_u32(objects + DATA, data).unwrap();
        g.set_u32(data + 12 + 8, body).unwrap();
        g.set_u32(body, 0xe000_0001).unwrap();
        g.set_u32(points::TAG_ENTRIES.base, tags).unwrap();
        for (tag, def) in [(1, 0x6000), (2, 0x6100), (3, 0x6200)] {
            g.set_u32(tags + 16 * tag + 8, def).unwrap();
        }
        g.set_u32(0x6000 + 0x38, model).unwrap();
        g.set_u32(0x6100 + 4, graph).unwrap();
        g.set_u16(body + 0x12a, block).unwrap();
        for (off, (handle, word)) in [0x18c, 0x1ac].into_iter().zip(channels) {
            g.set_u32(body + off, handle).unwrap();
            g.set_u16(body + off + 6, word).unwrap();
        }
        0x8001_0001
    }

    fn run(g: &mut Guest, c: &mut CpuAbi, pc: u32) -> Next {
        device_pose(&mut Cx { cpu: c, g }, pc).unwrap()
    }

    #[test]
    fn the_device_pose_returns_without_a_model_a_graph_or_a_block() {
        for (model, graph, block) in [(NONE, 3, 0x180), (2, NONE, 0x180), (2, 3, 0xffff)] {
            let mut g = guest();
            let object = device(&mut g, model, graph, block, [(1, 1), (1, 1)]);
            let mut c = frame(&mut g, object);
            assert_eq!(run(&mut g, &mut c, points::DEVICE_POSE.base), Next::Ret { pop: 0x10 });
            assert_eq!((c.esp, c.esi, c.edx), (STACK, 0xb2, NONE));
            assert_eq!(c.eflags & 0x40, 0x40, "the compare that ended it was equal");
            let stored = g.u32(STACK + 4).unwrap();
            assert_eq!(stored == 0x6200, block == 0xffff, "the graph over the argument once found");
        }
    }

    #[test]
    fn the_device_pose_evaluates_each_live_channel() {
        let cases: [([(u32, u16); 2], &[u32]); 4] = [
            ([(1, 1), (2, 2)], &[0x3000 + 0x18c, 0x3000 + 0x1ac]),
            ([(NONE, 1), (2, 2)], &[0x3000 + 0x1ac]),
            ([(1, 0xffff), (NONE, 2)], &[]),
            ([(1, 1), (2, 0xffff)], &[0x3000 + 0x18c]),
        ];
        for (channels, calls) in cases {
            let mut g = guest();
            let object = device(&mut g, 2, 3, 0x180, channels);
            let mut c = frame(&mut g, object);
            let mut pc = points::DEVICE_POSE.base;
            let mut made = Vec::new();
            loop {
                match run(&mut g, &mut c, pc) {
                    Next::Call { target, site } => {
                        assert_eq!(target, CHANNEL_EVALUATE);
                        let args: Vec<u32> = (0..4).map(|i| g.u32(c.esp + 4 * i).unwrap()).collect();
                        assert_eq!(args, [ONE, 0x5000, 0x6000, 0x7000]);
                        made.push(c.ebx);
                        if made.len() == 1 {
                            assert_eq!(c.ecx, 0x6200, "the graph definition");
                        }
                        // The evaluate pops its four arguments and clobbers `edx`.
                        c.esp += 0x10;
                        c.edx = 0x1234;
                        pc = site;
                    }
                    Next::Ret { pop } => {
                        assert_eq!(pop, 0x10);
                        break;
                    }
                    other => panic!("{other:?}"),
                }
            }
            assert_eq!(made, calls);
            assert_eq!((c.esp, c.ebx, c.ebp, c.esi, c.edi), (STACK, 0xb0, 0xb1, 0xb2, 0xb3));
        }
    }
}
