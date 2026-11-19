//! Halo 1 encounter script operations and their Halo 2 evaluator bindings.
//! Addresses are from the retail Xbox executables (docs/halo1/ai-control.md).

use crate::translate::{H2Builtin, Target};
use std::collections::HashMap;

pub(crate) const RENAMES: &[(&str, &str)] = &[
    ("ai_magically_see_unit", "ai_magically_see_object"),
    ("ai_go_to_vehicle_override", "ai_vehicle_enter"),
    ("ai_dialogue_triggers", "ai_dialogue_enable"),
];

/// An evaluator whose name cannot be learned from retail script nodes.
/// We require the evaluator and signature as well as its index. Several
/// nearby descriptors have the same signature and are retail no-ops.
pub struct Binding {
    pub name: &'static str,
    pub index: u16,
    pub evaluator: u32,
    pub returns: &'static str,
    pub arguments: &'static [&'static str],
}

pub const BINDINGS: &[Binding] = &[
    Binding {
        name: "ai_going_to_vehicle",
        index: hs::h1_ai_slots::VEHICLE_COUNT,
        evaluator: hs::h1_ai_slots::VEHICLE_COUNT_EVALUATOR,
        returns: "short",
        arguments: &["unit"],
    },
    Binding {
        name: "ai_grenades",
        index: 280,
        evaluator: 0x002a_4d70,
        returns: "void",
        arguments: &["boolean"],
    },
    Binding {
        name: "ai_detach",
        index: 288,
        evaluator: 0x002a_4f20,
        returns: "void",
        arguments: &["unit"],
    },
    Binding {
        // Retail's one-argument overload places both squads and groups.
        name: "ai_place",
        index: 290,
        evaluator: 0x002a_4fd0,
        returns: "void",
        arguments: &["ai"],
    },
    Binding {
        // Halo 2 has overloads of ai_place. The counted one (291) is
        // needed here even on discs where scripts only call 290 by name.
        name: "ai_place_counted",
        index: 291,
        evaluator: 0x002a_5010,
        returns: "void",
        arguments: &["ai", "short"],
    },
    Binding {
        // The reserved (ai) slot shares the retail discarded evaluator.
        // boot::h1_ai_control supplies its body on converted campaign maps.
        name: "ai_magically_see_players",
        index: hs::h1_ai_slots::CONTROL,
        evaluator: hs::h1_ai_slots::DISCARD_EVALUATOR,
        returns: "void",
        arguments: &["ai"],
    },
    Binding {
        name: "ai_allegiance_remove",
        index: 310,
        evaluator: 0x002a_5540,
        returns: "void",
        arguments: &["team", "team"],
    },
    Binding {
        name: "ai_braindead_by_unit",
        index: 312,
        evaluator: 0x002a_55f0,
        returns: "void",
        arguments: &["object_list", "boolean"],
    },
    Binding {
        name: "ai_force_active",
        index: 317,
        evaluator: 0x002a_5760,
        returns: "void",
        arguments: &["ai", "boolean"],
    },
    Binding {
        name: "ai_force_active_by_unit",
        index: 318,
        evaluator: 0x002a_57b0,
        returns: "void",
        arguments: &["unit", "boolean"],
    },
    Binding {
        name: "ai_set_team",
        index: 322,
        evaluator: 0x002a_5850,
        returns: "void",
        arguments: &["ai", "team"],
    },
    Binding {
        name: "ai_living_fraction",
        index: 327,
        evaluator: 0x002a_59a0,
        returns: "real",
        arguments: &["ai"],
    },
    Binding {
        name: "ai_allegiance_broken",
        index: 332,
        evaluator: 0x002a_5b70,
        returns: "boolean",
        arguments: &["team", "team"],
    },
    Binding {
        // Retail also calls 344 as ai_vehicle_enter. Keep the seat mapping
        // overload by name even when the two-argument overload was seen last.
        name: "ai_vehicle_enter",
        index: 343,
        evaluator: 0x002a_5ec0,
        returns: "void",
        arguments: &["ai", "unit", "unit_seat_mapping"],
    },
];

