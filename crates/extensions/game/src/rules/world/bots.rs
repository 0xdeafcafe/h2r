//! Bot orders (docs/game/rules-world-api.md, "Bots"): what the script
//! calls `bot_go`, `bot_hold`, `bot_follow`, `bot_attack` and `bot_free`
//! do on the host.
//!
//! An order is a hint to a phantom's brain on the host
//! ([`bots::brain::Order`]): it returns nothing to the script, it touches
//! no guest memory and no `RulesExt` byte, and its effect travels as the
//! phantom's pad, which every session kind already carries as a player's
//! input. [`plan`] turns a call into the order; the runtime queues it for
//! the next tick boundary like any call, and [`deliver`] makes it. A
//! machine that is not the authority makes none (its runtime makes no
//! calls), and a run playing a film back makes none (the seat's recorded
//! pad plays; `rules::runtime`'s film gate).

use ::bots::brain::{Order, Vec3};
use ::bots::objective::script::{Aim, ScriptGoal};
use ::rules::caps::id;
use ::rules::vm::{Arg, Call as VmCall};
use guest_layout::GuestMemory;
use forge_doc::BlockKind;

use crate::rules::census::Table;

/// A hold's reach at its shortest: the defender's keep
/// (`bots::objective::DEFEND_RADIUS`). A zone of spread members reaches
/// its farthest member instead.
pub const HOLD_RADIUS: f32 = ::bots::objective::DEFEND_RADIUS;

/// What a bot-order call does: the seat's player slot and the order, or
/// `None` to free it. A call through `none`, or to a labelled thing the
/// map has not, makes nothing, as a write through `none` does.
pub fn plan(m: &dyn GuestMemory, table: Option<&Table>, call: &VmCall) -> Option<(u8, Option<Order>)> {
    let player = match call.args.first() {
        Some(Arg::Player(Some(p))) => *p,
        _ => return None,
    };
    let target = |i: usize| match call.args.get(i) {
        Some(Arg::Player(Some(q))) => Some(*q),
        _ => None,
    };
    let object = |i: usize| match call.args.get(i) {
        Some(Arg::Object(Some(o))) => Some(*o),
        _ => None,
    };
    let order = match call.entry.id {
        id::BOT_GO => Some(Order::Go(zone(m, table, object(1)?)?.0)),
        id::BOT_HOLD => {
            let (centre, radius) = zone(m, table, object(1)?)?;
            Some(Order::Hold(centre, radius))
        }
        id::BOT_FOLLOW => Some(Order::Follow(u32::from(target(1)?))),
        id::BOT_ATTACK => Some(Order::Attack(u32::from(target(1)?))),
        id::BOT_FREE => None,
        _ => return None,
    };
    Some((player, order))
}

/// The centre and reach of the labelled thing `handle` names: the mean of
/// its label's members' positions (a zone of one flag is the flag), and
/// the farthest member from the centre, at least [`HOLD_RADIUS`]. `None`
/// for a thing the table has not, or one a host reads no position of
/// (only netgame flags have one).
fn zone(m: &dyn GuestMemory, table: Option<&Table>, handle: u32) -> Option<(Vec3, f32)> {
    let table = table?;
    let thing = table.get(handle)?;
    if thing.block != BlockKind::NetgameFlags {
        return None;
    }
    let flags = engine::objectives::netgame_flags(m).ok()?;
    let mut members = Vec::new();
    for placed in table.labelled(&thing.text) {
        let member = table.get(placed.object)?;
        let (at, _, _) = flags.get(member.index as usize)?;
        members.push(*at);
    }
    let n = members.len() as f32;
    let mut centre = [0.0; 3];
    for at in &members {
        for (c, p) in centre.iter_mut().zip(at) {
            *c += p / n;
        }
    }
    let radius = members
        .iter()
        .map(|at| {
            ((at[0] - centre[0]).powi(2) + (at[1] - centre[1]).powi(2) + (at[2] - centre[2]).powi(2))
                .sqrt()
        })
        .fold(HOLD_RADIUS, f32::max);
    Some((centre, radius))
}

