//! Encounter-level HaloScript operations on converted Halo 1 campaign maps.
//! The scripts still run on the retail Halo 2 interpreter. See
//! docs/halo1/ai-control.md for the builtin and guest ABI evidence.

use guest_layout::DatumHandle;
use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use hooks::{BuildError, Builder, Flow, Manifest, points};
use hs::h1_ai_slots::{self, Operation};

mod actors;
mod auto_migration;
mod contact;
mod follow;
mod follow_geometry;
mod platoons;
mod playfight;
mod preference;
mod reports;
mod speech;
#[path = "h1_ai_control/squads.rs"]
mod squad_control;
mod state;
mod teleport;
mod timers;
mod uncover;
mod uncover_limits;
mod uncover_targetless;
mod vehicles;

const NONE: u32 = u32::MAX;
const AI_GROUP: u32 = 0x4000_0000;
const AI_KIND: u32 = 0xc000_0000;

const MAGICALLY_SEE_PLAYERS: u16 = h1_ai_slots::CONTROL;
const MAGICALLY_SEE_OBJECT: u16 = h1_ai_slots::SEE_OBJECT;
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

impl hooks::Feature for AiControl {
    fn manifest(&self) -> Manifest {
        Manifest::new("h1-ai-control", "Halo 1 campaign encounter controls")
            .doc("docs/halo1/ai-control.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.raw().before(
            &points::HS_DISCARDED_EVALUATE,
            "ported Halo 1 AI controls",
            |call| {
                if call.arg_u32("opcode") != Some(u32::from(MAGICALLY_SEE_PLAYERS))
                    || !is_converted_campaign(call)
                {
                    return Ok(Flow::Continue);
                }
                let Some(operation) = call_operation(call, MAGICALLY_SEE_PLAYERS) else {
                    return Ok(Flow::Continue);
                };
                call.answer_async(move |cx| Box::pin(dispatch(cx, operation)))
            },
        );
        b.raw().before(
            &points::HS_UNIT_GRENADE_COUNT,
            "count actors trying to enter a Halo 1 vehicle",
            |call| {
                if call.arg_u32("opcode") != Some(u32::from(h1_ai_slots::VEHICLE_COUNT))
                    || !is_converted_campaign(call)
                {
                    return Ok(Flow::Continue);
                }
                let Some(operation) = call_operation(call, h1_ai_slots::VEHICLE_COUNT) else {
                    return Ok(Flow::Continue);
                };
                call.answer_async(move |cx| Box::pin(dispatch(cx, operation)))
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
        b.raw().before(
            &points::AI_DAMAGE_MODIFIER,
            "attenuate playfighting actor damage",
            |call| {
                if is_converted_campaign(call) {
                    let (Some(actor), Some(multiplier)) =
                        (call.arg_u32("actor"), call.arg_u32("multiplier"))
                    else {
                        return Ok(Flow::Continue);
                    };
                    let mut guest = call.ctx.runtime().memory();
                    let _ = playfight::scale_damage(&mut *guest, actor, multiplier);
                }
                Ok(Flow::Continue)
            },
        );
        playfight::register_firing_hooks(b);
        uncover::register_playfight_hook(b);
        uncover_limits::register(b);
        uncover_targetless::register(b);
        reports::register(b);
        teleport::register_spawn_hook(b);
        b.raw().before(
            &points::AI_SUBMIT_UNIT_CONTROL,
            "hold actor movement during the Halo 1 combat timer",
            |call| {
                if is_converted_campaign(call) {
                    let actor = call.cpu.eax;
                    let mut guest = call.ctx.runtime().memory();
                    let _ = timers::hold_movement(&mut *guest, actor);
                }
                Ok(Flow::Continue)
            },
        );
        b.raw().before(
            &points::AI_UPDATE_BEHAVIORS,
            "hold Halo 1 behaviour updates during the combat timer",
            |call| {
                if is_converted_campaign(call)
                    && let Some(actor) = call.arg_u32("actor")
                {
                    let hold = {
                        let guest = call.ctx.runtime().memory();
                        actors::by_handle(&*guest, DatumHandle(actor))
                            .and_then(|actor| {
                                actor.map_or(Ok(false), |actor| timers::held(&*guest, actor))
                            })
                            .unwrap_or(false)
                    };
                    if hold {
                        return call.ret(4).map(Flow::Return);
                    }
                }
                Ok(Flow::Continue)
            },
        );
        b.raw().after(
            &points::AI_TARGET_SCORE,
            "score a Halo 1 actor's preferred target",
            |call| {
                let score = f32::from_le_bytes(call.cpu.xmm[0][..4].try_into().unwrap());
                if score == 0.0 || !score.is_finite() {
                    return Ok(());
                }
                if is_converted_campaign(call) {
                    let guest = call.ctx.runtime().memory();
                    // The callee popped both arguments. Their values remain
                    // above its return address, at the top of its old stack.
                    let added = preference::score_bonus(&*guest, call.cpu.esp).unwrap_or(0.0);
                    call.cpu.xmm[0][..4].copy_from_slice(&(score + added).to_le_bytes());
                }
                Ok(())
            },
        );
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

fn is_converted_campaign(call: &hooks::Call<'_>) -> bool {
    let mem = call.ctx.runtime().memory();
    crate::health::loaded_map_name(&*mem)
        .is_ok_and(|name| crate::health::HALO1_LEVELS.contains(&name.as_str()))
}

/// Conversational checks share the original focused-prop awareness thresholds.
pub fn dialogue_awareness(
    m: &dyn GuestMemory,
    actor: u32,
    rate: u32,
) -> Result<u16, MemFault> {
    match actors::by_handle(m, guest_layout::DatumHandle(actor))? {
        Some(actor) => Ok(contact::focused(m, actor, rate)?.level),
        None => Ok(0),
    }
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

/// Retail's fixed evaluator and return routines use these same arrays.
/// Check the salted nodes before interpreting a spare head-atom word.
fn call_operation(call: &mut hooks::Call<'_>, opcode: u16) -> Option<Operation> {
    let head = call_head(call, opcode)?;
    match call.ctx.runtime().memory().u32(head + 16).ok()? {
        0 if opcode == MAGICALLY_SEE_PLAYERS => Some(Operation::SeePlayers),
        marker => Operation::from_marker(marker).filter(|op| op.slot() == opcode),
    }
}

pub fn call_head(call: &mut hooks::Call<'_>, opcode: u16) -> Option<u32> {
    use guest_layout::{DatumHandle, datum::DatumArray};

    let thread = call.arg_u32("thread")?;
    let mem = call.ctx.runtime().memory();
    let m = &*mem;
    let threads = DatumArray::at(m.u32(0x004f_9384).ok()?);
    let thread = threads.try_get(m, DatumHandle(thread)).ok()??;
    let frame = m.u32(thread + 0x10).ok()?;
    let expression = m.u32(frame + 4).ok()?;
    let nodes = DatumArray::at(m.u32(0x004f_9394).ok()?);
    let expression = nodes.try_get(m, DatumHandle(expression)).ok()??;
    let head = m.u32(expression + 16).ok()?;
    let head = nodes.try_get(m, DatumHandle(head)).ok()??;
    if m.u16(head + 2).ok()? != opcode || m.u16(head + 4).ok()? != 2 {
        return None;
    }
    Some(head)
}

async fn arguments(
    cx: &Cx,
    opcode: u16,
    types: Option<&[u16]>,
) -> Result<Option<Vec<u32>>, EntryError> {
    let thread = cx.arg(1)?;
    let first = cx.arg(2)?;
    let (pointer, count, packed) = if let Some(types) = types {
        let words: Vec<_> = types
            .chunks(2)
            .map(|pair| u32::from(pair[0]) | (u32::from(pair.get(1).copied().unwrap_or(0)) << 16))
            .collect();
        // GuestCall places its trailing words here, above the four
        // arguments and return address. The evaluator copies results
        // into its own HS frame before returning to us.
        let pointer = cx.entry().esp.wrapping_sub(4 * words.len() as u32);
        (pointer, types.len() as u32, words)
    } else {
        let (descriptor, count) = cx.read(|m| {
            let descriptor = m.u32(BUILTIN_TABLE + 4 * u32::from(opcode))?;
            Ok::<_, MemFault>((descriptor, u32::from(m.u16(descriptor + 0xc)?)))
        })?;
        (descriptor + 0xe, count, Vec::new())
    };
    let slots = cx
        .call(
            GuestCall::new(EVAL_ARGUMENTS)
                .args([thread, count, pointer, first].into_iter().chain(packed)),
        )
        .await?
        .eax;
    if slots == 0 {
        // A child expression is pending. The interpreter resumes this
        // same call with first=0.
        return Ok(None);
    }
    let values = cx.read(|m| {
        (0..count)
            .map(|i| m.u32(slots + 4 * i))
            .collect::<Result<_, _>>()
    })?;
    Ok(Some(values))
}

async fn complete(cx: &Cx, value: u32) -> Result<Ret, EntryError> {
    let result = cx
        .call(GuestCall::new(EVAL_RETURN).eax(value).ecx(cx.arg(1)?))
        .await?;
    Ok(Ret::new(result.eax, 12))
}

async fn awareness(cx: &Cx, subject: u32, object: Option<u32>) -> Result<(), EntryError> {
    let (squads, objects) = cx.read(|m| {
        let squads = squads(m, subject)?;
        let objects = match object {
            Some(unit) => (unit != NONE).then_some(unit).into_iter().collect(),
            None => crate::coop_players::players(m)
                .into_iter()
                .map(|player| player.unit)
                .filter(|&unit| unit != NONE)
                .collect::<Vec<_>>(),
        };
        Ok::<_, MemFault>((squads, objects))
    })?;
    for object in objects {
        for &squad in &squads {
            cx.call(GuestCall::new(SEE_OBJECT).eax(squad).args([object]))
                .await?;
        }
    }
    Ok(())
}

async fn see(cx: Cx, opcode: u16) -> Result<Ret, EntryError> {
    let Some(args) = arguments(&cx, opcode, None).await? else {
        return Ok(Ret::new(0, 12));
    };
    awareness(&cx, args[0], args.get(1).copied()).await?;
    complete(&cx, 0).await
}

async fn dispatch(cx: Cx, op: Operation) -> Result<Ret, EntryError> {
    let Some(args) = arguments(&cx, op.slot(), Some(op.parameters())).await? else {
        return Ok(Ret::new(0, 12));
    };
    let mut value = 0;
    if op == Operation::SeePlayers {
        awareness(&cx, args[0], None).await?;
    } else if cx.read(actors::active)? {
        match op {
            Operation::AttachFree => actors::attach_free(&cx, args[0], args[1]).await?,
            Operation::Free => {
                for actor in cx.read(|m| actors::selected(m, args[0]))? {
                    actors::move_actor(&cx, actor, NONE).await?;
                }
            }
            Operation::FreeUnits => {
                for actor in cx.read(|m| actors::actors_of_units(m, args[0]))? {
                    actors::move_actor(&cx, actor, NONE).await?;
                }
            }
            Operation::MigrateSpeak => speech::migrate(&cx, args[0], args[1], args[2]).await?,
            Operation::MigrateUnits => {
                let moves = cx.read(|m| {
                    let mut moves = Vec::new();
                    for actor in actors::actors_of_units(m, args[0])? {
                        if let Some(destination) = actors::migrate_to(m, actor, args[1])? {
                            moves.push((actor, destination));
                        }
                    }
                    Ok::<_, MemFault>(moves)
                })?;
                for (actor, destination) in moves {
                    actors::move_actor(&cx, actor, destination).await?;
                }
            }
            Operation::Teleport | Operation::TeleportUnsupported => {
                actors::teleport(&cx, args[0], op == Operation::TeleportUnsupported).await?;
            }
            Operation::TeleportReserved | Operation::TeleportUnsupportedReserved => {
                let pools = cx.read(|m| teleport::pools(m, args[1]))?;
                actors::teleport_reserved(
                    &cx, args[0], op == Operation::TeleportUnsupportedReserved, &pools,
                ).await?;
            }
            Operation::Maneuver => actors::maneuver(&cx, args[0]).await?,
            Operation::PlatoonTick => platoons::tick(&cx, args[0]).await?,
            Operation::SetManeuver => platoons::set(&cx, args[0], args[1] as u8 != 0)?,
            Operation::PreferAi | Operation::PreferPlayer | Operation::ClearPreference => {
                preference::set(&cx, op, &args)?;
            }
            Operation::Playfight => playfight::set(&cx, args[0], args[1] as u8 != 0, args[2])?,
            Operation::TimerExpire => {
                squad_control::expire(&cx, args[0])?;
                if args[1] as u8 != 0 {
                    awareness(&cx, args[0], None).await?;
                }
            }
            Operation::TimerTick | Operation::ExpireSquadTimers => {
                let revealed = if op == Operation::TimerTick {
                    timers::tick(&cx, args[0])?
                } else {
                    timers::expire(&cx, args[0])?
                };
                for squad in revealed {
                    awareness(&cx, squad, None).await?;
                }
            }
            Operation::LinkActivation => squad_control::link(&cx, args[0], args[1], args[2])?,
            Operation::ActivationTick => squad_control::activation_tick(&cx, args[0]).await?,
            Operation::FollowPlayers
            | Operation::FollowAi
            | Operation::FollowUnit
            | Operation::FollowDisable
            | Operation::FollowDistance => {
                follow::configure(&cx, op, &args)?;
            }
            Operation::AutomaticMigrationTarget => {
                auto_migration::set(&cx, args[0], args[1] as u8 != 0, args[2])?;
            }
            Operation::FollowTick | Operation::FollowWithDestinations | Operation::FollowWithSourcePositions => {
                follow::tick(&cx, op, args[0], args.get(1).copied().unwrap_or(NONE)).await?;
            }
            Operation::VehicleTick => vehicles::tick(&cx, args[0]).await?,
            Operation::VehicleEnter => {
                vehicles::enter_if_unassigned(&cx, args[0], args[1], args[2]).await?;
            }
            Operation::VehicleCount => value = cx.read(|m| vehicles::count(m, args[0]))?,
            operation if operation.registry() == Some(hs::h1_ai_slots::Registry::Vehicles) => {
                vehicles::configure(&cx, operation, &args)?;
                if operation == Operation::VehicleEncounter {
                    vehicles::assign(&cx, args[0], args[1]).await?;
                }
            }
            _ => unreachable!(),
        }
    }
    complete(&cx, value).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::entry::{EntryFuture, Regs, testing};
    use guest_layout::testing::FakeGuest;

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
    fn free_uses_the_native_squad_unlink_for_each_selected_actor() {
        use guest_layout::datum::layout;

        const ARRAY: u32 = 0x0083_0000;
        const ROWS: u32 = 0x0084_0000;
        const USED: u32 = 0x0085_0000;
        const AI: u32 = 0x0086_0000;
        const STRIDE: u32 = 0x100;
        let mut guest = guest(MAGICALLY_SEE_PLAYERS, 1);
        guest
            .put_u32(actors::AI_GLOBALS, AI)
            .put_u8(AI + 1, 1)
            .put_u32(actors::ACTORS, ARRAY)
            .put_u32(ARRAY + layout::ELEMENT_SIZE, STRIDE)
            .put_u32(ARRAY + layout::HIGH_WATER, 3)
            .put_u32(ARRAY + layout::DATA, ROWS)
            .put_u32(ARRAY + layout::BITMAP, USED)
            .put_u32(USED, 0b111)
            .put_u16(ROWS, 0x8200)
            .put_u16(ROWS + STRIDE, 0x8201)
            .put_u16(ROWS + 2 * STRIDE, 0x8202)
            .put_u32(ROWS + 0x30, 0)
            .put_u32(ROWS + STRIDE + 0x30, 2)
            .put_u32(ROWS + 2 * STRIDE + 0x30, 1);
        let run = testing::run(
            &|cx| -> EntryFuture { Box::pin(dispatch(cx, Operation::Free)) },
            guest,
            Regs {
                esp: STACK,
                ..Default::default()
            },
            |call, _| {
                if call.target == EVAL_ARGUMENTS {
                    assert_eq!(
                        call.args,
                        [THREAD, 1, STACK - 4, 1, u32::from(hs::casts::types::AI)]
                    );
                    Regs {
                        eax: SLOTS,
                        ..Default::default()
                    }
                } else {
                    Regs::default()
                }
            },
        );
        let moves: Vec<_> = run.calls[1..3]
            .iter()
            .map(|c| (c.target, c.eax, c.args.as_slice()))
            .collect();
        assert_eq!(
            moves,
            [
                (0x001e_3400, Some(0x8200_0000), &[NONE][..]),
                (0x001e_3400, Some(0x8202_0002), &[NONE][..]),
            ]
        );
        assert_eq!(run.calls[3].target, EVAL_RETURN);
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
