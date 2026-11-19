//! Encounter-level HaloScript operations on converted Halo 1 campaign maps.
//! The scripts still run on the retail Halo 2 interpreter. See
//! docs/halo1/ai-control.md for the builtin and guest ABI evidence.

use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use feature::{BuildError, Builder, Flow, Manifest, points};

const NONE: u32 = u32::MAX;
const AI_GROUP: u32 = 0x4000_0000;
const AI_KIND: u32 = 0xc000_0000;

const MAGICALLY_SEE_PLAYERS: u16 = 300;
const MAGICALLY_SEE_OBJECT: u16 = 305;
const BUILTIN_TABLE: u32 = 0x0047_44e0;
const EVAL_ARGUMENTS: u32 = 0x0020_9d50;
const EVAL_RETURN: u32 = 0x0020_9ae0;
const SEE_OBJECT: u32 = 0x0027_3d30;

const GROUP_COUNT: u32 = 0x158;
const GROUPS: u32 = 0x15c;
const GROUP_SIZE: u32 = 0x24;
const GROUP_PARENT: u32 = 0x20;
const SQUAD_COUNT: u32 = 0x160;
const SQUADS: u32 = 0x164;
const SQUAD_SIZE: u32 = 0x74;
const SQUAD_GROUP: u32 = 0x26;

pub struct AiControl;

impl feature::Feature for AiControl {
    fn manifest(&self) -> Manifest {
        Manifest::new("h1-ai-control", "Halo 1 campaign encounter controls")
            .doc("docs/halo1/ai-control.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.raw().before(
            &points::HS_DISCARDED_EVALUATE,
            "AI awareness of players",
            |call| {
                if call.arg_u32("opcode") != Some(u32::from(MAGICALLY_SEE_PLAYERS))
                    || !is_converted_campaign(call)
                {
                    return Ok(Flow::Continue);
                }
                call.answer_async(|cx| Box::pin(see(cx, MAGICALLY_SEE_PLAYERS)))
            },
        );
        b.raw().before(
            &points::HS_AI_SEE_OBJECT,
            "AI encounter awareness of an object",
            |call| {
                if !is_converted_campaign(call) {
                    return Ok(Flow::Continue);
                }
                call.answer_async(|cx| Box::pin(see(cx, MAGICALLY_SEE_OBJECT)))
            },
        );
        b.snapshot(feature::SnapshotPolicy::Stateless);
        Ok(())
    }
}

fn is_converted_campaign(call: &feature::Call<'_>) -> bool {
    let mem = call.ctx.runtime().memory();
    crate::health::loaded_map_name(&*mem)
        .is_ok_and(|name| crate::health::HALO1_LEVELS.contains(&name.as_str()))
}

/// Halo 2 encodes squads as bare indices and encounter groups with the
/// top two bits equal to 01. Walk parent links for nested groups.
fn squads(m: &dyn GuestMemory, ai: u32) -> Result<Vec<u32>, MemFault> {
    let kind = ai & AI_KIND;
    if kind != 0 && kind != AI_GROUP {
        return Ok(Vec::new());
    }
    let scenario = m.u32(points::SCENARIO.base)?;
    if scenario == 0 {
        return Ok(Vec::new());
    }
    let index = u32::from(ai as u16);
    let count = m.u32(scenario + SQUAD_COUNT)?;
    if kind == 0 {
        return Ok((index < count).then_some(index).into_iter().collect());
    }
    let groups = m.u32(scenario + GROUPS)?;
    let group_count = m.u32(scenario + GROUP_COUNT)?;
    if index >= group_count {
        return Ok(Vec::new());
    }
    let rows = m.u32(scenario + SQUADS)?;
    let mut out = Vec::new();
    for squad in 0..count {
        let mut parent = u32::from(m.u16(rows + squad * SQUAD_SIZE + SQUAD_GROUP)?);
        for _ in 0..group_count {
            if parent >= group_count {
                break;
            }
            if parent == index {
                out.push(squad);
                break;
            }
            parent = u32::from(m.u16(groups + parent * GROUP_SIZE + GROUP_PARENT)?);
        }
    }
    Ok(out)
}

