//! Halo 1 HaloScript operations without an equivalent retail evaluator.
//! The converter and runtime share the descriptor assignments in
//! `h1_contract::unit_builtins`. See docs/halo1/script-builtins.md.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU16, Ordering::Relaxed},
};

use guest_layout::{
    entry::{Cx, EntryResult, GuestCall, Ret},
    memory::{GuestMemory, MemFault},
    points,
};
use engine::hs_actions::{LATCHED as ACTION_TEST_FLAGS, PLAYER_CONTROL};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy};
use h1_contract::unit_builtins::{RUNTIME_SLOTS, RuntimeBuiltin, RuntimeSlot};

mod action;
mod countdown;
mod flashlight;

use countdown::Countdown;

const FUNCTION_TABLE: u32 = 0x0047_44e0;
const EVALUATE_ARGUMENTS: u32 = 0x0020_9d50;
const RETURN_FROM_EXPRESSION: u32 = 0x0020_9ae0;

const CINEMATIC_GLOBALS: u32 = engine::colour_pass::global::OVERRIDE_GATE;
const CINEMATIC_ACTIVE: u32 = 5;
const LOOK_DIRECTIONS: u32 = 0x3c00;
const MOVE_DIRECTIONS: u32 = 0x3c000;
const PLAYER0_THUMBSTICK_LAYOUT: u32 = 0x0051_ea30;
const SIM_MODE: u32 = 8;
const MAX_SKIP: u16 = coop::cinematic::CinematicTicks::MAX as u16;
const LOCAL_FRAME_UPDATE_RETURN: u32 = 0x0012_b646;

#[derive(Default)]
pub struct ScriptBuiltins {
    state: Arc<State>,
}

#[derive(Default)]
struct State {
    skip_ticks: AtomicU16,
    countdown: Mutex<Countdown>,
}

fn converted_campaign(m: &dyn GuestMemory) -> bool {
    crate::health::loaded_map_name(m)
        .is_ok_and(|name| crate::health::HALO1_LEVELS.contains(&name.as_str()))
}

fn cinematic_active(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    let globals = m.u32(CINEMATIC_GLOBALS)?;
    Ok(globals != 0 && m.u8(globals + CINEMATIC_ACTIVE)? != 0)
}

fn authors_ticks(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    let sim = m.u32(points::SIM_GLOBALS.base)?;
    Ok(sim != 0 && coop::cinematic::CinematicTicks::authors_updates(m.u32(sim + SIM_MODE)?))
}

fn authoritative_cinematic(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    Ok(authors_ticks(m)? && cinematic_active(m)?)
}

fn read_time(m: &dyn GuestMemory, offset: u32) -> Result<u32, MemFault> {
    let clock = m.u32(points::GAME_TIME.base)?;
    if clock == 0 {
        return Ok(0);
    }
    m.u32(clock + offset)
}

fn write_speed(m: &mut dyn GuestMemory, bits: u32) -> Result<(), MemFault> {
    let clock = m.u32(points::GAME_TIME.base)?;
    if clock != 0 {
        m.set_u32(clock + crate::cheats::TIME_SPEED, bits)?;
    }
    Ok(())
}

fn all_directions(m: &dyn GuestMemory, mask: u32) -> Result<u32, MemFault> {
    let control = m.u32(PLAYER_CONTROL)?;
    Ok(u32::from(control != 0 && m.u32(control + ACTION_TEST_FLAGS)? & mask == mask))
}

