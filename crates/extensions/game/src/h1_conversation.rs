//! Native services used by converted Halo 1 conversation scripts.

use guest_layout::datum::DatumArray;
use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::tags::{self, GroupTag};
use hooks::{BuildError, Builder, Flow, Manifest, points};
use hs::h1_conversation::{AI, BUILTINS, LONG, NONE, OBJECT, Op, REAL, SHORT, SOUND};
use runtime::guest::Permissions;

mod automatic;
mod cast;
mod combat;
pub mod dialogue;
mod leaders;
mod physical;
mod postcombat;
mod reactions;
mod replies;
mod search;
mod tactics;
mod visibility;
mod voices;

const BUILTIN_TABLE: u32 = 0x0047_44e0;
const EVAL_ARGUMENTS: u32 = 0x0020_9d50;
const EVAL_RETURN: u32 = 0x0020_9ae0;

const GLOBAL_VALUES: u32 = 0x004f_9380;
const SCENARIO_GLOBAL_OFFSET: u32 = 0x41d;
const GLOBAL_CAPACITY: u32 = hs::MAX_DEFINITIONS as u32;

const SOUND_INSTANCES: u32 = 0x004e_637c;
const SPATIAL_START: u32 = 0x0018_91d0;
const RADIO_START: u32 = 0x0018_9fe0;
const STOP_INSTANCE: u32 = 0x0012_6360;
const BIPED_LIP_SYNC: u32 = 0x0010_9220;
const HEAD_MARKER: u32 = 0x0400_0095;

pub struct Conversations;

