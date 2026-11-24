//! Weight and evaluate the two aim overlays for the angles in the pose frame.
use super::*;

const UNIT_ACTION: u32 = 0x111590;
const ACTION_ALLOWED: u32 = 0x10ee20;
const GET_WEIGHT: u32 = 0x1d9370;
const INIT_CHANNEL: u32 = 0x1c62f0;
const RESOLVE: u32 = 0x1cb490;
const APPLY: u32 = 0x1c7100;
const NEAR_ONE: u32 = 0x45dc78;

// The two action names are immediates in the loaded image.
const ACTION_NAMES: [u32; 2] = [0x112783, 0x11278a];

/// Compare the angle's magnitude with SMALL in extended precision. Between
/// loading the second angle and comparing it, the guest sets the neutral
/// flag provisionally; a failed comparison clears it in the caller.
fn angle_is_small(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    second: bool,
) -> std::result::Result<bool, GuestFault> {
    let (slot, last) = if second {
        (0x34, 0x1127c2)
    } else {
        (0x38, 0x1127ab)
    };
    let mut x = ray::load_fpu(c);
    fp::ld_f32(&mut x, g.u32(at(c.esp, slot))?);
    if second {
        g.set_u8(at(c.esp, 0x13), 1)?;
    }
    magnitude_is_small(c, g, &mut x, last)
}

fn active_flag(c: &mut CpuAbi, value: u8) -> bool {
    c.eax = low(c.eax, value);
    test_byte(c, value);
    value != 0
}

fn neutral(c: &mut CpuAbi, g: &impl Memory) -> std::result::Result<bool, GuestFault> {
    Ok(active_flag(c, g.u8(at(c.esp, 0x13))?))
}

fn finish(c: &mut CpuAbi, g: &impl Memory) -> Segment {
    let active = active_flag(c, g.u8(at(c.esp, 0x12))?);
    put_xmm(c, 0, scalar(g.u32(0x4454e4)?));
    Ok(Step::Next(if active { 0x1129cd } else { 0x1129c5 }))
}

/// The single-channel paths take the overall weight directly. Which
/// resolved channel the guest passes in ESI is their only difference.
fn apply_single(c: &mut CpuAbi, g: &mut impl Memory, channel: u32) -> Segment {
    let sp = c.esp;
    c.eax = g.u32(at(sp, 0xf4))?;
    c.ecx = g.u32(at(sp, 0xf0))?;
    c.edx = g.u32(at(sp, 0xec))?;
    push(c, g, c.eax)?;
    c.eax = g.u32(at(sp, 0x1c))?;
    push(c, g, c.ecx)?;
    c.ecx = g.u32(at(sp, 0x38))?;
    push(c, g, c.edx)?;
    c.edx = g.u32(at(sp, 0x34))?;
    for arg in [c.eax, c.ecx, c.edx] {
        push(c, g, arg)?;
    }
    c.esi = at(sp, channel);
    invoke(c, g, APPLY, 0x1129b5)
}

/// Both channels resolved. Their calls share arguments but leave different
/// register values for the evaluator and return to different sites.
fn apply_pair(c: &mut CpuAbi, g: &mut impl Memory, first: bool) -> Segment {
    let sp = c.esp;
    let (args, channel, site) = if first {
        c.ecx = g.u32(at(sp, 0xf4))?;
        c.edx = g.u32(at(sp, 0xf0))?;
        c.eax = g.u32(at(sp, 0xec))?;
        push(c, g, c.ecx)?;
        c.ecx = g.u32(at(sp, 0x24))?;
        ([c.edx, c.eax, c.ecx], 0xa4, 0x112901)
    } else {
        c.edx = g.u32(at(sp, 0xf4))?;
        c.eax = g.u32(at(sp, 0xf0))?;
        c.ecx = g.u32(at(sp, 0xec))?;
        push(c, g, c.edx)?;
        c.edx = g.u32(at(sp, 0x14))?;
        ([c.eax, c.ecx, c.edx], 0xc4, 0x1129b5)
    };
    for arg in args.into_iter().chain([c.ebx, c.edi]) {
        push(c, g, arg)?;
    }
    c.esi = at(sp, channel);
    invoke(c, g, APPLY, site)
}

fn apply_second_if_weighted(c: &mut CpuAbi, g: &mut impl Memory) -> Segment {
    let weight = g.u32(at(c.esp, 0x14))?;
    put_xmm(c, 0, scalar(weight));
    if compare(c, weight, xmm(c, 3)[0]) == 0 {
        apply_pair(c, g, false)
    } else {
        finish(c, g)
    }
}

