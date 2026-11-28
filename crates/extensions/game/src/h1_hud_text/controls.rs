//! Script control of the health, shield, motion sensor and crosshair
//! widgets. The disc uses retired native Boolean slots for these calls.

use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points::{HS_UNUSED_EVALUATOR, HUD_SENSOR_DRAW};
use hooks::{Builder, Flow};

use h1_contract::hud::{
    COMPONENT_START, CONTROL_FLAGS, SENSOR, WIDGET_CONTROL_SIZE, WIDGET_CONTROLS,
};
use h1_contract::hud_builtins::{CONTROL_BUILTINS, SCENARIO_HELP_BUILTIN};

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
    eval_argument(cx, slot, move |mem, args| {
        let parameter = mem.u8(args)? != 0;
        let flags = mem.u32(tag + CONTROL_FLAGS)?;
        if !invert && parameter && flags & bit == 0 {
            let component = bit.trailing_zeros() / 2;
            let time = mem.u32(flash::GAME_TIME)?;
            let tick = mem.u32(time + 8)?;
            mem.set_u32(tag + COMPONENT_START + 4 * component, tick)?;
        }
        mem.set_u32(tag + CONTROL_FLAGS, update_flags(flags, bit, parameter, invert))
    }).await
}

async fn eval_argument(
    cx: Cx,
    slot: u16,
    apply: impl FnOnce(&mut dyn GuestMemory, u32) -> Result<(), MemFault> + Send,
) -> Result<Ret, EntryError> {
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
        cx.write(|mem| apply(mem, args))?;
        cx.call(GuestCall::new(HS_RETURN).ecx(thread).eax(0))
            .await?;
    }
    Ok(Ret::new(0, 12))
}

pub(super) fn widget_control(
    mem: &dyn GuestMemory,
    tag: u32,
    name: u32,
) -> Result<Option<u32>, MemFault> {
    let count = mem.u32(tag + WIDGET_CONTROLS)?.min(4096);
    let widgets = mem.u32(tag + WIDGET_CONTROLS + 4)?;
    let stride = if super::extended(mem, tag)? { WIDGET_CONTROL_SIZE } else { 8 };
    for i in 0..count {
        let entry = widgets + stride * i;
        if mem.u32(entry)? == name {
            return Ok(Some(entry));
        }
    }
    Ok(None)
}

fn widget_hidden(mem: &dyn GuestMemory, state: u32) -> Result<bool, MemFault> {
    let Some(tag) = mission_hud(mem)? else {
        return Ok(false);
    };
    let flags = mem.u32(tag + CONTROL_FLAGS)?;
    if flags == 0 {
        return Ok(false);
    }
    let Some(entry) = widget_control(mem, tag, mem.u32(state - 8)?)? else {
        return Ok(false);
    };
    Ok(flags & mem.u32(entry + 4)? != 0)
}

fn sensor_hidden(mem: &dyn GuestMemory) -> Result<bool, MemFault> {
    let Some(tag) = mission_hud(mem)? else {
        return Ok(false);
    };
    Ok(mem.u32(tag + CONTROL_FLAGS)? & SENSOR != 0)
}

/// The scripted widget visibility at the Rust walk's seam.
#[cfg(feature = "desktop-runtime")]
struct ControlsSeam;

#[cfg(feature = "desktop-runtime")]
impl render_bridge::native_hud::seam::Seam for ControlsSeam {
    fn gate(
        &self,
        mem: &dyn GuestMemory,
        at: &render_bridge::native_hud::seam::At,
    ) -> Result<Option<bool>, MemFault> {
        // The widget's state (`+8`) is the guest gate's argument.
        Ok(widget_hidden(mem, at.widget + 8).unwrap_or(false).then_some(false))
    }
}

pub(super) fn register(b: &mut Builder) {
    // The scripted widget visibility is the Rust walk's gate answer
    // (docs/graphics/native-hud-overlay.md, "The override seam").
    #[cfg(feature = "desktop-runtime")]
    render_bridge::native_hud::seam::register(0, "h1-controls", std::sync::Arc::new(ControlsSeam));
    b.raw().before(
        &HS_UNUSED_EVALUATOR,
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
            if slot == SCENARIO_HELP_BUILTIN.1 {
                return c.answer_async(move |cx| {
                    Box::pin(eval_argument(cx, slot, move |mem, args| {
                        let index = mem.u16(args)?;
                        super::show_scenario_help(mem, tag, index)
                    }))
                });
            }
            let Some(&(_, _, bit, invert)) = CONTROL_BUILTINS.iter().find(|entry| entry.1 == slot)
            else {
                return Ok(Flow::Continue);
            };
            c.answer_async(move |cx| Box::pin(eval(cx, tag, slot, bit, invert)))
        },
    ).last();
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
