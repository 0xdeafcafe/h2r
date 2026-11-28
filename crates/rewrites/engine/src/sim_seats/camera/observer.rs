//! Observer frame. Each callback can change a later user's mapping or the
//! listener's cluster, so the loop reads these at their use sites.
use crate::light_constants::Error;
use guest_layout::points;
use crate::sim_native::sub_flags;
use crate::sim_physics::spatial::fp_error;
use crate::sim_physics::{Result, byte, logic, pop, push};
use runtime::{CpuAbi, Guest, TransferAbi};

const FRAME_SCALE: u32 = 0x0046_8d28;
const FRAME_STEP: u32 = 0x004e_9bd0;
const FIRST_OBSERVER: u32 = 0x004e_9c8c;
const OBSERVER_STRIDE: u32 = 0x358;
const OBSERVERS_END: u32 = FIRST_OBSERVER + 4 * OBSERVER_STRIDE;
const BSP_INDEX: u32 = 0x0046_86c4;
const BSP: u32 = 0x004e_033c;
const LEAF_DATA: u32 = 0x004e_0348;
const RESOURCE_STATE: u32 = 0x0051_0c50;
const PREDICT_CLUSTER: u32 = 0x004e_a934;
const ZERO: u32 = crate::function_eval::constant::ZERO;

const AFTER_COMMAND: u32 = 0x16f2da;
const AFTER_ADVANCE: u32 = 0x16f2f4;
const AFTER_PLACEMENT: u32 = 0x16f2fa;
const AFTER_LISTENER: u32 = 0x16f301;
const AFTER_LEAF: u32 = 0x16f31c;
const AFTER_PREDICT: u32 = 0x16f35d;
const AFTER_RESOURCES: u32 = 0x16f367;
pub const SITES: &[u32] = &[
    AFTER_COMMAND,
    AFTER_ADVANCE,
    AFTER_PLACEMENT,
    AFTER_LISTENER,
    AFTER_LEAF,
    AFTER_PREDICT,
    AFTER_RESOURCES,
];

pub fn supported(c: &CpuAbi, _: &Guest) -> bool {
    c.mxcsr & 0x1f80 == 0x1f80
}

fn resources(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    c.ecx = g.u32(RESOURCE_STATE)?;
    if c.ecx != 0 {
        let suppress = g.u8(c.ecx.wrapping_add(5))?;
        c.edx = byte(c.edx, suppress);
        if suppress != 0 {
            return next_user(c, g);
        }
    }
    let predict = g.u8(PREDICT_CLUSTER)?;
    c.ecx = byte(c.ecx, predict);
    logic(c, u32::from(predict), 0x80);
    if predict == 0 {
        c.eax = c.eax as i16 as i32 as u32;
        return g.call(c, 0x3f500, AFTER_RESOURCES);
    }

    let cluster = c.eax as u16;
    sub_flags(c, u32::from(cluster), u32::from(u16::MAX), 16);
    if cluster == u16::MAX {
        return next_user(c, g);
    }
    c.eax = cluster as i16 as i32 as u32;
    g.call(c, 0x3f450, AFTER_PREDICT)
}

/// Walk through unmapped seats in one Rust call. At a guest callback
/// EDI is the user number, ESI the observer position, EBP the map offset,
/// and EBX the observer array offset. The guest frame is the continuation.
fn visit(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    loop {
        if c.ebx != 0u32.wrapping_sub(OBSERVER_STRIDE) {
            c.eax = g.u32(points::PLAYER_GLOBALS.base)?;
            let mapped = g.u32(c.eax.wrapping_add(c.ebp))? != u32::MAX;
            c.eax = byte(c.eax, u8::from(mapped));
            if mapped {
                logic(c, 1, 0x80);
                push(c, g, c.edi)?;
                g.set_u8(c.esi.wrapping_sub(4), 1)?;
                return g.call(c, 0x16f570, AFTER_COMMAND);
            }
        }
        if !advance(c) {
            return finish(c, g);
        }
    }
}

fn advance(c: &mut CpuAbi) -> bool {
    c.esi = c.esi.wrapping_add(OBSERVER_STRIDE);
    c.edi = c.edi.wrapping_add(1);
    c.ebx = c.ebx.wrapping_add(OBSERVER_STRIDE);
    c.ebp = c.ebp.wrapping_add(4);
    sub_flags(c, c.esi, OBSERVERS_END, 32);
    (c.esi as i32) < (OBSERVERS_END as i32)
}

fn finish(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    g.set_u8(PREDICT_CLUSTER, 0)?;
    c.ebx = pop(c, g)?;
    g.ret(c, 4)
}

fn next_user(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    if advance(c) {
        visit(c, g)
    } else {
        finish(c, g)
    }
}

pub fn run(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    match pc {
        pc if pc == points::CAMERA_OBSERVER.base => {
            let scale = g.u32(FRAME_SCALE)?;
            let dt = g.u32(c.esp.wrapping_add(4))?;
            let step = simd::mulss(&mut c.mxcsr, u128::from(scale), dt)
                .map_err(|_| fp_error(Error::Simd))?;
            c.xmm[0] = step.to_le_bytes();
            for saved in [c.ebx, c.ebp, c.esi, c.edi] {
                push(c, g, saved)?;
            }
            c.edi = 0;
            g.set_u32(FRAME_STEP, step as u32)?;
            c.ebp = 12;
            c.esi = FIRST_OBSERVER;
            c.ebx = 0;
            visit(c, g)
        }
        AFTER_COMMAND => {
            let step = g.u32(FRAME_STEP)?;
            c.xmm[0] = u128::from(step).to_le_bytes();
            let f = simd::ucomiss(&mut c.mxcsr, step, g.u32(ZERO)?)
                .map_err(|_| fp_error(Error::Simd))?;
            let ah = f & 0xd5 | 2;
            c.eax = (c.eax & !0xff00) | (ah << 8);
            logic(c, ah & 0x44, 0x80);
            if f & 0x44 != 0x40 {
                g.call(c, 0x16fe90, AFTER_ADVANCE)
            } else {
                place(c, g)
            }
        }
        AFTER_ADVANCE => place(c, g),
        AFTER_PLACEMENT => {
            c.eax = c.edi;
            g.call(c, 0x16ebf0, AFTER_LISTENER)
        }
        AFTER_LISTENER => {
            c.eax = u32::MAX;
            if g.u16(BSP_INDEX)? == u16::MAX {
                return resources(c, g);
            }
            c.ecx = g.u32(BSP)?;
            c.edx = c.esi;
            c.eax = 0;
            logic(c, 0, 1 << 31);
            g.call(c, 0x14a280, AFTER_LEAF)
        }
        AFTER_LEAF => {
            if c.eax != u32::MAX {
                c.ecx = g.u32(LEAF_DATA)?;
                c.edx = g.u32(c.ecx.wrapping_add(48))?;
                let index = g.u16(c.edx.wrapping_add(c.eax.wrapping_mul(8)))?;
                c.eax = (c.eax & !0xffff) | u32::from(index);
            }
            resources(c, g)
        }
        AFTER_PREDICT | AFTER_RESOURCES => next_user(c, g),
        _ => Err(runtime::GuestFault {
            addr: pc,
            len: 1,
            access: runtime::guest::Access::Execute,
            reason: runtime::guest::FaultReason::InvalidOperation,
        }),
    }
}

fn place(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    push(c, g, c.edi)?;
    g.call(c, 0x170fd0, AFTER_PLACEMENT)
}