pub(crate) fn bind(builtins: &mut HashMap<String, H2Builtin>, catalog: &hs::Catalog) {
    for entry in BINDINGS {
        let Ok(function) = catalog.function(entry.index) else {
            continue;
        };
        if function.evaluate() != entry.evaluator
            || function.flags() & 2 != 0
            || catalog.type_name(function.return_type()).ok() != Some(entry.returns)
            || !function
                .parameters()
                .iter()
                .map(|&p| catalog.type_name(p).ok())
                .eq(entry.arguments.iter().copied().map(Some))
        {
            continue;
        }
        builtins.insert(
            entry.name.to_owned(),
            H2Builtin {
                index: entry.index,
                return_type: function.return_type(),
                parameters: function.parameters().to_vec(),
                special: false,
            },
        );
    }
}

/// Builtins with an extension opcode and a retail return profile.
pub(crate) fn runtime_operation(name: &str) -> Option<hs::h1_ai_slots::Operation> {
    use hs::h1_ai_slots::Operation as RuntimeOperation;

    match name {
        "ai_magically_see_players" => Some(RuntimeOperation::SeePlayers),
        "ai_go_to_vehicle" => Some(RuntimeOperation::VehicleEnter),
        "ai_vehicle_encounter" => Some(RuntimeOperation::VehicleEncounter),
        "ai_vehicle_enterable_distance" => Some(RuntimeOperation::VehicleDistance),
        "ai_vehicle_enterable_actor_type" => Some(RuntimeOperation::VehicleActorType),
        "ai_vehicle_enterable_team" => Some(RuntimeOperation::VehicleTeam),
        "ai_vehicle_enterable_actors" => Some(RuntimeOperation::VehicleActors),
        "ai_vehicle_enterable_disable" => Some(RuntimeOperation::VehicleDisable),
        "ai_going_to_vehicle" => Some(RuntimeOperation::VehicleCount),
        "ai_try_to_fight" => Some(RuntimeOperation::PreferAi),
        "ai_try_to_fight_player" => Some(RuntimeOperation::PreferPlayer),
        "ai_try_to_fight_nothing" => Some(RuntimeOperation::ClearPreference),
        "ai_playfight" => Some(RuntimeOperation::Playfight),
        "ai_link_activation" => Some(RuntimeOperation::LinkActivation),
        "ai_timer_expire" => Some(RuntimeOperation::ExpireSquadTimers),
        "ai_automatic_migration_target" => Some(RuntimeOperation::AutomaticMigrationTarget),
        "ai_follow_target_players" => Some(RuntimeOperation::FollowPlayers),
        "ai_follow_target_ai" => Some(RuntimeOperation::FollowAi),
        "ai_follow_target_unit" => Some(RuntimeOperation::FollowUnit),
        "ai_follow_target_disable" => Some(RuntimeOperation::FollowDisable),
        "ai_follow_distance" => Some(RuntimeOperation::FollowDistance),
        "ai_migrate_and_speak" => Some(RuntimeOperation::MigrateSpeak),
        "ai_attach_free" => Some(RuntimeOperation::AttachFree),
        "ai_free" => Some(RuntimeOperation::Free),
        "ai_free_units" => Some(RuntimeOperation::FreeUnits),
        "ai_migrate_by_unit" => Some(RuntimeOperation::MigrateUnits),
        "ai_teleport_to_starting_location" => Some(RuntimeOperation::Teleport),
        "ai_teleport_to_starting_location_if_unsupported" => {
            Some(RuntimeOperation::TeleportUnsupported)
        }
        _ => None,
    }
}

