//! A script's HUD on the HUD kit (docs/game/rules-world-api.md, "HUD";
//! docs/game/game-types-without-carriers.md, "The display"): the show
//! state the VM keeps (`rules::vm::RulesExt::show`) feeds the objective
//! HUDs (`hud_kit_core::views::objective`: `ctf`, `assault`, `oddball`,
//! `koth`, `territories`, `siege`) for each local user.
//!
//! A script declares widgets at the elements of those HUDs
//! (`hud hill: text at koth.hill`) and waypoints at their navpoints
//! (`waypoint hill_mark at koth.mark`); the HUD a script feeds is the one
//! its slots' first word names ([`huds_of`]). Each tick, for each local
//! user, [`publish`] makes `rules::show::view` of the state for that
//! user's player, turns it into the kit's [`Fed`] ([`fed`]) and publishes
//! the HUD's values (`objective::feed`). The view is a pure function of
//! the state's bytes, the blob and the viewer, so the host, a replica that
//! runs the same VM and a machine that holds only the state's bytes make
//! the same values; the tests below run all three.
//!
//! A waypoint marks an object: the navpoint is over it and its text is the
//! distance from the user's unit, in metres (a world unit is 3.048 metres,
//! 10 feet: docs/game/film-format.md, "Tag 3 presentation records"). The place is a zone's
//! middle (a labelled flag's area), else the unit of the player carrying the
//! objective (a carried flag, bomb or ball), else the object's own origin
//! (a dropped one): [`place`].
//!
//! A widget the script reads per viewer (`hud_value_for`: the Territories
//! count of those the viewer's side holds) is made per viewer by
//! `rules::show::view`.
//!
//! `Viewer`s, machines that run no VM, take the state over the actor
//! channel's `Show` record (`rules::show::State::decode` on its bytes,
//! `super::show::receive`), which the capture offers from the authority
//! and a digest's hash repairs after a loss.

use std::sync::{Arc, OnceLock};

use ::rules::ir::Ir;
use ::rules::show::{State, View, Viewer, view};
use guest_layout::GuestMemory;
use guest_layout::handle::DatumHandle;
use guest_layout::view::objects;
use hud_kit_core::model::{Describe, Values};
use hud_kit_core::views::objective::{self, Fed, Widget};

use super::show::roster;
use super::zones::{self, Body, Zones};
use crate::hud_kit::{self, Handle, USERS};

/// The objective HUDs a script feeds, by name.
pub const HUDS: [&str; 6] = [
    objective::ctf::NAME,
    objective::assault::NAME,
    objective::oddball::NAME,
    objective::koth::NAME,
    objective::territories::NAME,
    objective::siege::NAME,
];

/// The first word of a slot (`odd` in `odd.holder`, the prefix the kit's
/// element ids carry) and the objective HUD it belongs to.
const SLOT_WORDS: [(&str, &str); 6] = [
    ("ctf", objective::ctf::NAME),
    ("asl", objective::assault::NAME),
    ("odd", objective::oddball::NAME),
    ("koth", objective::koth::NAME),
    ("ter", objective::territories::NAME),
    ("siege", objective::siege::NAME),
];

/// Metres in a world unit: 304.8 cm, 10 feet
/// (docs/game/film-format.md, "Tag 3 presentation records"; the guest's
/// world unit; `superbounce_mode::METRES_PER_UNIT` is the same).
const METRES_PER_UNIT: f32 = 3.048;

