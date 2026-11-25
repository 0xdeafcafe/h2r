//! Per-submix room and filter submission. The room setter can synchronously
//! deliver a guest callback. The record, both descriptors and the return sites
//! therefore live in their original guest locations throughout the update.

use crate::points;
use crate::sim_native::{add_flags, logic_flags, sub_flags};
use crate::sim_objects::frame::{pop, push, set_slot, slot};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_objects::{Body, Cx, Next, Result};
use crate::sim_physics::spatial::{finish_fpu, load_fpu};
use runtime::{CpuAbi, Guest};
use sim_sound::submix::{self, CutoffScale};

mod position;

const SOUND_GLOBALS: u32 = 0x51ebe4;
const FLOAT_ZERO: u32 = 0x45dbd8;
const FLOAT_ONE: u32 = 0x45dbc0;
const LOG_SCALE: u32 = 0x45dde8;
const ROOM_SCALE: u32 = 0x45e008;
const OCCLUSION_LF_RATIO: u32 = 0x45dc20;
// Instruction operand locations in the owner's image.
const GAIN_FLOOR: u32 = 0x221237;
const ROOM_FLOOR: u32 = 0x221357;
const CURVE_MIN: u32 = 0x2212d8;

const INTEGER_CONVERSION: u32 = 0x322b0c;
const ROOM_CURVE: u32 = 0x12aff0;
const SET_ROOM: u32 = 0x37c6e5;
const SET_FILTER: u32 = 0x37b68b;
const FILTER_FREQUENCY: u32 = 0x470084;
const FILTER_RESONANCE: u32 = 0x470088;

const DIRECT: u32 = 0x221252;
const DIRECT_HF: u32 = 0x2212a1;
const TRIM: u32 = 0x2212e9;
const ROOM: u32 = 0x221326;
const OBSTRUCTION: u32 = 0x2213b2;
const OCCLUSION: u32 = 0x221406;
const ROOM_SENT: u32 = 0x221427;
const CUTOFF: u32 = 0x22144a;
const RESONANCE: u32 = 0x22145a;
const FILTER_SENT: u32 = 0x22147c;
const RESONANCE_INTEGER: u32 = 0x21e314;

pub const BODIES: &[Body] = &[
    Body {
        point: &points::SOUND_SUBMIX_UPDATE,
        sites: &[position::POSITION_SENT, position::EFFECTS_SENT],
        run: position::update,
    },
    Body {
        point: &points::SOUND_SUBMIX_FX,
        sites: &[
            DIRECT,
            DIRECT_HF,
            TRIM,
            ROOM,
            OBSTRUCTION,
            OCCLUSION,
            ROOM_SENT,
            CUTOFF,
            RESONANCE,
            FILTER_SENT,
        ],
        run: update,
    },
    Body {
        point: &points::SOUND_SUBMIX_CUTOFF,
        sites: &[],
        run: cutoff,
    },
    Body {
        point: &points::SOUND_SUBMIX_RESONANCE,
        sites: &[RESONANCE_INTEGER],
        run: resonance,
    },
];

fn call(target: u32, site: u32) -> Next {
    Next::Call { target, site }
}

fn clamped_gain(c: &mut CpuAbi, g: &Guest) -> Result<()> {
    let floor = g.u32(GAIN_FLOOR)?;
    sub_flags(c, c.eax, floor, 32);
    if (c.eax as i32) < floor as i32 {
        c.eax = floor;
    } else {
        logic_flags(c, c.eax);
        if c.eax as i32 > 0 {
            c.eax = 0;
            logic_flags(c, 0);
        }
    }
    Ok(())
}

// Returns either the call to the guest's integer conversion or the immediate
// floor for zero gain. The latter keeps the comparison flags, so it joins
// the same descriptor store as the conversion without its integer clamp.
fn gain(c: &mut CpuAbi, g: &mut Guest, scratch: u32, site: u32) -> Result<Option<Next>> {
    let input = sse::lane0(c, 0);
    sse::ucomiss(c, input, g.u32(FLOAT_ZERO)?);
    sse::lahf(c);
    let mask = (c.eax >> 8) & 0x44;
    logic_flags(c, mask);
    set_slot(c, g, scratch, input)?;
    if !sse::parity(c) {
        c.eax = g.u32(GAIN_FLOOR)?;
        return Ok(None);
    }
    let mut fpu = load_fpu(c);
    submix::gain_level(&mut fpu.0, input, g.u32(LOG_SCALE)?);
    // The last operation before each five-byte conversion call is a
    // six-byte x87 multiply by the shared scale.
    finish_fpu(c, g, fpu, site - 11, LOG_SCALE)?;
    Ok(Some(call(INTEGER_CONVERSION, site)))
}