async fn see(cx: Cx, opcode: u16) -> Result<Ret, EntryError> {
    let thread = cx.arg(1)?;
    let first = cx.arg(2)?;
    let (descriptor, count) = cx.read(|m| {
        let descriptor = m.u32(BUILTIN_TABLE + 4 * u32::from(opcode))?;
        Ok::<_, MemFault>((descriptor, u32::from(m.u16(descriptor + 0xc)?)))
    })?;
    let slots = cx
        .call(GuestCall::new(EVAL_ARGUMENTS).args([thread, count, descriptor + 0xe, first]))
        .await?
        .eax;
    // The retail argument evaluator returns zero while a child expression
    // is pending. The interpreter resumes this same opcode with first=0.
    if slots == 0 {
        return Ok(Ret::new(0, 12));
    }
    let (squads, objects) = cx.read(|m| {
        let squads = squads(m, m.u32(slots)?)?;
        let objects = if opcode == MAGICALLY_SEE_PLAYERS {
            crate::coop_players::players(m)
                .into_iter()
                .map(|player| player.unit)
                .filter(|&unit| unit != NONE)
                .collect::<Vec<_>>()
        } else {
            let unit = m.u32(slots + 4)?;
            (unit != NONE).then_some(unit).into_iter().collect()
        };
        Ok::<_, MemFault>((squads, objects))
    })?;
    for object in objects {
        for &squad in &squads {
            cx.call(GuestCall::new(SEE_OBJECT).eax(squad).args([object]))
                .await?;
        }
    }
    let result = cx
        .call(GuestCall::new(EVAL_RETURN).eax(0).ecx(thread))
        .await?;
    Ok(Ret::new(result.eax, 12))
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::entry::{EntryFuture, Regs, testing};
    use engine::testing::FakeGuest;

    const STACK: u32 = 0x0030_0000;
    const DESCRIPTOR: u32 = 0x0031_0000;
    const SLOTS: u32 = 0x0032_0000;
    const THREAD: u32 = 0xbeef_0001;
    const SCENARIO: u32 = 0x0080_0000;
    const GROUP_ROWS: u32 = 0x0081_0000;
    const SQUAD_ROWS: u32 = 0x0082_0000;

    fn guest(opcode: u16, count: u16) -> FakeGuest {
        let mut guest = FakeGuest::new();
        guest
            .put_u32(STACK + 4, u32::from(opcode))
            .put_u32(STACK + 8, THREAD)
            .put_u32(STACK + 12, 1)
            .put_u32(BUILTIN_TABLE + u32::from(opcode) * 4, DESCRIPTOR)
            .put_u16(DESCRIPTOR + 0xc, count)
            .put_u32(points::SCENARIO.base, SCENARIO)
            .put_u32(SCENARIO + GROUP_COUNT, 3)
            .put_u32(SCENARIO + GROUPS, GROUP_ROWS)
            .put_u16(GROUP_ROWS + GROUP_PARENT, u16::MAX)
            .put_u16(GROUP_ROWS + GROUP_SIZE + GROUP_PARENT, 0)
            .put_u16(GROUP_ROWS + 2 * GROUP_SIZE + GROUP_PARENT, u16::MAX)
            .put_u32(SCENARIO + SQUAD_COUNT, 3)
            .put_u32(SCENARIO + SQUADS, SQUAD_ROWS)
            .put_u16(SQUAD_ROWS + SQUAD_GROUP, 0)
            .put_u16(SQUAD_ROWS + SQUAD_SIZE + SQUAD_GROUP, 1)
            .put_u16(SQUAD_ROWS + 2 * SQUAD_SIZE + SQUAD_GROUP, 2)
            .put_u32(SLOTS, AI_GROUP);
        guest
    }

    #[test]
    fn an_encounter_sees_each_live_player_once_per_member_squad() {
        use crate::coop_players::layout;

        const PLAYER_ARRAY: u32 = 0x0083_0000;
        const PLAYER_ROWS: u32 = 0x0084_0000;
        const PLAYER_BITS: u32 = 0x0085_0000;
        let mut guest = guest(MAGICALLY_SEE_PLAYERS, 1);
        guest
            .put_u32(points::PLAYERS.base, PLAYER_ARRAY)
            .put_u32(PLAYER_ARRAY + layout::ARRAY_DATA, PLAYER_ROWS)
            .put_u32(PLAYER_ARRAY + layout::ARRAY_LIMIT, 3)
            .put_u32(PLAYER_ARRAY + layout::ARRAY_IN_USE, PLAYER_BITS)
            .put_u32(PLAYER_BITS, 0b111)
            .put_u32(PLAYER_ROWS + layout::PLAYER_UNIT, 0x8000_0001)
            .put_u32(
                PLAYER_ROWS + layout::PLAYER_SIZE + layout::PLAYER_UNIT,
                NONE,
            )
            .put_u32(
                PLAYER_ROWS + 2 * layout::PLAYER_SIZE + layout::PLAYER_UNIT,
                0x8000_0003,
            );
        let run = testing::run(
            &|cx| -> EntryFuture { Box::pin(see(cx, MAGICALLY_SEE_PLAYERS)) },
            guest,
            Regs {
                esp: STACK,
                ..Default::default()
            },
            |call, _| match call.target {
                EVAL_ARGUMENTS => {
                    assert_eq!(call.args, [THREAD, 1, DESCRIPTOR + 0xe, 1]);
                    Regs {
                        eax: SLOTS,
                        ..Default::default()
                    }
                }
                SEE_OBJECT => Regs::default(),
                EVAL_RETURN => {
                    assert_eq!((call.eax, call.ecx), (Some(0), Some(THREAD)));
                    Regs::default()
                }
                _ => panic!("unexpected guest call"),
            },
        );
        let aware: Vec<_> = run
            .calls
            .iter()
            .filter(|c| c.target == SEE_OBJECT)
            .map(|c| (c.eax.unwrap(), c.args[0]))
            .collect();
        assert_eq!(
            aware,
            [
                (0, 0x8000_0001),
                (1, 0x8000_0001),
                (0, 0x8000_0003),
                (1, 0x8000_0003),
            ]
        );
        assert_eq!(run.result, Ok(Ret::new(0, 12)));
    }

    #[test]
    fn awaiting_a_script_argument_leaves_the_interpreter_frame_pending() {
        let run = testing::run(
            &|cx| -> EntryFuture { Box::pin(see(cx, MAGICALLY_SEE_OBJECT)) },
            guest(MAGICALLY_SEE_OBJECT, 2),
            Regs {
                esp: STACK,
                ..Default::default()
            },
            |_, _| Regs::default(),
        );
        assert_eq!(run.calls.len(), 1);
        assert_eq!(run.calls[0].target, EVAL_ARGUMENTS);
        assert_eq!(run.result, Ok(Ret::new(0, 12)));
    }
}
