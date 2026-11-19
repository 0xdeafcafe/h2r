//! Live encounter follow controls. The current native combat order supplies
//! the areas, and its runtime follow fields take a per-squad target.

use engine::{
    DatumHandle,
    entry::{Cx, EntryError, GuestCall},
    memory::{GuestMemory, MemFault},
    view::objects,
};
use hs::h1_ai_slots::{DEFAULT_FOLLOW_RADIUS, FOLLOW_WORDS, Operation, Registry};

use super::{
    AI_GROUP, GROUP_COUNT, NONE, actors, auto_migration, points, squad_control, squads, state,
};

const WORDS: u32 = Registry::Follow.words() as u32;
const WIDTH: u32 = 3;
const TARGET: u32 = state::STRIDE;
const RADIUS: u32 = state::STRIDE * 2;

const FOLLOW_NONE: u32 = 0;
const FOLLOW_PLAYERS: u32 = 1;
const FOLLOW_AI: u32 = 2;
const FOLLOW_UNIT: u32 = 3;

const BUILD_AREA_SET: u32 = 0x0020_4450;
const AREA_COUNT: u32 = 0x30;
const FOLLOW_OBJECT: u32 = 0x80;
const ORDER_INDEX: u32 = 0x2a;
const ORDER_COUNT: u32 = 0x240;
const ORDERS: u32 = 0x244;
const ORDER_STRIDE: u32 = 0x7c;
const ORDER_FLAGS: u32 = 0x24;
const ORDER_RADIUS: u32 = 0x50;
const NATIVE_FOLLOW_PLAYER: u8 = 0x10;

type R<T> = Result<T, MemFault>;

pub(super) fn configure(cx: &Cx, op: Operation, args: &[u32]) -> R<()> {
    cx.write(|m| {
        let (Some(group), Some(start)) = (
            squad_control::encounter(m, args[0])?,
            state::registry(m, args[args.len() - 1], WORDS)?,
        ) else {
            return Ok(());
        };
        let group = u32::from(group);
        if group >= u32::from(FOLLOW_WORDS) / WIDTH {
            return Ok(());
        }
        let at = start + group * WIDTH * state::STRIDE;
        if op == Operation::FollowDistance {
            let radius = f32::from_bits(args[1]);
            if radius.is_finite() {
                m.set_f32(
                    at + RADIUS,
                    if radius > 0.0 {
                        radius
                    } else {
                        DEFAULT_FOLLOW_RADIUS
                    },
                )?;
            }
            return Ok(());
        }
        let (mode, target) = match op {
            Operation::FollowDisable => (FOLLOW_NONE, NONE),
            Operation::FollowPlayers => (FOLLOW_PLAYERS, NONE),
            Operation::FollowAi => (FOLLOW_AI, args[1]),
            Operation::FollowUnit => (FOLLOW_UNIT, args[1]),
            _ => unreachable!(),
        };
        m.set_u32(at, mode)?;
        m.set_u32(at + TARGET, target)?;
        if mode == FOLLOW_NONE {
            for squad in squads(m, AI_GROUP | group)? {
                if let Some(runtime) = squad_control::runtime(m, squad)? {
                    m.set_u32(runtime + FOLLOW_OBJECT, NONE)?;
                }
            }
        }
        Ok(())
    })
}

#[derive(Clone, Copy)]
struct Target {
    unit: u32,
    position: [f32; 3],
    squad: u32,
}

fn target(m: &dyn GuestMemory, unit: u32, squad: u32) -> R<Option<Target>> {
    let Some((_, object)) = objects::get(m, DatumHandle(unit))? else {
        return Ok(None);
    };
    if object.damage_flags(m)? & 4 != 0 {
        return Ok(None);
    }
    Ok(Some(Target {
        unit,
        position: object.position(m)?,
        squad,
    }))
}

fn targets(m: &dyn GuestMemory, mode: u32, reference: u32) -> R<Vec<Target>> {
    let mut out = Vec::new();
    match mode {
        FOLLOW_PLAYERS => {
            for player in crate::coop_players::players(m) {
                out.extend(target(m, player.unit, NONE)?);
            }
        }
        FOLLOW_AI => {
            for actor in actors::selected(m, reference)? {
                out.extend(target(m, actor.unit, actor.squad)?);
            }
        }
        FOLLOW_UNIT => {
            out.extend(target(m, reference, NONE)?);
        }
        _ => {}
    }
    Ok(out)
}

fn closest(squad: u32, origin: [f32; 3], targets: &[Target]) -> Option<u32> {
    targets
        .iter()
        .filter(|t| t.squad != squad)
        .map(|t| {
            let distance: f32 = t
                .position
                .iter()
                .zip(origin)
                .map(|(a, b)| (a - b).powi(2))
                .sum();
            (distance, t.unit)
        })
        .filter(|(distance, _)| distance.is_finite())
        .min_by(|(a, _), (b, _)| a.total_cmp(b))
        .map(|(_, unit)| unit)
}