#[derive(Clone, Copy)]
enum Stage {
    Direct,
    DirectHf,
    Curve,
    Room,
    Obstruction,
    Occlusion,
    Submit,
}

fn update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let c = &mut *cx.cpu;
    let g = &mut *cx.g;
    let mut stage = match pc {
        DIRECT | DIRECT_HF | ROOM | OBSTRUCTION | OCCLUSION => {
            clamped_gain(c, g)?;
            match pc {
                DIRECT => Stage::DirectHf,
                DIRECT_HF => Stage::Curve,
                ROOM => Stage::Obstruction,
                OBSTRUCTION => Stage::Occlusion,
                _ => Stage::Submit,
            }
        }
        TRIM => {
            set_slot(c, g, 8, sse::lane0(c, 0))?;
            Stage::Room
        }
        ROOM_SENT => {
            let freq = sse::cvtsi2ss(c, 0, g.u32(FILTER_FREQUENCY)?);
            set_slot(c, g, 0x0c, 1)?;
            set_slot(c, g, 0x10, 0)?;
            push(c, g, freq)?;
            return Ok(call(points::SOUND_SUBMIX_CUTOFF.base, CUTOFF));
        }
        CUTOFF => {
            c.edx = g.u32(FILTER_RESONANCE)?;
            push(c, g, c.edx)?;
            set_slot(c, g, 0x18, c.eax)?;
            return Ok(call(points::SOUND_SUBMIX_RESONANCE.base, RESONANCE));
        }
        RESONANCE => {
            c.ecx = g.u32(c.edi.wrapping_add(0x34))?;
            set_slot(c, g, 0x18, c.eax)?;
            c.eax = c.esp.wrapping_add(0x0c);
            push(c, g, c.eax)?;
            push(c, g, c.ecx)?;
            set_slot(c, g, 0x24, 0)?;
            set_slot(c, g, 0x28, 0)?;
            return Ok(call(SET_FILTER, FILTER_SENT));
        }
        FILTER_SENT => {
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            add_flags(c, c.esp, 64);
            c.esp = c.esp.wrapping_add(64);
            return Ok(Next::Ret { pop: 4 });
        }
        _ => {
            c.esp = c.esp.wrapping_sub(64);
            push(c, g, c.esi)?;
            c.esi = g.u32(SOUND_GLOBALS)?;
            push(c, g, c.edi)?;
            c.edi = c
                .esi
                .wrapping_add(0x1a18)
                .wrapping_add(c.eax.wrapping_mul(0x3c));
            c.eax = 0;
            g.write(c.esp.wrapping_add(0x24), [0; 36])?;
            Stage::Direct
        }
    };

    loop {
        let (next_stage, scratch, return_site) = match stage {
            Stage::Direct => {
                sse::load(c, 0, g.u32(c.esi.wrapping_add(0x2abc))?);
                (Stage::DirectHf, 8, DIRECT)
            }
            Stage::DirectHf => {
                set_slot(c, g, 0x24, c.eax)?;
                sse::load(c, 0, g.u32(c.esi.wrapping_add(0x2ac0))?);
                (Stage::Curve, 8, DIRECT_HF)
            }
            Stage::Curve => {
                set_slot(c, g, 0x28, c.eax)?;
                c.eax = slot(c, g, 0x4c)?;
                set_slot(c, g, 0x34, 0)?;
                set_slot(c, g, 0x3c, 0)?;
                sse::load(c, 0, g.u32(OCCLUSION_LF_RATIO)?);
                set_slot(c, g, 0x44, sse::lane0(c, 0))?;
                push(c, g, c.eax)?;
                push(c, g, 0)?;
                push(c, g, g.u32(CURVE_MIN)?)?;
                c.ecx = (c.ecx & !0xff) | 1;
                return Ok(call(ROOM_CURVE, TRIM));
            }
            Stage::Room => {
                sse::load(c, 0, g.u32(c.esi.wrapping_add(0x2ac4))?);
                (Stage::Obstruction, 0x4c, ROOM)
            }
            Stage::Obstruction => {
                sse::load(c, 0, g.u32(ROOM_SCALE)?);
                sse::load(c, 1, slot(c, g, 8)?);
                sse::ss_reg(c, Ss::Mul, 1, 0);
                let attenuation = sse::ss_reg(c, Ss::Sub, 0, 1);
                c.ecx = sse::cvttss2si(c, attenuation);
                c.eax = c.eax.wrapping_sub(c.ecx);
                let floor = g.u32(ROOM_FLOOR)?;
                if (c.eax as i32) < floor as i32 {
                    c.eax = floor;
                } else {
                    c.edx = if c.eax as i32 > 0 { 0 } else { u32::MAX };
                    c.eax &= c.edx;
                }
                set_slot(c, g, 0x2c, c.eax)?;
                set_slot(c, g, 0x30, c.eax)?;
                sse::load(c, 0, g.u32(FLOAT_ONE)?);
                sse::ss(c, Ss::Sub, 0, g.u32(c.edi.wrapping_add(0x30))?);
                (Stage::Occlusion, 0x4c, OBSTRUCTION)
            }
            Stage::Occlusion => {
                set_slot(c, g, 0x38, c.eax)?;
                sse::load(c, 0, g.u32(FLOAT_ONE)?);
                sse::ss(c, Ss::Sub, 0, g.u32(c.edi.wrapping_add(0x2c))?);
                (Stage::Submit, 0x4c, OCCLUSION)
            }
            Stage::Submit => {
                c.ecx = g.u32(c.edi.wrapping_add(0x34))?;
                set_slot(c, g, 0x40, c.eax)?;
                c.eax = c.esp.wrapping_add(0x24);
                push(c, g, 1)?;
                push(c, g, c.eax)?;
                push(c, g, c.ecx)?;
                return Ok(call(SET_ROOM, ROOM_SENT));
            }
        };
        if let Some(transfer) = gain(c, g, scratch, return_site)? {
            return Ok(transfer);
        }
        stage = next_stage;
    }
}