pub(crate) fn runtime_target(target: &Target, op: hs::h1_ai_slots::Operation) -> Option<&H2Builtin> {
    use hs::h1_ai_slots::{CONTROL, Operation};
    let control = target.builtins.get("ai_magically_see_players")?;
    if control.index != CONTROL {
        return None;
    }
    if op == Operation::VehicleCount {
        return target
            .builtins
            .get("ai_going_to_vehicle")
            .filter(|b| b.index == op.slot());
    }
    Some(control)
}

pub(crate) fn status_supported(target: &Target) -> bool {
    ["ai_combat_status", "if", ">="]
        .iter()
        .all(|name| target.builtins.contains_key(*name))
        && crate::compile::STATUS_LADDER
            .iter()
            .all(|(name, _)| target.externals.contains_key(*name))
}

/// The lookup key distinguishes the overload. The expression carries its
/// retail name so an observed Halo 2 catalog can validate both maps.
pub(crate) fn retail_name(name: &str) -> &str {
    match name {
        "ai_place_counted" => "ai_place",
        _ => name,
    }
}

pub(crate) fn operation(name: &str) -> Option<h1ai::control::Operation> {
    use h1ai::control::Operation::*;
    Some(match name {
        "ai_attack" => Attack,
        "ai_defend" => Defend,
        "ai_retreat" | "ai_maneuver" => Maneuver,
        "ai_maneuver_enable" => ManeuverEnable,
        "ai_timer_expire" => TimerExpire,
        "ai_spawn_actor" => Spawn,
        "ai_follow_target_players" => FollowPlayers,
        "ai_follow_target_ai" => FollowAi,
        "ai_follow_target_disable" => FollowDisable,
        "ai_follow_distance" => FollowDistance,
        "ai_migrate" => Migrate,
        _ => return None,
    })
}

pub(crate) fn request(
    name: &str,
    arg: &hs::Node,
    tables: &crate::Tables,
) -> Option<h1ai::control::Request> {
    use h1ai::control::Operation;
    let ai = |node: &hs::Node| {
        (node.atom() && !node.global() && node.opcode() == crate::catalog::types::AI)
            .then_some(node.data())
    };
    let operation = operation(name)?;
    let target = match operation {
        Operation::FollowAi | Operation::Migrate => Some(ai(tables.node(arg.next()).ok()?)?),
        Operation::FollowDistance => {
            let radius = tables.node(arg.next()).ok()?;
            let value = f32::from_bits(radius.data());
            if !radius.atom()
                || radius.global()
                || radius.opcode() != crate::catalog::types::REAL
                || !value.is_finite()
                || value < 0.0
            {
                return None;
            }
            Some(radius.data())
        }
        _ => None,
    };
    Some(h1ai::control::Request {
        operation,
        reference: ai(arg)?,
        target,
    })
}

/// References that should guide the choice of empty squads retained in the
/// rebuilt map. An unknown follow subject can select any encounter.
#[derive(Default)]
pub struct RetainedSubjects {
    pub follow_encounters: Option<std::collections::BTreeSet<u16>>,
    pub teleport_subjects: Option<std::collections::BTreeSet<u32>>,
    pub migration_destinations: std::collections::BTreeSet<String>,
}