/// What a bot-goal call does: the team and the place set for it, or
/// `None` to clear the team's. The place is the zone the script's
/// `inside` tests (`super::zones::Zones`), its shape included, so a bot
/// stands where the script counts it in. A call through `none`, or to a
/// thing with no zone (not a netgame flag, or not on the map), makes
/// nothing, as a write through `none` does.
pub fn goal(m: &dyn GuestMemory, table: Option<&Table>, call: &VmCall) -> Option<(u8, Option<ScriptGoal>)> {
    let team = match call.args.first() {
        Some(Arg::Team(Some(t))) => *t,
        _ => return None,
    };
    let aim = match call.entry.id {
        id::BOT_GOAL_ATTACK => Aim::Attack,
        id::BOT_GOAL_DEFEND => Aim::Defend,
        id::BOT_GOAL_HOLD => Aim::Hold,
        id::BOT_GOAL_CLEAR => return Some((team, None)),
        _ => return None,
    };
    let handle = match call.args.get(1) {
        Some(Arg::Object(Some(o))) => *o,
        _ => return None,
    };
    let zones = super::zones::Zones::read(m, table?);
    Some((team, Some(place(team, aim, handle, zones.get(handle)?))))
}

fn place(team: u8, aim: Aim, handle: u32, zone: &super::zones::Zone) -> ScriptGoal {
    let super::zones::Shape::Area { at, yaw, area } = &zone.shape;
    ScriptGoal {
        team: i16::from(team),
        aim,
        zone: handle,
        at: *at,
        yaw: *yaw,
        area: area.clone(),
    }
}

/// Sets a place in its team's bot goals, or clears the team's (`None`).
/// A refusal ([`bots::objective::script::Refused`]) is the call's error,
/// recorded by its text.
pub fn set_goal(team: u8, goal: Option<ScriptGoal>) -> anyhow::Result<()> {
    match goal {
        Some(g) => ::bots::objective::script::set(g).map_err(anyhow::Error::from),
        None => {
            ::bots::objective::script::clear(Some(i16::from(team)));
            Ok(())
        }
    }
}

/// Delivers an order to the seat's phantom brain ([`bots::phantom`]);
/// `false` when no joined phantom has the seat.
pub fn deliver(player: u8, order: Option<Order>) -> bool {
    ::bots::phantom::order(u32::from(player), order)
}

#[cfg(test)]
mod tests {
    use ::rules::caps;

    use super::*;
    use crate::rules::census;

    const SCNR: u32 = 0x1006_0000;
    const FLAGS_AT: u32 = SCNR + 0x400;

    fn call(name: &str, args: Vec<Arg>) -> VmCall {
        VmCall {
            entry: caps::entry_named(name).unwrap(),
            args,
        }
    }

    /// A guest whose scenario holds one territory flag at `at`.
    fn flag_map(at: Vec3) -> guest_layout::testing::FakeGuest {
        let mut m = guest_layout::testing::FakeGuest::new();
        m.put_u32(guest_layout::points::SCENARIO.base, SCNR);
        m.map(SCNR, 0x1000);
        m.put_u32(SCNR + engine::objectives::NETGAME_FLAGS, 1)
            .put_u32(SCNR + engine::objectives::NETGAME_FLAGS + 4, FLAGS_AT);
        m.put_f32(FLAGS_AT, at[0])
            .put_f32(FLAGS_AT + 4, at[1])
            .put_f32(FLAGS_AT + 8, at[2])
            .put_u16(FLAGS_AT + engine::objectives::FLAG_TYPE, engine::objectives::kind::TERRITORY)
            .put_u16(FLAGS_AT + engine::objectives::FLAG_TEAM, 0);
        m
    }

    fn table(m: &dyn GuestMemory) -> Table {
        let ir = ::rules::compile(
            "rules \"T\" 1\ntype slayer\n\non kill(victim, killer) { if killer != none { score(killer, 1) } }\n",
        )
        .unwrap();
        Table::new(&census::things(m).unwrap(), &ir)
    }