fn cutoff(cx: &mut Cx<'_>, _: u32) -> Result<Next> {
    let c = &mut *cx.cpu;
    let g = &mut *cx.g;
    let input = slot(c, g, 4)?;
    let lower = g.u32(0x45dcfc)?;
    sse::load(c, 1, lower);
    sse::load(c, 0, input);
    sse::comiss(c, lower, input);
    if !sse::below_or_equal(c) {
        c.eax = g.u32(0x21e284)?;
    } else {
        sse::comiss(c, input, g.u32(0x45dde4)?);
        if sse::below_or_equal(c) {
            let mut fpu = load_fpu(c);
            let settings = CutoffScale {
                radians_per_hz: [g.u32(0x45dde0)?, g.u32(0x45dddc)?],
                log_scale: g.u32(0x45ddd8)?,
            };
            let result = submix::cutoff(&mut fpu.0, input, settings);
            set_slot(c, g, 4, result.value)?;
            x87::abi::store_commit(&mut fpu.0, result.status, true);
            finish_fpu(c, g, fpu, 0x21e2b9, c.esp.wrapping_add(4))?;
            sse::load(c, 0, result.value);
            c.eax = sse::cvttss2si(c, result.value) & u32::from(u16::MAX);
        } else {
            c.eax = 0;
        }
        logic_flags(c, c.eax);
    }
    Ok(Next::Ret { pop: 4 })
}

fn resonance(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let c = &mut *cx.cpu;
    let g = &mut *cx.g;
    if pc == RESONANCE_INTEGER {
        let max = u32::from(u16::MAX);
        sub_flags(c, c.eax, max, 32);
        c.eax = c.eax.min(max);
        return Ok(Next::Ret { pop: 4 });
    }
    let input = slot(c, g, 4)?;
    let upper = g.u32(0x45e004)?;
    sse::load(c, 1, input);
    sse::load(c, 0, upper);
    sse::comiss(c, input, upper);
    let value = if sse::below_or_equal(c) {
        input
    } else {
        set_slot(c, g, 4, upper)?;
        upper
    };
    let mut fpu = load_fpu(c);
    submix::resonance(&mut fpu.0, value, g.u32(0x45e000)?, g.u32(0x45dffc)?);
    finish_fpu(c, g, fpu, 0x21e309, 0x45dffc)?;
    Ok(call(INTEGER_CONVERSION, RESONANCE_INTEGER))
}