/// The kit's handle of each objective HUD, registered once.
fn handles() -> &'static [(&'static str, Handle)] {
    static HANDLES: OnceLock<Vec<(&'static str, Handle)>> = OnceLock::new();
    HANDLES.get_or_init(|| {
        let all: [(&'static str, Arc<dyn Describe + Send + Sync>); 6] = [
            (objective::ctf::NAME, Arc::new(objective::Ctf)),
            (objective::assault::NAME, Arc::new(objective::Assault)),
            (objective::oddball::NAME, Arc::new(objective::Oddball)),
            (objective::koth::NAME, Arc::new(objective::Koth)),
            (objective::territories::NAME, Arc::new(objective::Territories)),
            (objective::siege::NAME, Arc::new(objective::Siege)),
        ];
        all.into_iter().map(|(name, d)| (name, hud_kit::register(d))).collect()
    })
}

/// Every element of the HUD `name` hidden.
fn hidden(name: &str) -> Values {
    objective::feed(name, &Fed::default()).unwrap_or_default()
}

/// Registers the objective HUDs with the kit, hidden, so the game installs
/// their widgets at its activation. Called once when the rules feature
/// installs.
pub fn register() {
    for &(name, handle) in handles() {
        handle.publish(hidden(name));
    }
}

/// Hides every objective HUD, for every local user.
pub fn clear() {
    for &(name, handle) in handles() {
        handle.publish(hidden(name));
        for user in 1..USERS {
            handle.publish_for(user, None);
        }
        handle.navpoint(None);
    }
}

/// The objective HUDs the script `ir` feeds: those its widgets' and
/// waypoints' slots name before the dot, in the order they first appear.
pub fn huds_of(ir: &Ir) -> Vec<&'static str> {
    let slots = ir
        .widgets
        .iter()
        .map(|w| w.slot)
        .chain(ir.waypoints.iter().filter_map(|w| w.slot));
    let mut out: Vec<&'static str> = Vec::new();
    for slot in slots {
        let Some(slot) = ir.strings.get(usize::from(slot)) else {
            continue;
        };
        let word = slot.split('.').next().unwrap_or_default();
        if let Some(&(_, name)) = SLOT_WORDS.iter().find(|&&(w, _)| w == word)
            && !out.contains(&name)
        {
            out.push(name);
        }
    }
    out
}

/// Metres between two points, as a navpoint's text, at most the kit's
/// widest.
pub fn metres(from: [f32; 3], to: [f32; 3]) -> u16 {
    let d: f32 = from.iter().zip(&to).map(|(a, b)| (a - b) * (a - b)).sum::<f32>().sqrt();
    (d * METRES_PER_UNIT)
        .round()
        .clamp(0.0, f32::from(objective::MARK_MAX)) as u16
}

/// Where the object a waypoint marks is: the middle of its zone; else, for
/// an objective a player carries, that player's unit (`carrier_at`); else
/// the object's own origin (`origin`). `None` for an object that is none of
/// these (a deleted one).
pub fn place(
    zones: &Zones,
    object: u32,
    carrier_at: &dyn Fn(u32) -> Option<[f32; 3]>,
    origin: &dyn Fn(u32) -> Option<[f32; 3]>,
) -> Option<[f32; 3]> {
    match zones.get(object) {
        Some(zone) => Some(zone.shape.middle()),
        None => carrier_at(object).or_else(|| origin(object)),
    }
}

/// The unit of the player carrying the objective `object`, from the
/// objective registry and the living bodies.
fn carried_at(m: &dyn GuestMemory, bodies: &[Body], object: u32) -> Option<[f32; 3]> {
    let slot = crate::rules::objectives::carrier(m, object).ok()??;
    bodies.iter().find(|b| b.slot == slot).map(|b| b.at)
}

/// The origin of the live object `object` (`Object::POSITION`), when it is
/// a datum with a body and its position is a number.
fn origin_of(m: &dyn GuestMemory, object: u32) -> Option<[f32; 3]> {
    let (_, body) = objects::get(m, DatumHandle(object)).ok()??;
    body.position(m).ok().filter(|p| p.iter().all(|c| c.is_finite()))
}

/// What `viewer` sees of the script's state, as the kit's objective HUDs
/// read it: each widget at its element, a widget whose value is a side
/// marked `mine` when the viewer is on it, the names of the players a
/// widget's value names by slot (16 and the slot), and the distance each
/// waypoint's navpoint shows, from `distance` (the waypoint's object).
pub fn fed(shown: &View, viewer: Viewer, names: &[(u8, String)], distance: &dyn Fn(u32) -> Option<u16>) -> Fed {
    let mut out = Fed {
        team: viewer.team,
        ..Fed::default()
    };
    for w in &shown.widgets {
        out.widgets.push((
            w.slot.clone(),
            Widget {
                shown: w.shown,
                value: w.value,
                style: w.style,
                text: w.text.clone(),
                mine: viewer.on_side(w.value),
            },
        ));
        if let Some(slot) = w.value.checked_sub(16).and_then(|s| u8::try_from(s).ok()).filter(|&s| s < 16)
            && let Some(name) = names.iter().find(|(s, _)| *s == slot)
        {
            out.names.push(name.clone());
        }
    }
    for p in &shown.waypoints {
        if let (Some(slot), Some(d)) = (&p.slot, distance(p.at)) {
            out.distances.push((slot.clone(), d));
        }
    }
    out
}

/// Feeds the objective HUDs `ir` names from `state`, for each local user
/// of this machine, and puts each waypoint's navpoint over its zone for
/// user 0 (the kit's navpoints are shared). Returns whether a HUD is fed,
/// so the runtime hides them when none is.
pub fn publish(m: &dyn GuestMemory, ir: &Ir, state: &State, zones: &Zones) -> bool {
    let used = huds_of(ir);
    if used.is_empty() {
        return false;
    }
    let roster = roster(m);
    let bodies = zones::bodies(m);
    let names: Vec<(u8, String)> = roster.iter().map(|p| (p.slot, p.name.clone())).collect();
    for &(name, handle) in handles() {
        if !used.contains(&name) {
            continue;
        }
        for p in &roster {
            let Some(user) = p.user else {
                continue;
            };
            let viewer = Viewer {
                slot: Some(p.slot),
                team: p.team,
            };
            let shown = view(state, ir, viewer);
            let me = bodies.iter().find(|b| b.slot == p.slot).map(|b| b.at);
            let at = |object: u32| {
                place(
                    zones,
                    object,
                    &|o| carried_at(m, &bodies, o),
                    &|o| origin_of(m, o),
                )
            };
            let distance = |object: u32| Some(metres(me?, at(object)?));
            let values = objective::feed(name, &fed(&shown, viewer, &names, &distance)).unwrap_or_default();
            if user == 0 {
                handle.publish(values);
                for def in &ir.waypoints {
                    let Some(slot) = def.slot.and_then(|s| ir.strings.get(usize::from(s))) else {
                        continue;
                    };
                    if slot.split('.').next() != Some(name) {
                        continue;
                    }
                    handle.navpoint_at(slot, shown.waypoint_at(slot).and_then(|w| at(w.at)));
                }
            } else {
                handle.publish_for(user, Some(values));
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rules::fake::FakeGame;
    use ::rules::show::MAX_STATE_BYTES;
    use ::rules::vm::{Input, Vm};

    /// An invented script: it declares the Territories-like count and the
    /// King of the Hill rows, and sets them at the round's start.
    const SCRIPT: &str = "rules \"Fed\" 1\ntype slayer\nrequires hud.widgets, hud.waypoints\n\
        label hill: zones numbered\n\
        hud hill_state: text at koth.hill\n\
        hud held: text at koth.time\n\
        hud moved: banner at koth.banner\n\
        waypoint hill_mark at koth.mark\n\
        on round_start {\n\
            hud_style(hill_state, 1)\n\
            hud_value(hill_state, 1)\n\
            hud_show(hill_state, true)\n\
            hud_value(held, 41)\n\
            hud_show(held, true)\n\
            hud_text(moved, \"HILL MOVED\")\n\
            hud_show(moved, true)\n\
            if let z = hill[0] { mark(hill_mark, z) }\n\
        }\n";

    /// One machine's VM after the round's start.
    fn run() -> Vm {
        let (ir, _) = ::rules::build(SCRIPT).unwrap();
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        let mut g = FakeGame::with_players(2);
        g.label("hill", 0x4001_0000, 0, 1);
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        assert_eq!(g.script_error, None);
        vm
    }

    /// The koth HUD's rows for one viewer, from a state's bytes and the
    /// blob.
    fn rows(ir: &Ir, state: &State, viewer: Viewer) -> Vec<(String, Option<String>)> {
        let names = vec![(0, "Ann".to_string()), (1, "Bob".to_string())];
        let shown = view(state, ir, viewer);
        let values = objective::feed("koth", &fed(&shown, viewer, &names, &|_| Some(120))).unwrap();
        objective::koth::hud()
            .elements
            .iter()
            .map(|e| {
                let v = values.get(e.id).unwrap();
                (e.id.to_string(), v.shown.then(|| v.text.clone()))
            })
            .collect()
    }

    /// The same invented state shows in the HUD's values on the host, on a
    /// replica that runs its own VM and on a machine that holds only the
    /// state's bytes; a viewer on the holding side reads the hill as its
    /// own, the other side's as the enemy's.
    #[test]
    fn an_invented_script_state_reaches_the_objective_huds_values_on_the_host_and_a_replica() {
        let host = run();
        let replica = run();
        assert_eq!(host.ext().encode(), replica.ext().encode(), "the same state on both machines");
        let bytes = host.show().encode();
        assert!(bytes.len() <= MAX_STATE_BYTES);
        let held = State::decode(&bytes).unwrap();
        assert_eq!(&held, host.show());

        let team1 = Viewer { slot: Some(0), team: Some(1) };
        let team0 = Viewer { slot: Some(1), team: Some(0) };
        let row = |text: &str| Some(text.to_string());
        let ours = vec![
            ("koth.hill".into(), row("Hill: Yours")),
            ("koth.time".into(), row("Held 0:41")),
            ("koth.phase".into(), None),
            ("koth.mark".into(), row("120")),
            ("koth.banner".into(), row("HILL MOVED")),
        ];
        let theirs = vec![
            ("koth.hill".into(), row("Hill: Held by the enemy")),
            ("koth.time".into(), row("Held 0:41")),
            ("koth.phase".into(), None),
            ("koth.mark".into(), row("120")),
            ("koth.banner".into(), row("HILL MOVED")),
        ];
        let ir = host.ir();
        for state in [host.show(), replica.show(), &held] {
            assert_eq!(rows(ir, state, team1), ours);
            assert_eq!(rows(ir, state, team0), theirs);
        }
        // A machine with no player at the screen sees nothing.
        assert!(rows(ir, host.show(), Viewer::default()).iter().all(|(_, text)| text.is_none()));
        assert_eq!(huds_of(ir), ["koth"]);
    }

    #[test]
    fn a_waypoint_is_the_zone_the_script_marked() {
        let host = run();
        let shown = view(host.show(), host.ir(), Viewer { slot: Some(0), team: None });
        let mark = shown.waypoint_at("koth.mark").unwrap();
        assert_eq!(mark.at, 0x4001_0000);
        assert_eq!(mark.name, "hill_mark");
    }

    #[test]
    fn a_distance_is_whole_metres_and_stops_at_the_widest() {
        assert_eq!(metres([0.0; 3], [0.0; 3]), 0);
        assert_eq!(metres([0.0, 0.0, 0.0], [10.0, 0.0, 0.0]), 30);
        assert_eq!(metres([0.0; 3], [1.0e6, 0.0, 0.0]), objective::MARK_MAX);
    }

    /// A zone is its middle; an objective somebody carries is that
    /// player's unit; one on the ground is its own origin; a thing that is
    /// none of those has no mark.
    #[test]
    fn a_waypoint_follows_a_carried_object_and_rests_on_a_dropped_one() {
        let zones = Zones::default();
        let carried = |o: u32| (o == 1).then_some([10.0, 0.0, 0.0]);
        let ground = |o: u32| matches!(o, 1 | 2).then_some([99.0, 0.0, 0.0]);
        assert_eq!(place(&zones, 1, &carried, &ground), Some([10.0, 0.0, 0.0]));
        assert_eq!(place(&zones, 2, &carried, &ground), Some([99.0, 0.0, 0.0]));
        assert_eq!(place(&zones, 3, &carried, &ground), None);
    }

    /// The Territories count is the viewer's own: one value for each seat,
    /// the same on the host, a replica and a machine that holds the bytes.
    #[test]
    fn the_territories_count_is_each_viewers_own() {
        const TERRITORIES: &str = "rules \"Count\" 1\ntype slayer\nrequires hud.widgets\n\
            hud owned: text at ter.owned\n\
            on round_start {\n\
                hud_style(owned, 5)\n\
                hud_show(owned, true)\n\
                for each players p { hud_value_for(owned, p, 3 - 2 * slot_of(p)) }\n\
            }\n";
        let (ir, _) = ::rules::build(TERRITORIES).unwrap();
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        let mut g = FakeGame::with_players(2);
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        assert_eq!(g.script_error, None);
        let held = State::decode(&vm.show().encode()).unwrap();
        let text = |state: &State, slot: u8| {
            let viewer = Viewer { slot: Some(slot), team: None };
            let shown = view(state, vm.ir(), viewer);
            let values = objective::feed("territories", &fed(&shown, viewer, &[], &|_| None)).unwrap();
            values.get("ter.owned").unwrap().text.clone()
        };
        for state in [vm.show(), &held] {
            assert_eq!(text(state, 0), "Territories 3 of 5");
            assert_eq!(text(state, 1), "Territories 1 of 5");
        }
    }

    /// The built-in Siege feeds the Siege HUD and no other: after the
    /// round's start each side reads its own role, the clock, the open
    /// site, the sites to take and the round and phase, from the VM's
    /// state and from its bytes alike.
    #[test]
    fn the_built_in_siege_feeds_the_siege_hud_for_each_side() {
        use ::rules::vm::Placed;
        let (ir, _) = ::rules::build(::rules::builtin::SIEGE).unwrap();
        assert_eq!(huds_of(&ir), ["siege"]);
        let bits = ir.default_options();
        let mut vm = Vm::new(ir, bits, 0).unwrap();
        let mut g = FakeGame::with_players(2);
        g.players.get_mut(&1).unwrap().team = 1;
        g.variant.insert("team_play".into(), 1);
        g.labelled.insert(
            "territory".into(),
            vec![Placed { placement: 0, object: 600 }, Placed { placement: 1, object: 601 }],
        );
        g.labelled.insert(
            "ctf_flag_spawn".into(),
            vec![Placed { placement: 0, object: 20 }, Placed { placement: 1, object: 21 }],
        );
        g.object_teams.extend([(20, 0), (21, 1)]);
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        vm.begin_tick(1);
        vm.update(&mut g);
        assert_eq!(g.script_error, None);
        let held = State::decode(&vm.show().encode()).unwrap();
        assert!(vm.show().encode().len() <= MAX_STATE_BYTES);
        let rows = |state: &State, slot: u8, team: u8| -> Vec<Option<String>> {
            let viewer = Viewer { slot: Some(slot), team: Some(team) };
            let shown = view(state, vm.ir(), viewer);
            let values = objective::feed("siege", &fed(&shown, viewer, &[], &|_| Some(30))).unwrap();
            objective::siege::hud()
                .elements
                .iter()
                .map(|e| {
                    let v = values.get(e.id).unwrap();
                    v.shown.then(|| v.text.clone())
                })
                .collect()
        };
        for state in [vm.show(), &held] {
            let attack = rows(state, 0, 0);
            let defend = rows(state, 1, 1);
            // The clock is the phase's 180 s, less the tick that ran.
            assert!(attack[0].as_deref().is_some_and(|t| t.starts_with("ATTACK 2:5") || t == "ATTACK 3:00"), "{attack:?}");
            assert!(defend[0].as_deref().is_some_and(|t| t.starts_with("DEFEND ")), "{defend:?}");
            assert_eq!(attack[1].as_deref(), Some("SITE OPEN"));
            assert_eq!(attack[2].as_deref(), Some("SITES 0 OF 2"));
            // Two rounds by default; two sites and the stand are three phases.
            assert_eq!(attack[3].as_deref(), Some("ROUND 1 OF 2 PHASE 1 OF 3"));
            assert_eq!(attack[4].as_deref(), Some("30"));
            assert_eq!(&attack[1..], &defend[1..]);
        }
    }

    #[test]
    fn the_huds_a_script_feeds_are_named_by_its_slots_first_word() {
        let (ir, _) = ::rules::build(
            "rules \"X\" 1\ntype slayer\nrequires hud.widgets\non kill(v, k) { }\n\
             hud a: text at ctf.own\nhud b: text at odd.time\nhud c: text at elsewhere.row\n\
             hud d: text at ctf.enemy\n",
        )
        .unwrap();
        assert_eq!(huds_of(&ir), ["ctf", "oddball"]);
        let none = ::rules::build("rules \"Y\" 1\ntype slayer\nuses slayer\n").unwrap().0;
        assert!(huds_of(&none).is_empty());
    }

    /// A player named by a widget's value (16 and a slot) is looked up in
    /// the roster, and only that one.
    #[test]
    fn a_widget_naming_a_player_by_slot_carries_that_players_name() {
        let ir = ::rules::build(
            "rules \"Z\" 1\ntype slayer\nrequires hud.widgets\nhud holder: text at odd.holder\n\
             on round_start { hud_value(holder, 17)\nhud_style(holder, 1)\nhud_show(holder, true) }\n",
        )
        .unwrap()
        .0;
        let mut vm = Vm::new(ir, 0, 0).unwrap();
        let mut g = FakeGame::with_players(3);
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        let names = vec![(0, "Ann".to_string()), (1, "Bob".to_string())];
        let viewer = Viewer { slot: Some(0), team: None };
        let shown = view(vm.show(), vm.ir(), viewer);
        let f = fed(&shown, viewer, &names, &|_| None);
        assert_eq!(f.names, [(1, "Bob".to_string())]);
        let v = objective::feed("oddball", &f).unwrap();
        assert_eq!(v.get("odd.holder").unwrap().text, "Ball: Carried by Bob");
    }
}
