//! Bind clips to the manager's three channels and advance their blend state.
use super::*;
use crate::function_eval::constant::ONE;
use guest_layout::points::TAG_ENTRIES;
use crate::sim_native::Memory;
use crate::sim_objects::sse::{Ss, comiss, cvtsi2ss, lane0 as lane, load as load_ss, ss};
use crate::sim_physics::{arithmetic, byte, logic};
use runtime::{CpuAbi, GuestFault, TransferAbi};

mod advance;
mod bind;
mod reset;

type Transfer = Result<TransferAbi, GuestFault>;
const NONE: u32 = u32::MAX;

pub(super) const BODIES: &[Body] = &[
    body!(0x1cb0a0, 0x1cb0c8, &[], reset::invalidate),
    body!(0x1cb0d0, 0x1cb230, &[0x1cb200], reset::initialize),
    body!(
        0x1cb230,
        0x1cb3ac,
        &[0x1cb2be, 0x1cb2db, 0x1cb2f2, 0x1cb33a],
        reset::rebind
    ),
    body!(0x1cb3b0, 0x1cb402, &[0x1cb3f2], bind::remap),
    body!(
        0x1cb410,
        0x1cb48c,
        &[0x1cb436, 0x1cb45a, 0x1cb46c],
        bind::select
    ),
    body!(0x1cb490, 0x1cb4b9, &[0x1cb4b1], bind::by_handle),
    body!(0x1cb4c0, 0x1cb519, &[0x1cb4e9, 0x1cb50d], bind::by_name),
    body!(0x1cb520, 0x1cb5a7, &[0x1cb577, 0x1cb59b], bind::by_state),
    body!(0x1cb5b0, 0x1cb5e5, &[0x1cb5df], advance::advance_one),
    body!(
        0x1cb5f0,
        0x1cb675,
        &[0x1cb625, 0x1cb65d],
        advance::begin_blend
    ),
    body!(0x1cb680, 0x1cb70e, &[], advance::blend_flags),
    body!(
        0x1cb710,
        0x1cb914,
        &[
            0x1cb750, 0x1cb7a9, 0x1cb7d2, 0x1cb835, 0x1cb87f, 0x1cb8e4, 0x1cb909
        ],
        advance::manager
    ),
];

fn eq32(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    arithmetic(c, a, b, true) == 0
}

fn eq_narrow(c: &mut CpuAbi, a: u32, b: u32, bits: u32) -> bool {
    crate::sim_native::sub_flags(c, a, b, bits);
    c.eflags & 0x40 != 0
}

fn test(c: &mut CpuAbi, value: u8) -> bool {
    logic(c, u32::from(value), 0x80);
    value != 0
}

fn valid(c: &mut CpuAbi, g: &impl Memory, ch: u32, none: u32) -> Result<bool, GuestFault> {
    Ok(!eq32(c, g.u32(ch)?, none) && !eq_narrow(c, g.u16(at(ch, 6))?.into(), none, 16))
}

fn tag_offset(c: &mut CpuAbi, handle: u32) -> u32 {
    let v = (handle & 0xffff) << 4;
    logic(c, v, 1 << 31);
    c.eflags_defined &= !0x800;
    v
}

fn tag(g: &impl Memory, table: u32, offset: u32) -> Result<u32, GuestFault> {
    g.u32(at(at(table, offset), 8))
}

fn frame(c: &CpuAbi, g: &impl Memory, off: u32) -> Result<u32, GuestFault> {
    g.u32(at(c.esp, off))
}