async fn evaluate(cx: Cx, slot: &'static RuntimeSlot, state: Arc<State>) -> EntryResult {
    let thread = cx.arg(1)?;
    let result = if slot.parameters.is_empty() {
        match slot.builtin {
            RuntimeBuiltin::ResetLightCaches => {
                cx.write(crate::h1_cinematics::reset_light_caches)?;
                0
            }
            RuntimeBuiltin::GameTime => cx.read(|m| read_time(m, 8))?,
            RuntimeBuiltin::JoystickNormal => {
                u32::from(cx.read(|m| m.u16(PLAYER0_THUMBSTICK_LAYOUT))? < 2)
            }
            RuntimeBuiltin::LookAllDirections => cx.read(|m| all_directions(m, LOOK_DIRECTIONS))?,
            RuntimeBuiltin::MoveAllDirections => cx.read(|m| all_directions(m, MOVE_DIRECTIONS))?,
            RuntimeBuiltin::CountdownStop | RuntimeBuiltin::CountdownRestart => {
                state.countdown.lock().unwrap_or_else(|e| e.into_inner()).running =
                    slot.builtin == RuntimeBuiltin::CountdownRestart;
                0
            }
            _ => unreachable!("fixed argument slots declare their signatures"),
        }
    } else {
        let descriptor = cx.read(|m| m.u32(FUNCTION_TABLE + 4 * u32::from(slot.index)))?;
        let first = cx.arg(2)?;
        let args = cx
            .call(GuestCall::new(EVALUATE_ARGUMENTS).args([
                thread,
                slot.parameters.len() as u32,
                descriptor + 14,
                first,
            ]))
            .await?
            .eax;
        // The retail helper pushes a child frame while evaluating an
        // argument. Re-enter through the scheduler just as retail does.
        if args == 0 {
            return Ok(Ret::new(0, 12));
        }
        let value = cx.read(|m| m.u32(args))?;
        match slot.builtin {
            RuntimeBuiltin::GameSpeed => {
                cx.write(|m| write_speed(m, value))?;
                0
            }
            RuntimeBuiltin::GameSkipTicks => {
                let count = value as i16;
                if count <= MAX_SKIP as i16 {
                    // A synchronous follower can execute this script during catch-up.
                    // Its host's sequenced 0x27 updates already include those ticks.
                    let pending = if cx.read(authors_ticks)? { count.max(0) as u16 } else { 0 };
                    state.skip_ticks.store(pending, Relaxed);
                }
                0
            }
            RuntimeBuiltin::CountdownSet => {
                let (duration, start) = cx.read(|m| {
                    Ok::<_, MemFault>((m.i32(args + 4)?, m.u8(args + 8)? != 0))
                })?;
                let mut timer = state.countdown.lock().unwrap_or_else(|e| e.into_inner());
                timer.remaining = duration;
                timer.running = start;
                0
            }
            RuntimeBuiltin::CountdownGet => {
                let timer = state.countdown.lock().unwrap_or_else(|e| e.into_inner());
                i32::from(timer.digit(value as i16)) as u32
            }
            RuntimeBuiltin::FlashlightSet | RuntimeBuiltin::FlashlightSetList => {
                let on = cx.read(|m| m.u8(args + 4))? != 0;
                if slot.builtin == RuntimeBuiltin::FlashlightSet {
                    cx.write(|m| flashlight::set(m, value, on))?;
                } else {
                    cx.write(|m| flashlight::set_list(m, value, on))?;
                }
                0
            }
            RuntimeBuiltin::FlashlightGet => u32::from(cx.read(|m| flashlight::is_on(m, value))?),
            _ => unreachable!("query slots have no arguments"),
        }
    };
    cx.call(GuestCall::new(RETURN_FROM_EXPRESSION).ecx(thread).eax(result))
        .await?;
    Ok(Ret::new(0, 12))
}

/// Halo 1 dispatches its extra ticks after the regular simulation batch
/// and before drawing the frame (0x0bd6f6/0x0bd3b0). Reuse Halo 2's batch
/// loop so every extra tick runs the normal script and animation phases.
/// The loop may shorten elapsed time when a level switch interrupts it.
async fn skip_in_frame(cx: Cx, count: u16) -> EntryResult {
    let previous_seconds = f32::from_bits(cx.arg(0)?);
    let seconds_per_tick = f32::from_bits(cx.read(|m| read_time(m, crate::cheats::TIME_TICK_SECONDS))?);
    let elapsed = cx.entry().esp + 4;
    cx.write(|m| m.set_f32(elapsed, f32::from(count) * seconds_per_tick))?;
    cx.call(
        GuestCall::new(points::GAME_UPDATE_TICKS.base)
            .eax(u32::from(count))
            .ebx(elapsed),
    )
    .await?;
    let skipped_seconds = cx.read(|m| m.f32(elapsed))?;
    cx.call(
        GuestCall::new(points::GAME_UPDATE_FRAME.base)
            .args([(previous_seconds + skipped_seconds).to_bits()]),
    )
    .await?;
    Ok(Ret::new(0, 4))
}

fn calls_flashlight_list(call: &mut hooks::Call<'_>, slot: &RuntimeSlot) -> Option<bool> {
    let head = crate::h1_ai_control::call_head(call, slot.index)?;
    let mem = call.ctx.runtime().memory();
    let scnr = mem.u32(points::SCENARIO.base).ok()?;
    let table = scnr.checked_add(h1_contract::script_fields::STRINGS)?;
    let len = mem.u32(table).ok()?;
    let offset = mem.u32(head + 12).ok()?;
    if len.checked_sub(offset)? <= slot.name.len() as u32 {
        return Some(false);
    }
    let base = mem.u32(table + 4).ok()?;
    let name = mem.cstr(base.checked_add(offset)?, slot.name.len()).ok()?;
    Some(name == slot.name.as_bytes())
}

