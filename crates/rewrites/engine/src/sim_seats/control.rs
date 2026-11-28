//! The common simulation consumer of a player's action. A local controller,
//! phantom, remote peer and film all reach the same action record here.
//! Each callback sees its original frame, and can change the values the
//! following segment gathers. The neutral path carries forward the unit's
//! three distinct view directions.
use super::leaf::{self, Leaf};
use crate::points;
use crate::sim_native::{Memory, plan::segment};
use crate::sim_physics::{
    arithmetic, logic, pop, push,
    spatial::{finish_fpu, fp_error, load_fpu},
};
use guest_layout::{Player, Unit};
use runtime::{
    CpuAbi, Guest, GuestFault, TransferAbi,
    guest::{Access, FaultReason},
};
use x87::abi::arith_op;

type Result<T> = std::result::Result<T, GuestFault>;
type Step = Result<Option<TransferAbi>>;
type Vector = [u32; 3];

pub const ACTION_SITES: &[u32] = &[0x150651, 0x15066a, 0x150673, 0x150760, 0x15077a];
pub const NEUTRAL_SITES: &[u32] = &[0x1507b4, 0x15080d];
const PROJECT_VIEW: u32 = 0xcb810;
const MOVEMENT_BASIS: u32 = 0x150400;
// Operand of the action's assignment of the control header. The build's
// default comes from the mapped image, including when init was hooked.
const ACTION_CONTROL_HEADER: u32 = 0x150720;
const DIRECTION_FLAG: u32 = 1 << 10;

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}

fn vector(m: &impl Memory, addr: u32) -> Result<Vector> {
    Ok([m.u32(addr)?, m.u32(at(addr, 4))?, m.u32(at(addr, 8))?])
}

fn store_vector(m: &mut impl Memory, addr: u32, v: Vector) -> Result<()> {
    for (i, component) in v.into_iter().enumerate() {
        m.set_u32(at(addr, i as u32 * 4), component)?;
    }
    Ok(())
}

// Gathering a record before writing it is legal only across disjoint
// physical ranges. A refusal drops the complete segment (including pushes)
// and lets its original run with the live CPU and untouched memory.
fn disjoint(m: &impl Memory, a: (u32, usize), b: (u32, usize)) -> Result<()> {
    let fault = || GuestFault {
        addr: a.0,
        len: a.1,
        access: Access::Read,
        reason: FaultReason::InvalidBuffer,
    };
    // One lookup per page: either range can cross page mappings.
    let x = crate::sim_native::spans::Spans::of(m, a.0, a.1).ok_or_else(fault)?;
    let y = crate::sim_native::spans::Spans::of(m, b.0, b.1).ok_or_else(fault)?;
    if x.overlaps(&y) {
        Err(fault())
    } else {
        Ok(())
    }
}

fn data(m: &impl Memory, global: u32) -> Result<u32> {
    m.u32(at(m.u32(global)?, 0x44))
}

fn object(m: &impl Memory, table: u32, handle: u32) -> Result<u32> {
    m.u32(at(table, (handle & 0xffff) * 12 + 8))
}

fn direction(c: &mut CpuAbi, m: &mut impl Memory) -> Result<()> {
    let input = c.ebp;
    let out = at(c.esp, 0x10);
    let mut x = load_fpu(c);
    x.fld(m.u32(at(input, 8))?).map_err(fp_error)?;
    x.fcos().map_err(fp_error)?;
    for (offset, sine) in [(0, false), (4, true)] {
        x.fld(m.u32(at(input, 4))?).map_err(fp_error)?;
        if sine { x.fsin() } else { x.fcos() }.map_err(fp_error)?;
        x.arith_st(arith_op::MUL, 1).map_err(fp_error)?;
        m.set_u32(at(out, offset), x.fstp_m32().map_err(fp_error)?)?;
    }
    x.fstp_st(0).map_err(fp_error)?;
    x.fld(m.u32(at(input, 8))?).map_err(fp_error)?;
    x.fsin().map_err(fp_error)?;
    m.set_u32(at(out, 8), x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c, m, x, 0x150641, at(out, 8))
}

fn movement(c: &mut CpuAbi, m: &mut impl Memory) -> Result<TransferAbi> {
    let sp = c.esp;
    c.eax = at(sp, 0x28);
    push(c, m, c.eax)?;
    c.ecx = at(sp, 0x1c);
    push(c, m, c.ecx)?;
    c.edx = at(sp, 0x10);
    c.ebx = at(c.ebp, 0xc);
    push(c, m, c.edx)?;
    c.eax = c.esi;
    m.call(c, MOVEMENT_BASIS, ACTION_SITES[1])
}

