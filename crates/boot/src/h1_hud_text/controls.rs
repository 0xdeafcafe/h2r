//! Script control of the health, shield, motion sensor and crosshair
//! widgets. The disc uses retired native Boolean slots for these calls.

use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use engine::points::{HS_DISCARDED_EVAL, HUD_SENSOR_DRAW, HUD_WIDGET_GATE};
use feature::{Builder, Flow};

use h1scripts::hud::CONTROL_BUILTINS;
use h1text::hud::{COMPONENT_FLASH, COMPONENT_START, CONTROL_FLAGS, SENSOR, WIDGET_CONTROLS};

use super::{flash, mission_hud};

const HS_FUNCTION_TABLE: u32 = 0x0047_44e0;
const HS_EVALUATE_ARGUMENTS: u32 = 0x0020_9d50;
const HS_RETURN: u32 = 0x0020_9ae0;

fn update_flags(flags: u32, bit: u32, parameter: bool, invert: bool) -> u32 {
    if parameter ^ invert {
        flags | bit
    } else {
        flags & !bit
    }
}

async fn eval(cx: Cx, tag: u32, slot: u16, bit: u32, invert: bool) -> Result<Ret, EntryError> {
    let thread = cx.arg(1)?;
    let initialize = cx.arg(2)?;
    let descriptor = cx.read(|mem| mem.u32(HS_FUNCTION_TABLE + u32::from(slot) * 4))?;
    let args = cx
        .call(GuestCall::new(HS_EVALUATE_ARGUMENTS).args([thread, 1, descriptor + 0xe, initialize]))
        .await?
        .eax;
    // A suspended argument resumes at the evaluator, just as for a
    // native Boolean builtin. Leave its parent frame in place.
    if args != 0 {
        cx.write(|mem| {
            let parameter = mem.u8(args)? != 0;
            let flags = mem.u32(tag + CONTROL_FLAGS)?;
            if !invert && parameter && flags & bit == 0 {
                let component = bit.trailing_zeros() / 2;
                let time = mem.u32(flash::GAME_TIME)?;
                let tick = mem.u32(time + 8)?;
                mem.set_u32(tag + COMPONENT_START + 4 * component, tick)?;
            }
            mem.set_u32(
                tag + CONTROL_FLAGS,
                update_flags(flags, bit, parameter, invert),
            )
        })?;
        cx.call(GuestCall::new(HS_RETURN).ecx(thread).eax(0))
            .await?;
    }
    Ok(Ret::new(0, 12))
}

fn role(mem: &dyn GuestMemory, tag: u32, name: u32) -> Result<u32, MemFault> {
    let count = mem.u32(tag + WIDGET_CONTROLS)?.min(4096);
    let widgets = mem.u32(tag + WIDGET_CONTROLS + 4)?;
    for i in 0..count {
        let entry = widgets + 8 * i;
        if mem.u32(entry)? == name {
            return mem.u32(entry + 4);
        }
    }
    Ok(0)
}

fn suppress(mem: &dyn GuestMemory, tag: u32, flags: u32, role: u32) -> Result<bool, MemFault> {
    if flags & role != 0 {
        return Ok(true);
    }
    if flags & (role << 1) == 0 {
        return Ok(false);
    }
    let component = role.trailing_zeros() / 2;
    let start = mem.u32(tag + COMPONENT_START + 4 * component)?;
    let mut record = [0; 0x18];
    mem.read(tag + COMPONENT_FLASH + component * 0x18, &mut record)?;
    // The replacement bitmap has its tint baked in. Pulse its visibility
    // near the peak of the source flash curve.
    Ok(flash::record_strength(&record, flash::elapsed(mem, start)?) >= 0.75)
}

fn widget_hidden(mem: &dyn GuestMemory, state: u32) -> Result<bool, MemFault> {
    let Some(tag) = mission_hud(mem)? else {
        return Ok(false);
    };
    let flags = mem.u32(tag + CONTROL_FLAGS)?;
    if flags == 0 {
        return Ok(false);
    }
    let role = role(mem, tag, mem.u32(state - 8)?)?;
    suppress(mem, tag, flags, role)
}

fn sensor_hidden(mem: &dyn GuestMemory) -> Result<bool, MemFault> {
    let Some(tag) = mission_hud(mem)? else {
        return Ok(false);
    };
    suppress(mem, tag, mem.u32(tag + CONTROL_FLAGS)?, SENSOR)
}

pub(super) fn register(b: &mut Builder) {
    b.raw().before(
        &HS_DISCARDED_EVAL,
        "evaluate the scripted HUD controls",
        |c| {
            let sp = c.cpu.esp;
            let (tag, slot) = {
                let mem = c.ctx.memory();
                let Some(slot) = mem.u16(sp + 4).ok() else {
                    return Ok(Flow::Continue);
                };
                (mission_hud(&*mem).ok().flatten(), slot)
            };
            let Some(tag) = tag else {
                return Ok(Flow::Continue);
            };
            let Some(&(_, _, bit, invert)) = CONTROL_BUILTINS.iter().find(|entry| entry.1 == slot)
            else {
                return Ok(Flow::Continue);
            };
            c.answer_async(move |cx| Box::pin(eval(cx, tag, slot, bit, invert)))
        },
    );
    b.raw()
        .before(
            &HUD_WIDGET_GATE,
            "apply the scripted widget visibility",
            |c| {
                let state = c.cpu.eax;
                let hidden = {
                    let mem = c.ctx.memory();
                    widget_hidden(&*mem, state).unwrap_or(false)
                };
                if hidden {
                    c.cpu.eax = 0;
                    return c.ret(12).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
    b.raw()
        .before(
            &HUD_SENSOR_DRAW,
            "apply the scripted sensor visibility",
            |c| {
                let hidden = {
                    let mem = c.ctx.memory();
                    sensor_hidden(&*mem).unwrap_or(false)
                };
                if hidden {
                    return c.ret(0).map(Flow::Return);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_and_blink_bits_change_independently() {
        let flags = update_flags(0, 1, false, true);
        assert_eq!(flags, 1);
        let flags = update_flags(flags, 2, true, false);
        assert_eq!(flags, 3);
        let flags = update_flags(flags, 1, true, true);
        assert_eq!(flags, 2);
        assert_eq!(update_flags(flags, 2, false, false), 0);
    }
}