fn blend(c: &mut CpuAbi, g: &mut impl Memory) -> Segment {
    let sp = c.esp;
    let total = g.u32(at(sp, 0x1c))?;
    put_xmm(c, 2, scalar(total));
    let small = g.u32(SMALL)?;
    put_xmm(c, 3, scalar(small));
    if compare(c, total, small) != 0 {
        return finish(c, g);
    }

    logic_flags(c, c.esi);
    if c.esi == 0 {
        logic_flags(c, c.edi);
        return if c.edi == 0 {
            finish(c, g)
        } else {
            apply_single(c, g, 0xc4)
        };
    }
    logic_flags(c, c.edi);
    if c.edi == 0 {
        let near_one = g.u32(NEAR_ONE)?;
        put_xmm(c, 0, scalar(near_one));
        if compare(c, near_one, total) != 0 && neutral(c, g)? {
            return finish(c, g);
        }
        return apply_single(c, g, 0xa4);
    }

    let one = g.u32(ONE)?;
    put_xmm(c, 4, scalar(one));
    let fraction = g.u32(at(sp, 0x20))?;
    put_xmm(c, 1, scalar(fraction));
    c.edi = g.u32(at(sp, 0x34))?;
    c.ebx = g.u32(at(sp, 0x38))?;
    let complement = op(c, Sub, one, fraction);
    let first = op(c, Mul, complement, total);
    put_xmm(c, 0, scalar(first));
    let first_is_small = compare(c, small, first) == 0;
    let second = op(c, Mul, fraction, total);
    put_xmm(c, 1, scalar(second));
    g.set_u32(at(sp, 0x24), first)?;
    g.set_u32(at(sp, 0x14), second)?;
    if first_is_small {
        g.set_u32(at(sp, 0x14), one)?;
        return apply_pair(c, g, false);
    }

    if compare(c, small, second) == 0 {
        put_xmm(c, 0, scalar(0));
        g.set_u32(at(sp, 0x24), one)?;
        g.set_u32(at(sp, 0x14), 0)?;
    } else {
        if compare(c, first, small) != 0 {
            return apply_second_if_weighted(c, g);
        }
        let near_one = g.u32(NEAR_ONE)?;
        put_xmm(c, 1, scalar(near_one));
        if compare(c, near_one, first) == 0 {
            return apply_pair(c, g, true);
        }
    }
    if neutral(c, g)? {
        apply_second_if_weighted(c, g)
    } else {
        apply_pair(c, g, true)
    }
}

pub(super) fn run(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Segment {
    let sp = c.esp;
    match pc {
        0x112769 => {
            c.eax = g.u32(at(sp, 0xe8))?;
            push(c, g, 4)?;
            return invoke(c, g, UNIT_ACTION, 0x112777);
        }
        0x112777 => {
            c.ecx = g.u32(at(sp, 0x18))?;
            g.set_u8(at(sp, 0x12), c.eax as u8)?;
            c.eax = g.u32(at(c.ecx, 0x7c))?;
            for operand in ACTION_NAMES {
                let name = g.u32(operand)?;
                sub_flags(c, c.eax, name, 32);
                if c.eax == name {
                    return invoke(c, g, ACTION_ALLOWED, 0x112795);
                }
            }
        }
        0x112795 => {
            let active = c.eax as u8;
            test_byte(c, active);
            if active == 0 {
                g.set_u8(at(sp, 0x12), active)?;
            }
        }
        0x1127e4 => {
            c.eax = at(c.ebp, 0x88);
            g.set_u32(at(sp, 0x1c), xmm(c, 0)[0])?;
            return invoke(c, g, GET_WEIGHT, 0x1127f5);
        }
        0x1127f5 => {
            c.ecx = at(sp, 0xa4);
            g.set_u32(at(sp, 0x20), xmm(c, 0)[0])?;
            return invoke(c, g, INIT_CHANNEL, 0x112807);
        }
        0x112807 => {
            c.ecx = at(sp, 0xc4);
            return invoke(c, g, INIT_CHANNEL, 0x112813);
        }
        0x112813 => {
            c.eax = g.u32(at(sp, 0x24))?;
            push(c, g, c.eax)?;
            c.eax = g.u32(at(sp, 0x18))?;
            c.ecx = at(sp, 0xa4);
            push(c, g, c.ecx)?;
            c.ebx = 0;
            logic_flags(c, 0);
            return invoke(c, g, RESOLVE, 0x11282b);
        }
        0x11282b => {
            c.edx = g.u32(at(sp, 0x14))?;
            push(c, g, c.edx)?;
            c.eax = at(sp, 0xc4);
            push(c, g, c.eax)?;
            c.eax = g.u32(at(sp, 0x18))?;
            return invoke(c, g, RESOLVE, 0x112841);
        }
        0x112841 => return blend(c, g),
        0x112901 => {
            put_xmm(c, 3, scalar(g.u32(SMALL)?));
            return apply_second_if_weighted(c, g);
        }
        0x1129b5 => return finish(c, g),
        _ => unreachable!("aim overlay continuation {pc:#x}"),
    }

    if !angle_is_small(c, g, false)? || !angle_is_small(c, g, true)? {
        g.set_u8(at(sp, 0x13), 0)?;
    }
    if !active_flag(c, g.u8(at(c.ebp, 0x91))?) {
        return finish(c, g);
    }
    c.eax = at(c.ebp, 0x90);
    invoke(c, g, GET_WEIGHT, 0x1127e4)
}