/// Transient packet fields supplied by the action and the movement helper.
/// Remaining fields (including the fourth vector) belong to `c6ef0`
/// ([`Leaf::InitPacket`]).
struct ActionControl {
    flags: u32,
    throttle: Vector,
    triggers: [u32; 2],
    facing: Vector,
    look: Vector,
    selections: [u8; 8],
    aim_assist: [u8; 36],
}

impl ActionControl {
    fn gather(m: &impl Memory, input: u32, frame: u32) -> Result<Self> {
        Ok(Self {
            flags: m.u32(input)?,
            throttle: vector(m, at(frame, 0x28))?,
            triggers: [m.u32(at(input, 0x14))?, m.u32(at(input, 0x18))?],
            facing: vector(m, at(frame, 0x1c))?,
            look: vector(m, at(frame, 0x10))?,
            selections: m.load(at(input, 0x1e))?,
            aim_assist: m.load(at(input, 0x34))?,
        })
    }

    fn publish(&self, m: &mut impl Memory, packet: u32) -> Result<()> {
        m.set_u32(packet, m.u32(ACTION_CONTROL_HEADER)?)?;
        m.set_u16(at(packet, 4), 0)?;
        m.write_bytes(at(packet, 6), &self.selections[..4], 1)?;
        m.write_bytes(at(packet, 10), &self.selections[4..], 1)?;
        m.set_u32(at(packet, 0x10), self.flags)?;
        store_vector(m, at(packet, 0x14), self.throttle)?;
        for (offset, value) in [0x20, 0x24].into_iter().zip(self.triggers) {
            m.set_u32(at(packet, offset), value)?;
        }
        store_vector(m, at(packet, 0x28), self.facing)?;
        store_vector(m, at(packet, 0x34), self.look)?;
        store_vector(m, at(packet, 0x40), self.look)?;
        for (i, word) in self.aim_assist.chunks_exact(4).enumerate() {
            m.write_bytes(at(packet, 0x58 + i as u32 * 4), word, 1)?;
        }
        Ok(())
    }
}

fn submit(c: &mut CpuAbi, m: &mut impl Memory) -> Result<TransferAbi> {
    let sp = c.esp;
    let input = c.ebp;
    let packet = at(sp, 0x34);
    disjoint(m, (input, 0x59), (packet, 0x7c))?;
    disjoint(m, (sp, 0x34), (packet, 0x7c))?;
    // Resolve the object array before publishing the packet. Neither
    // pointer load can observe a partial packet on this fast path.
    let header = m.u32(points::OBJECTS.base)?;
    disjoint(m, (points::OBJECTS.base, 4), (packet, 0x7c))?;
    disjoint(m, (at(header, 0x44), 4), (packet, 0x7c))?;
    let table = m.u32(at(header, 0x44))?;
    let control = ActionControl::gather(m, input, sp)?;
    let unit = m.u32(at(sp, 0xb4))?;
    control.publish(m, packet)?;
    c.xmm[0] = u128::from(control.triggers[1]).to_le_bytes();
    c.eax = unit;
    c.edi = pop(c, m)?;
    let body = object(m, table, unit)?;
    let flagged = m.u8(at(input, 0x58))? != 0;
    c.esi = pop(c, m)?;
    c.ebp = pop(c, m)?;
    let address = at(body, Unit::FLAGS.offset);
    let flags = m.u32(address)?;
    c.ebx = pop(c, m)?;
    let bit = 1 << 30;
    c.edx = if flagged { flags | bit } else { flags & !bit };
    logic(c, c.edx, 1 << 31);
    m.set_u32(address, c.edx)?;
    c.ecx = packet;
    m.call(
        c,
        points::UNIT_APPLY_CONTROL.base,
        if flagged {
            ACTION_SITES[3]
        } else {
            ACTION_SITES[4]
        },
    )
}