#[derive(Clone, Copy)]
struct Follow {
    squad: u32,
    runtime: u32,
    order: u32,
    order_address: u32,
    target: u32,
    radius: f32,
}

struct ActiveFollows {
    follows: Vec<Follow>,
    migrations: Vec<auto_migration::Migration>,
}

fn active_follows(m: &dyn GuestMemory, base: u32, deferred: Option<&[auto_migration::Destination]>) -> R<ActiveFollows> {
    let mut active = ActiveFollows {
        follows: Vec::new(),
        migrations: Vec::new(),
    };
    let Some(start) = state::registry(m, base, WORDS)? else {
        return Ok(active);
    };
    let scenario = m.u32(points::SCENARIO.base)?;
    let groups = m
        .u32(scenario + GROUP_COUNT)?
        .min(u32::from(FOLLOW_WORDS) / WIDTH);
    let orders = m.u32(scenario + ORDERS)?;
    let count = m.u32(scenario + ORDER_COUNT)?;
    for group in 0..groups {
        let at = start + group * WIDTH * state::STRIDE;
        let mode = m.u32(at)?;
        if mode == FOLLOW_NONE {
            continue;
        }
        let candidates = targets(m, mode, m.u32(at + TARGET)?)?;
        let radius = m.f32(at + RADIUS)?;
        let members = squads(m, AI_GROUP | group)?;
        if let Some(deferred) = deferred {
            let anchors: Vec<_> = candidates.iter().map(|t| t.position).collect();
            active.migrations.extend(auto_migration::plan(
                m, start, &members, deferred, &anchors, radius,
            )?);
        }
        for squad in members {
            let Some(runtime) = squad_control::runtime(m, squad)? else {
                continue;
            };
            let order = u32::from(m.u16(runtime + ORDER_INDEX)?);
            let target = if order < count {
                closest(squad, m.vec3(runtime + 0x18)?, &candidates).unwrap_or(NONE)
            } else {
                NONE
            };
            active.follows.push(Follow {
                squad,
                runtime,
                order,
                order_address: orders + order * ORDER_STRIDE,
                target,
                radius,
            });
        }
    }
    Ok(active)
}

pub(super) async fn tick(cx: &Cx, base: u32, encoded: u32) -> Result<(), EntryError> {
    let deferred = cx.read(|m| auto_migration::read_destinations(m, encoded))?;
    let active = cx.read(|m| active_follows(m, base, Some(&deferred)))?;
    let moves = !active.migrations.is_empty();
    for migration in active.migrations {
        auto_migration::apply(cx, migration).await?;
    }
    let follows = if moves {
        cx.read(|m| active_follows(m, base, None))?.follows
    } else {
        active.follows
    };
    for follow in follows {
        cx.write(|m| m.set_u32(follow.runtime + FOLLOW_OBJECT, follow.target))?;
        if follow.target == NONE {
            continue;
        }
        let radius = follow.order_address + ORDER_RADIUS;
        let flags = follow.order_address + ORDER_FLAGS;
        let (old_radius, old_flags) =
            cx.read(|m| Ok::<_, MemFault>((m.u32(radius)?, m.u8(flags)?)))?;
        // Use the native area collector with this invocation's radius
        // and target. Restore the current order before trying another squad.
        cx.write(|m| {
            m.set_f32(radius, follow.radius)?;
            m.set_u8(flags, old_flags & !NATIVE_FOLLOW_PLAYER)
        })?;
        let result = cx
            .call(
                GuestCall::new(BUILD_AREA_SET)
                    .eax(follow.order)
                    .args([follow.squad]),
            )
            .await;
        cx.write(|m| {
            m.set_u32(radius, old_radius)?;
            m.set_u8(flags, old_flags)?;
            if m.u8(follow.runtime + AREA_COUNT)? == 0 {
                m.set_u32(follow.runtime + FOLLOW_OBJECT, NONE)?;
            }
            Ok::<_, MemFault>(())
        })?;
        result?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ai_follow_selects_the_nearest_live_candidate_outside_its_squad() {
        let candidates = [
            Target {
                unit: 0x8000_0001,
                position: [0.0, 0.0, 0.0],
                squad: 2,
            },
            Target {
                unit: 0x8000_0002,
                position: [6.0, 0.0, 0.0],
                squad: 3,
            },
            Target {
                unit: 0x8000_0003,
                position: [3.0, 0.0, 0.0],
                squad: 4,
            },
        ];
        assert_eq!(closest(2, [0.0; 3], &candidates), Some(0x8000_0003));
    }
}