impl Feature for ScriptBuiltins {
    fn manifest(&self) -> Manifest {
        Manifest::new("h1-script-builtins", "Extra Halo 1 campaign script operations")
            .doc("docs/halo1/script-builtins.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        action::install(b);
        flashlight::install(b);
        crate::h1_cinematics::hooks(b);
        for point in [
            &points::HS_UNUSED_EVALUATOR,
            &points::HS_UNIT_FLAG_SET,
            &points::HS_UNIT_FLAG_GET,
            &points::HS_AI_LIST_FLAG_SET,
            &points::HS_SHORT_TO_LONG_EVALUATOR,
            &points::HS_LOOK_ANY_DIRECTION,
            &points::HS_MOVE_ANY_DIRECTION,
            &points::HS_FALSE_EVALUATOR,
            &points::HS_CONSTANT_LONG_EVALUATOR,
        ] {
            let state = Arc::clone(&self.state);
            b.raw().before(point, "Halo 1 script builtin", move |call| {
                let Some(index) = call.arg_u32("index") else {
                    return Ok(Flow::Continue);
                };
                let Some(slot) = RUNTIME_SLOTS.iter().find(|slot| {
                    slot.index == index as u16 && slot.evaluate == point.base
                }) else {
                    return Ok(Flow::Continue);
                };
                if !converted_campaign(&*call.ctx.runtime().memory()) {
                    return Ok(Flow::Continue);
                }
                // Slot 312 also serves native ai_braindead_by_unit calls.
                if slot.builtin == RuntimeBuiltin::FlashlightSetList
                    && calls_flashlight_list(call, slot) != Some(true)
                {
                    return Ok(Flow::Continue);
                }
                let state = Arc::clone(&state);
                call.answer_async(move |cx| Box::pin(evaluate(cx, slot, state)))
            });
        }

        let state = Arc::clone(&self.state);
        b.raw().before(&points::GAME_UPDATE_FRAME, "Run queued cinematic ticks", move |call| {
            if state.skip_ticks.load(Relaxed) == 0 {
                return Ok(Flow::Continue);
            }
            let count = {
                let mem = call.ctx.runtime().memory();
                // The continuation inside skip_in_frame calls the original
                // function again with its own return token.
                let mut ret = [0; 4];
                mem.read_bytes(call.cpu.esp, &mut ret, 1)?;
                if u32::from_le_bytes(ret) != LOCAL_FRAME_UPDATE_RETURN {
                    return Ok(Flow::Continue);
                }
                let count = state.skip_ticks.swap(0, Relaxed);
                if count == 0 || !authoritative_cinematic(&*mem).unwrap_or(false) {
                    return Ok(Flow::Continue);
                }
                count
            };
            call.answer_async(move |cx| Box::pin(skip_in_frame(cx, count)))
        });
        let state = Arc::clone(&self.state);
        b.raw().before(&points::OBJECT_FUNCTION_VALUE, "Draw a countdown digit", move |call| {
            let Some(id) = call.arg_u32("name") else { return Ok(Flow::Continue) };
            if (id >> 24) as usize != h1_contract::shader::COUNTDOWN_DIGITS[0].len() {
                return Ok(Flow::Continue);
            }
            let value = state.countdown.lock().unwrap_or_else(|e| e.into_inner()).shader_value(id);
            let (Some(value), Some(out)) = (value, call.arg_u32("value")) else {
                return Ok(Flow::Continue);
            };
            call.ctx.runtime().memory().write_bytes(out, &value.to_le_bytes(), 1)?;
            call.cpu.eax = 1;
            call.ret(12).map(Flow::Return)
        });
        let state = Arc::clone(&self.state);
        b.frame().on_tick("Update the Halo 1 numeric countdown", move |o| {
            if let Ok(tick) = read_time(&*o.guest(), 8) {
                state.countdown.lock().unwrap_or_else(|e| e.into_inner()).tick(tick);
            }
        });
        let state = Arc::clone(&self.state);
        b.game().on_start("Reset Halo 1 script state", move |o| {
            state.skip_ticks.store(0, Relaxed);
            let name = crate::health::loaded_map_name(&*o.guest()).unwrap_or_default();
            let mut timer = state.countdown.lock().unwrap_or_else(|e| e.into_inner());
            *timer = Countdown::default();
            timer.load_ids(&name);
        });
        let save = Arc::clone(&self.state);
        let load = Arc::clone(&self.state);
        b.snapshot(SnapshotPolicy::section(
            move || {
                let mut bytes = save.skip_ticks.load(Relaxed).to_le_bytes().to_vec();
                let timer = save.countdown.lock().unwrap_or_else(|e| e.into_inner());
                bytes.extend_from_slice(&timer.save());
                bytes
            },
            move |bytes, rt| {
                let [low, high, countdown @ ..] = bytes else {
                    return Err("invalid Halo 1 script state".into());
                };
                let count = u16::from_le_bytes([*low, *high]);
                if count > MAX_SKIP {
                    return Err("cinematic tick request exceeds 15".into());
                }
                let name = crate::health::loaded_map_name(&*rt.memory()).unwrap_or_default();
                let mut timer = load.countdown.lock().unwrap_or_else(|e| e.into_inner());
                timer.load(countdown)?;
                timer.load_ids(&name);
                load.skip_ticks.store(count, Relaxed);
                Ok(())
            },
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests;