pub fn action(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Step {
    if c.eflags & DIRECTION_FLAG != 0 || c.x87.control & 0xc00 != 0 {
        return Ok(None);
    }
    // An answered `c6ef0` lays its packet in the same segment as `submit`:
    // 31 block pieces and about 30 stores.
    segment::<64, _>(
        c,
        g,
        pc,
        points::PLAYER_ACTION_TO_UNIT_CONTROL.base..0x150783,
        ACTION_SITES,
        |c, m, pc| action_step(c, m, pc),
    )
}

fn action_step(c: &mut CpuAbi, m: &mut impl Memory, pc: u32) -> Result<TransferAbi> {
    leaf::at_site(c, m, pc);
    match pc {
        0x150651 => movement(c, m),
        0x15066a => {
            c.edx = at(c.esp, 0x34);
            leaf::call(c, m, Leaf::InitPacket, ACTION_SITES[2], |c, m| {
                action_step(c, m, ACTION_SITES[2])
            })
        }
        0x150673 => submit(c, m),
        0x150760 | 0x15077a => {
            c.esp = arithmetic(c, c.esp, 0xa0, false);
            m.ret(c, 8)
        }
        _ => {
            c.esp = c.esp.wrapping_sub(0xa0);
            push(c, m, c.ebx)?;
            push(c, m, c.ebp)?;
            c.ebp = m.u32(at(c.esp, 0xb0))?;
            let saved = (c.esp.wrapping_sub(8), 8);
            disjoint(m, (at(c.ebp, 8), 4), saved)?;
            c.ecx = m.u32(points::PLAYERS.base)?;
            c.edx = m.u32(at(c.ecx, 0x44))?;
            push(c, m, c.esi)?;
            c.esi = c.eax;
            push(c, m, c.edi)?;
            let player = at(c.edx, (c.eax & 0xffff) * Player::SIZE);
            c.eax = m.u32(at(player, Player::UNIT.offset))?;
            arithmetic(c, c.eax, u32::MAX, true);
            direction(c, m)?;
            if c.eax == u32::MAX {
                return movement(c, m);
            }
            c.edx = at(c.esp, 0x10);
            push(c, m, c.edx)?;
            m.call(c, PROJECT_VIEW, ACTION_SITES[0])
        }
    }
}

pub fn neutral(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Step {
    // An answered `c6ef0` adds 31 block pieces to the entry's segment.
    segment::<48, _>(
        c,
        g,
        pc,
        points::PLAYER_NEUTRAL_UNIT_CONTROL.base..0x150812,
        NEUTRAL_SITES,
        |c, m, pc| neutral_step(c, m, pc),
    )
}

fn neutral_step(c: &mut CpuAbi, m: &mut impl Memory, pc: u32) -> Result<TransferAbi> {
    leaf::at_site(c, m, pc);
    match pc {
        0x1507b4 => {
            let packet = at(c.esp, 4);
            let body = c.esi;
            disjoint(
                m,
                (at(body, Unit::DESIRED_FACING.offset), 0x3c),
                (packet, 0x7c),
            )?;
            let facing = vector(m, at(body, Unit::DESIRED_FACING.offset))?;
            let aim = vector(m, at(body, Unit::DESIRED_AIM.offset))?;
            let look = vector(m, at(body, Unit::DESIRED_LOOK.offset))?;
            store_vector(m, at(packet, 0x28), facing)?;
            store_vector(m, at(packet, 0x34), aim)?;
            store_vector(m, at(packet, 0x40), look)?;
            c.esi = at(body, Unit::DESIRED_LOOK.offset);
            arithmetic(c, body, Unit::DESIRED_LOOK.offset, false);
            c.eax = c.edi;
            c.ecx = packet;
            c.edx = look[0];
            m.call(c, points::UNIT_APPLY_CONTROL.base, NEUTRAL_SITES[1])
        }
        0x15080d => {
            c.esi = pop(c, m)?;
            c.esp = arithmetic(c, c.esp, 0x7c, false);
            m.ret(c, 0)
        }
        _ => {
            c.esp = c.esp.wrapping_sub(0x7c);
            c.ecx = m.u32(points::OBJECTS.base)?;
            let table = m.u32(at(c.ecx, 0x44))?;
            let index = c.edi & 0xffff;
            logic(c, index, 1 << 31);
            c.eax = index * 3;
            push(c, m, c.esi)?;
            c.esi = object(m, table, c.edi)?;
            c.edx = at(c.esp, 4);
            leaf::call(c, m, Leaf::InitPacket, NEUTRAL_SITES[0], |c, m| {
                neutral_step(c, m, NEUTRAL_SITES[0])
            })
        }
    }
}

mod unit;
pub use unit::apply;