    #[test]
    fn follow_attack_and_free_need_only_the_roster() {
        let m = guest_layout::testing::FakeGuest::new();
        assert_eq!(
            plan(&m, None, &call("bot_follow", vec![Arg::Player(Some(1)), Arg::Player(Some(0))])),
            Some((1, Some(Order::Follow(0))))
        );
        assert_eq!(
            plan(&m, None, &call("bot_attack", vec![Arg::Player(Some(3)), Arg::Player(Some(2))])),
            Some((3, Some(Order::Attack(2))))
        );
        assert_eq!(
            plan(&m, None, &call("bot_free", vec![Arg::Player(Some(3))])),
            Some((3, None))
        );
        // A call through `none` does nothing.
        assert_eq!(
            plan(&m, None, &call("bot_follow", vec![Arg::Player(Some(1)), Arg::Player(None)])),
            None
        );
        assert_eq!(plan(&m, None, &call("bot_free", vec![Arg::Player(None)])), None);
    }

    #[test]
    fn go_and_hold_resolve_the_zone_from_the_map() {
        let at = [15.0, 5.0, 1.0];
        let m = flag_map(at);
        let table = table(&m);
        let handle = table.labelled("territory")[0].object;
        assert_eq!(
            plan(&m, Some(&table), &call("bot_go", vec![Arg::Player(Some(1)), Arg::Object(Some(handle))])),
            Some((1, Some(Order::Go(at))))
        );
        assert_eq!(
            plan(&m, Some(&table), &call("bot_hold", vec![Arg::Player(Some(2)), Arg::Object(Some(handle))])),
            Some((2, Some(Order::Hold(at, HOLD_RADIUS))))
        );
        // A handle the map has not, and a call through `none`, make nothing.
        assert_eq!(
            plan(&m, Some(&table), &call("bot_go", vec![Arg::Player(Some(1)), Arg::Object(Some(0))])),
            None
        );
        assert_eq!(
            plan(&m, Some(&table), &call("bot_hold", vec![Arg::Player(Some(1)), Arg::Object(None)])),
            None
        );
    }

    #[test]
    fn a_team_goal_takes_the_zone_the_script_counts_in() {
        let at = [15.0, 5.0, 1.0];
        let m = flag_map(at);
        let table = table(&m);
        let handle = table.labelled("territory")[0].object;
        let args = |t| vec![Arg::Team(Some(t)), Arg::Object(Some(handle))];
        let (team, g) = goal(&m, Some(&table), &call("bot_goal_attack", args(0))).unwrap();
        let g = g.unwrap();
        assert_eq!((team, g.team, g.aim, g.zone), (0, 0, Aim::Attack, handle));
        // The zone's own shape: the bot is in it where `inside` says so.
        let zone = super::super::zones::Zones::read(&m, &table);
        let shape = &zone.get(handle).unwrap().shape;
        assert_eq!(g.middle(), shape.middle());
        assert!(g.contains(at) && shape.contains(at));
        assert_eq!(
            goal(&m, Some(&table), &call("bot_goal_defend", args(1))).unwrap().1.unwrap().aim,
            Aim::Defend
        );
        assert_eq!(
            goal(&m, Some(&table), &call("bot_goal_clear", vec![Arg::Team(Some(1))])),
            Some((1, None))
        );
        // A thing with no zone, and a call through `none`, make nothing.
        let none = vec![Arg::Team(Some(0)), Arg::Object(Some(0))];
        assert_eq!(goal(&m, Some(&table), &call("bot_goal_hold", none)), None);
        assert_eq!(goal(&m, Some(&table), &call("bot_goal_clear", vec![Arg::Team(None)])), None);
    }

    #[test]
    fn delivering_to_a_seat_no_phantom_has_is_refused() {
        assert!(!deliver(15, Some(Order::Go([0.0; 3]))));
    }
}