pub fn retained_subjects(tables: &crate::Tables, catalog: &crate::Catalog) -> RetainedSubjects {
    let mut retained = RetainedSubjects {
        follow_encounters: Some(Default::default()),
        teleport_subjects: Some(Default::default()),
        ..RetainedSubjects::default()
    };
    let literal_ai = |node: &hs::Node| {
        node.atom() && !node.global() && node.opcode() == crate::catalog::types::AI
    };
    for node in tables.nodes.iter().filter(|n| n.live() && !n.atom() && !n.script_call()) {
        let Ok(function) = catalog.function(node.opcode()) else {
            continue;
        };
        let first = || {
            let head = tables.node(hs::Datum(node.data())).ok()?;
            tables.node(head.next()).ok()
        };
        match function.name.as_str() {
            "ai_follow_target_players" | "ai_follow_target_ai" | "ai_follow_target_unit" => {
                if let Some(subject) = first().filter(|n| literal_ai(n)) {
                    if let Some(encounters) = &mut retained.follow_encounters {
                        encounters.insert(subject.data() as u16);
                    }
                } else {
                    retained.follow_encounters = None;
                }
            }
            "ai_teleport_to_starting_location" | "ai_teleport_to_starting_location_if_unsupported" => {
                if let Some(subject) = first().filter(|n| literal_ai(n)) {
                    if let Some(subjects) = &mut retained.teleport_subjects {
                        subjects.insert(subject.data());
                    }
                } else {
                    retained.teleport_subjects = None;
                }
            }
            "ai_migrate" | "ai_migrate_and_speak" | "ai_migrate_by_unit" | "ai_vehicle_encounter" => {
                if let Some(destination) = first().and_then(|arg| tables.node(arg.next()).ok())
                    .filter(|n| literal_ai(n) && n.data() >> 30 == 2)
                    && let Ok(name) = tables.string(destination.string_offset())
                {
                    retained.migration_destinations.insert(name.to_ascii_lowercase());
                }
            }
            _ => {}
        }
    }
    retained
}

/// Level data has to be built from the original squad and platoon indices
/// before the converter drops unused squads. Only direct AI literals name
/// a stable source index.
pub fn requests(tables: &crate::Tables, catalog: &crate::Catalog) -> Vec<h1ai::control::Request> {
    use h1ai::control::Operation::*;
    use hs::Datum;
    let mut out = Vec::new();
    let mut dynamic_follow = false;
    let mut begins = Vec::new();
    for (slot, node) in tables.nodes.iter().enumerate() {
        if !node.live() || node.atom() || node.script_call() {
            continue;
        }
        let Ok(function) = catalog.function(node.opcode()) else {
            continue;
        };
        let call = Datum(u32::from(node.salt()) << 16 | slot as u32);
        if function.name == "begin" {
            begins.push(call);
            continue;
        }
        let Some(operation) = operation(&function.name) else {
            dynamic_follow |= function.name == "ai_follow_target_unit";
            continue;
        };
        let request = tables
            .node(Datum(node.data()))
            .ok()
            .and_then(|head| tables.node(head.next()).ok())
            .and_then(|arg| request(&function.name, arg, tables));
        if let Some(request) = request {
            out.push((call, request));
        } else if matches!(operation, FollowDistance | FollowPlayers | FollowAi) {
            dynamic_follow = true;
        }
    }
    out.iter()
        .filter_map(|&(call, request)| {
            if request.operation != FollowDistance {
                return Some(request);
            }
            if dynamic_follow {
                return None;
            }
            let same_encounter =
                |other: &h1ai::control::Request| other.reference as u16 == request.reference as u16;
            // A literal distance can move into an order when there is exactly
            // one setter and one following operation, and their shared begin
            // runs the setter first. Actors can't observe the radius before
            // the only operation that makes them follow. c20 uses this form.
            let mut controls = out.iter().filter(|(_, other)| {
                same_encounter(other)
                    && matches!(other.operation, FollowDistance | FollowPlayers | FollowAi)
            });
            if controls.clone().count() != 2 {
                return None;
            }
            let &(follow, _) = controls.find(|(_, r)| r.operation == FollowPlayers)?;
            begins
                .iter()
                .any(|&parent| ordered_children(tables, parent, call, follow))
                .then_some(request)
        })
        .collect()
}

fn ordered_children(
    tables: &crate::Tables,
    parent: hs::Datum,
    before: hs::Datum,
    after: hs::Datum,
) -> bool {
    let Ok(head) = tables
        .node(parent)
        .and_then(|node| tables.node(hs::Datum(node.data())))
    else {
        return false;
    };
    let mut next = head.next();
    let mut seen = false;
    for _ in 0..tables.nodes.len() {
        if next == after {
            return seen;
        }
        seen |= next == before;
        let Ok(arg) = tables.node(next) else {
            break;
        };
        next = arg.next();
    }
    false
}