impl hooks::Feature for Conversations {
    fn manifest(&self) -> Manifest {
        Manifest::new("h1-conversations", "Converted campaign conversations")
            .doc("docs/halo1/ai.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let data = b.guest_alloc(0x100, Permissions::RW)?;
        let pending = b.guest_alloc(voices::BOOTSTRAP_BYTES, Permissions::RW)?;
        leaders::install(b, pending);
        dialogue::install(b, pending);
        let reactions = reactions::install(b)?;
        automatic::install(b, reactions);
        combat::install(b, reactions)?;
        tactics::install(b, reactions)?;
        postcombat::install(b, reactions)?;
        let physical = physical::install(b, reactions)?;
        search::install(b, reactions, physical)?;
        let opcode = u32::from(BUILTINS[0].1);
        b.raw().before(
            &points::HS_UNUSED_EVALUATOR,
            "converted conversation operations",
            move |call| {
                if call.arg_u32("index") != Some(opcode) {
                    return Ok(Flow::Continue);
                }
                let converted = {
                    let mem = call.ctx.runtime().memory();
                    crate::health::loaded_map_name(&*mem)
                        .is_ok_and(|name| crate::health::HALO1_LEVELS.contains(&name.as_str()))
                };
                if !converted {
                    return Ok(Flow::Continue);
                }
                call.answer_async(move |cx| Box::pin(evaluate(cx, data, opcode)))
            },
        ).first();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// A signed 16-bit read (`GuestMemory` has only the unsigned one).
fn read_i16(m: &dyn GuestMemory, addr: u32) -> Result<i16, MemFault> {
    m.u16(addr).map(|v| v as i16)
}

fn global(m: &dyn GuestMemory, index: u32, ty: u16) -> Result<Option<u32>, MemFault> {
    if index >= GLOBAL_CAPACITY {
        return Ok(None);
    }
    let scenario = m.u32(points::SCENARIO.base)?;
    if scenario == 0 || index >= m.u32(scenario + 0x1c0)? {
        return Ok(None);
    }
    let definitions = m.u32(scenario + 0x1c4)?;
    if definitions == 0 || m.u16(definitions + index * 0x28 + 0x20)? != ty {
        return Ok(None);
    }
    let values = m.u32(GLOBAL_VALUES)?;
    if values == 0 {
        return Ok(None);
    }
    let row = DatumArray::at(values).get_absolute(m, (index + SCENARIO_GLOBAL_OFFSET) as i32)?;
    Ok(row.map(|row| row + 4))
}

fn datum(m: &dyn GuestMemory, pool: u32, handle: u32) -> Result<Option<u32>, MemFault> {
    if handle == NONE {
        return Ok(None);
    }
    let array = m.u32(pool)?;
    if array == 0 {
        return Ok(None);
    }
    DatumArray::at(array).try_get(m, DatumHandle(handle))
}

fn object_address(m: &dyn GuestMemory, handle: u32) -> Result<Option<u32>, MemFault> {
    match datum(m, points::OBJECTS.base, handle)? {
        Some(row) => Ok(Some(m.u32(row + 8)?).filter(|&address| address != 0)),
        None => Ok(None),
    }
}

fn sound_live(m: &dyn GuestMemory, instance: u32) -> Result<bool, MemFault> {
    match datum(m, SOUND_INSTANCES, instance)? {
        Some(row) => Ok(m.u8(row + 2)? == 0),
        None => Ok(false),
    }
}

async fn evaluate(cx: Cx, data: u32, opcode: u32) -> Result<Ret, EntryError> {
    let thread = cx.arg(1)?;
    let first = cx.arg(2)?;
    let descriptor = cx.read(|m| m.u32(BUILTIN_TABLE + 4 * opcode))?;
    let slots = cx
        .call(GuestCall::new(EVAL_ARGUMENTS).args([thread, 4, descriptor + 0xe, first]))
        .await?
        .eax;
    if slots == 0 {
        return Ok(Ret::new(0, 12));
    }
    if let Some((op, a, b, c)) = cx.read(|m| {
        let Some(op) = Op::decode(m.u32(slots)?) else {
            return Ok::<_, MemFault>(None);
        };
        Ok(Some((
            op,
            m.u32(slots + 4)?,
            m.u32(slots + 8)?,
            m.u32(slots + 12)?,
        )))
    })? {
        match op {
            Op::StartSound => start_sound(&cx, data, a, b, c).await?,
            Op::StopSound | Op::PollSound => {
                finish_sound(&cx, a, matches!(op, Op::StopSound)).await?
            }
            Op::CancelCommand => cast::cancel_command(&cx, a, b).await?,
            Op::Score => cast::score(&cx, data, a, b, c).await?,
            _ => cx.write(|m| cast::operate(m, op, a, b, c))?,
        }
    }
    let result = cx
        .call(GuestCall::new(EVAL_RETURN).eax(0).ecx(thread))
        .await?;
    Ok(Ret::new(result.eax, 12))
}

async fn start_sound(cx: &Cx, scratch: u32, a: u32, b: u32, out: u32) -> Result<(), EntryError> {
    let input = cx.read(|m| {
        let (Some(sound), Some(out)) = (global(m, a, SOUND)?, global(m, out, LONG)?) else {
            return Ok::<_, MemFault>(None);
        };
        let sound = m.u32(sound)?;
        let object = if b == NONE {
            NONE
        } else if let Some(object) = global(m, b, OBJECT)? {
            m.u32(object)?
        } else {
            return Ok(None);
        };
        let valid = sound != NONE
            && tags::tag_get_checked(m, DatumHandle(sound), GroupTag::from_chars(*b"snd!"))?
                .is_some_and(|address| address != 0)
            && (b == NONE || object_address(m, object)?.is_some());
        Ok(Some((sound, object, out, valid)))
    })?;
    let Some((sound, object, out, valid)) = input else {
        return Ok(());
    };
    let instance = if !valid {
        NONE
    } else if object == NONE {
        let mut record = [0u8; 0x58];
        record[..4].copy_from_slice(&0x400u32.to_le_bytes());
        record[4..8].copy_from_slice(&1.0f32.to_le_bytes());
        record[0x44..0x4c].fill(0xff);
        cx.write(|m| m.write(scratch, &record))?;
        cx.call(GuestCall::new(RADIO_START).eax(scratch).args([sound]))
            .await?
            .eax
    } else {
        play_spatial(cx, scratch, object, sound).await?
    };
    cx.write(|m| m.set_u32(out, instance))?;
    Ok(())
}

/// Plays a line from the unit's head and gives the same sound instance to
/// native lip sync. The caller supplies its own 16-byte sound parameters.
pub async fn play_spatial(
    cx: &Cx,
    scratch: u32,
    object: u32,
    sound: u32,
) -> Result<u32, EntryError> {
    cx.write(|m| {
        m.set_u32(scratch, NONE)?;
        m.set_u32(scratch + 4, sound)?;
        m.set_u32(scratch + 8, 1.0f32.to_bits())?;
        m.set_u32(scratch + 12, 0)
    })?;
    let handle = cx
        .call(
            GuestCall::new(SPATIAL_START)
                .esi(object)
                .eax(HEAD_MARKER)
                .args([scratch]),
        )
        .await?
        .eax;
    if handle != NONE {
        cx.call(
            GuestCall::new(BIPED_LIP_SYNC)
                .ebx(object)
                .args([sound, handle]),
        )
        .await?;
    }
    Ok(handle)
}

async fn finish_sound(cx: &Cx, global_index: u32, stop: bool) -> Result<(), EntryError> {
    let live = cx.write(|m| {
        let Some(address) = global(m, global_index, LONG)? else {
            return Ok::<_, MemFault>(None);
        };
        let handle = m.u32(address)?;
        let live = sound_live(m, handle)?;
        if stop || !live {
            m.set_u32(address, NONE)?;
        }
        Ok((stop && live).then_some(handle))
    })?;
    if let Some(handle) = live {
        cx.call(GuestCall::new(STOP_INSTANCE).ebx(handle)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
