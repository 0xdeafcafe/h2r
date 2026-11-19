//! Level-specific routes for Halo 1's platoon controls.
//!
//! Keep the unpruned Halo 1 indices until script references have been
//! decoded. The converter may remove unplaced squads and renumber the
//! remaining ones. See docs/halo1/ai-control.md.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    convert::{Options, Plan},
    h1::{Encounter, H1Ai, PlatoonTrigger, Squad},
    h2::{AreaRef, Order},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Operation {
    Attack,
    Defend,
    Maneuver,
    ManeuverEnable,
    TimerExpire,
    Spawn,
    FollowPlayers,
    FollowAi,
    FollowDisable,
    FollowDistance,
    Migrate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Request {
    pub operation: Operation,
    pub reference: u32,
    /// A second AI reference or, for FollowDistance, the radius's f32 bits.
    pub target: Option<u32>,
}

/// Indices in the original map. Used to key one maneuver-enable global
/// per source platoon, even if multiple script spellings select that platoon.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Platoon {
    pub encounter: u16,
    pub index: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    SetOrder {
        squad: u16,
        order: u16,
    },
    Migrate {
        from: u16,
        to: u16,
        platoon: Option<Platoon>,
    },
    Enable(Platoon),
    ExpireTimer {
        squad: u16,
        reveal_players: bool,
    },
    Spawn(u16),
}

/// A source platoon trigger resolved to retained Halo 2 squads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trigger {
    pub condition: u16,
    pub squads: Vec<u16>,
}

/// Native squad orders and migration routes driven by a Halo 1 platoon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatoonRule {
    pub platoon: Platoon,
    pub members: Vec<u16>,
    pub change: Option<Trigger>,
    pub maneuver: Option<Trigger>,
    pub orders: Vec<(u16, u16)>,
    pub routes: Vec<(u16, u16)>,
    pub maneuver_flags: u16,
    pub can_arm: bool,
}

#[derive(Default)]
pub struct Control {
    pub actions: BTreeMap<Request, Vec<Action>>,
    pub switches: BTreeSet<Platoon>,
    pub maneuver_requests: BTreeMap<Request, Vec<Platoon>>,
    pub platoon_rules: Vec<PlatoonRule>,
    /// Squads that can participate in follow-driven migration (Halo 1 flag 0x20).
    pub automatic_migration: BTreeSet<u16>,
}

#[derive(Clone, Copy)]
enum Selection {
    Encounter,
    Platoon(u8),
    Squad(u8),
}

fn decode(source: &H1Ai, ai: u32) -> Option<(u16, &Encounter, Selection)> {
    let index = ai as u16;
    let encounter = source.encounters.get(usize::from(index))?;
    let subindex = (ai >> 16) as u8;
    let selection = match ai >> 30 {
        0 => Selection::Encounter,
        1 if usize::from(subindex) < encounter.platoons.len() => Selection::Platoon(subindex),
        2 if usize::from(subindex) < encounter.squads.len() => Selection::Squad(subindex),
        _ => return None,
    };
    Some((index, encounter, selection))
}

/// Halo 1 resolves a squad argument to its platoon for these orders.
fn platoons(encounter: &Encounter, selection: Selection) -> impl Iterator<Item = u8> {
    let selected = match selection {
        Selection::Encounter => None,
        Selection::Platoon(i) => Some(i16::from(i)),
        Selection::Squad(i) => Some(encounter.squads[usize::from(i)].platoon),
    };
    (0..encounter.platoons.len())
        .filter(move |&i| selected.is_none_or(|p| p == i as i16))
        .filter_map(|i| u8::try_from(i).ok())
}

fn members(encounter: &Encounter, platoon: u8) -> impl Iterator<Item = &Squad> {
    encounter
        .squads
        .iter()
        .filter(move |q| q.platoon == i16::from(platoon))
}

fn maneuver_target(encounter: &Encounter, squad: &Squad) -> Option<usize> {
    usize::try_from(squad.maneuver_to)
        .ok()
        .filter(|&i| i < encounter.squads.len())
}

/// The additional empty squads needed for scripted maneuvers. The caller
/// still applies the runtime's total squad capacity.
pub fn maneuver_targets(source: &H1Ai, requests: &[Request]) -> BTreeSet<String> {
    let explicit = requests
        .iter()
        .filter(|r| r.operation == Operation::Maneuver)
        .filter_map(|r| decode(source, r.reference))
        .flat_map(|(_, encounter, selection)| {
            platoons(encounter, selection)
                .flat_map(move |platoon| members(encounter, platoon))
                .filter_map(|q| maneuver_target(encounter, q))
                .map(|i| squad_name(encounter, &encounter.squads[i]).to_ascii_lowercase())
        });
    let triggered = source.encounters.iter().flat_map(|encounter| {
        encounter.squads.iter().filter_map(move |squad| {
            let platoon = encounter
                .platoons
                .get(usize::try_from(squad.platoon).ok()?)?;
            if platoon.maneuver.condition == 0 {
                return None;
            }
            let i = maneuver_target(encounter, squad)?;
            Some(squad_name(encounter, &encounter.squads[i]).to_ascii_lowercase())
        })
    });
    explicit.chain(triggered).collect()
}

/// Follow-driven migration uses even unoccupied squads as destinations.
pub fn automatic_migration_targets(source: &H1Ai) -> BTreeSet<String> {
    source
        .encounters
        .iter()
        .flat_map(|encounter| {
            encounter
                .squads
                .iter()
                .filter(|squad| squad.flags & 0x20 != 0)
                .map(|squad| squad_name(encounter, squad).to_ascii_lowercase())
        })
        .collect()
}

fn squad_name(encounter: &Encounter, squad: &Squad) -> String {
    format!("{}/{}", encounter.name, squad.name)
}

fn converted_indices(source: &H1Ai, plan: &Plan) -> Vec<Vec<Option<u16>>> {
    source
        .encounters
        .iter()
        .enumerate()
        .map(|(ei, encounter)| {
            encounter
                .squads
                .iter()
                .map(|squad| {
                    let name = squad_name(encounter, squad);
                    let mut matches = plan.ai.squads.iter().enumerate().filter(|(i, q)| {
                        q.initial_zone == ei as i16 && plan.squad_names[*i].eq_ignore_ascii_case(&name)
                    });
                    let (i, _) = matches.next()?;
                    // Ambiguous source names cannot identify the original squad.
                    matches.next().is_none().then_some(i as u16)
                })
                .collect()
        })
        .collect()
}

/// Create the Halo 2 orders the scripts can select and keep every action's
/// exact converted squad index. A maneuver with a stripped destination is
/// kept unresolved because sending its actors elsewhere changes the level.
pub fn prepare(source: &H1Ai, plan: &mut Plan, opts: &Options, requests: &[Request]) -> Control {
    let indices = converted_indices(source, plan);
    let mut control = Control::default();
    let mut orders = BTreeMap::new();
    let mut follow_orders = BTreeMap::new();
    let radii: BTreeMap<_, _> = requests
        .iter()
        .filter(|request| request.operation == Operation::FollowDistance)
        .filter_map(|request| Some((request.reference as u16, f32::from_bits(request.target?))))
        .collect();
    for request in requests.iter().copied().collect::<BTreeSet<_>>() {
        let Some((encounter_index, encounter, selection)) = decode(source, request.reference)
        else {
            continue;
        };
        let ids = &indices[usize::from(encounter_index)];
        if request.operation == Operation::Maneuver {
            control.maneuver_requests.insert(
                request,
                platoons(encounter, selection)
                    .map(|index| Platoon {
                        encounter: encounter_index,
                        index,
                    })
                    .collect(),
            );
        }
        let actions = if request.operation == Operation::Spawn {
            // Halo 1's counted spawn accepts a squad, or picks the first
            // member of a platoon. An encounter is not a spawn selection.
            let source_index = match selection {
                Selection::Encounter => None,
                Selection::Squad(i) => Some(usize::from(i)),
                Selection::Platoon(i) => encounter
                    .squads
                    .iter()
                    .position(|q| q.platoon == i16::from(i)),
            };
            source_index
                .and_then(|i| ids[i])
                .map(|i| vec![Action::Spawn(i)])
        } else if request.operation == Operation::TimerExpire {
            Some(
                encounter
                    .squads
                    .iter()
                    .enumerate()
                    .filter(|(index, squad)| match selection {
                        Selection::Encounter => true,
                        Selection::Platoon(i) => squad.platoon == i16::from(i),
                        Selection::Squad(i) => *index == usize::from(i),
                    })
                    .filter_map(|(index, squad)| {
                        Some(Action::ExpireTimer {
                            squad: ids[index]?,
                            reveal_players: squad.flags & 0x10 != 0,
                        })
                    })
                    .collect(),
            )
        } else if request.operation == Operation::FollowDistance {
            // The source analysis proved this assignment is folded into
            // the subsequent follow order.
            Some(Vec::new())
        } else if matches!(
            request.operation,
            Operation::FollowPlayers | Operation::FollowAi | Operation::FollowDisable
        ) {
            follow_actions(
                source,
                plan,
                opts,
                &mut follow_orders,
                &indices,
                encounter_index,
                encounter,
                request,
                radii.get(&encounter_index).copied().unwrap_or(4.0),
            )
        } else if request.operation == Operation::Migrate {
            migration_actions(
                source,
                &indices,
                encounter_index,
                encounter,
                selection,
                request,
            )
        } else {
            platoon_actions(
                plan,
                opts,
                &mut orders,
                ids,
                encounter_index,
                encounter,
                selection,
                request.operation,
            )
        };
        if let Some(actions) = actions {
            control
                .switches
                .extend(actions.iter().filter_map(|action| match action {
                    Action::Enable(platoon) => Some(*platoon),
                    _ => None,
                }));
            control.actions.insert(request, actions);
        }
    }
    prepare_platoon_rules(source, plan, opts, &indices, &mut orders, &mut control);
    for (ei, (encounter, ids)) in source.encounters.iter().zip(&indices).enumerate() {
        for (squad, &index) in encounter.squads.iter().zip(ids) {
            if let Some(index) = index
                && squad.flags & 0x20 != 0
            {
                control.automatic_migration.insert(index);
                if plan.ai.squads[usize::from(index)].initial_order < 0
                    && let Some(order) =
                        build_order(plan, opts, ei as u16, squad, index, Operation::Attack)
                {
                    plan.ai.squads[usize::from(index)].initial_order = order as i16;
                }
            }
        }
    }
    control
}

fn prepare_platoon_rules(
    source: &H1Ai,
    plan: &mut Plan,
    opts: &Options,
    indices: &[Vec<Option<u16>>],
    orders: &mut BTreeMap<(u16, Operation), u16>,
    control: &mut Control,
) {
    let explicit: BTreeSet<_> = control
        .maneuver_requests
        .values()
        .flatten()
        .copied()
        .collect();
    for (ei, (encounter, ids)) in source.encounters.iter().zip(indices).enumerate() {
        let members = |subject: Option<i16>| {
            encounter
                .squads
                .iter()
                .zip(ids)
                .filter(move |(q, _)| subject.is_none_or(|id| q.platoon == id))
                .filter_map(|(q, &id)| Some((q, id?)))
        };
        let trigger = |trigger: PlatoonTrigger| {
            if !(1..=9).contains(&trigger.condition) {
                return None;
            }
            let subject = usize::try_from(trigger.subject)
                .ok()
                .filter(|&i| i < encounter.platoons.len())
                .map(|i| i as i16);
            Some(Trigger {
                condition: trigger.condition,
                squads: members(subject).map(|(_, id)| id).collect(),
            })
        };
        for index in platoons(encounter, Selection::Encounter) {
            let source = &encounter.platoons[usize::from(index)];
            let platoon = Platoon {
                encounter: ei as u16,
                index,
            };
            let initial = if source.flags & 4 != 0 {
                Operation::Defend
            } else {
                Operation::Attack
            };
            let changed = if initial == Operation::Attack {
                Operation::Defend
            } else {
                Operation::Attack
            };
            let change = trigger(source.change);
            let maneuver = trigger(source.maneuver);
            let can_arm = maneuver.is_some() || explicit.contains(&platoon);
            let mut rule = PlatoonRule {
                platoon,
                members: Vec::new(),
                change,
                maneuver,
                orders: Vec::new(),
                routes: Vec::new(),
                maneuver_flags: source.flags as u16 & 3,
                can_arm,
            };
            for (q, id) in members(Some(i16::from(index))) {
                rule.members.push(id);
                if rule.change.is_some() || initial == Operation::Defend {
                    // Cache the attacking order before installing a defensive default.
                    let attack =
                        cached_order(plan, opts, orders, ei as u16, q, id, Operation::Attack);
                    let defend =
                        cached_order(plan, opts, orders, ei as u16, q, id, Operation::Defend);
                    if let (Some(attack), Some(defend)) = (attack, defend) {
                        plan.ai.squads[usize::from(id)].initial_order =
                            if initial == Operation::Attack {
                                attack
                            } else {
                                defend
                            } as i16;
                        if rule.change.is_some() {
                            rule.orders.push((
                                id,
                                if changed == Operation::Attack {
                                    attack
                                } else {
                                    defend
                                },
                            ));
                        }
                    }
                }
                if can_arm
                    && let Some(target) = maneuver_target(encounter, q)
                    && let Some(to) = ids[target]
                    && id != to
                {
                    if plan.ai.squads[usize::from(to)].initial_order < 0
                        && let Some(order) = build_order(
                            plan,
                            opts,
                            ei as u16,
                            &encounter.squads[target],
                            to,
                            Operation::Attack,
                        )
                    {
                        plan.ai.squads[usize::from(to)].initial_order = order as i16;
                    }
                    rule.routes.push((id, to));
                }
            }
            if !rule.members.is_empty() && (can_arm || rule.change.is_some()) {
                control.platoon_rules.push(rule);
            }
        }
    }
}

fn cached_order(
    plan: &mut Plan,
    opts: &Options,
    orders: &mut BTreeMap<(u16, Operation), u16>,
    encounter: u16,
    squad: &Squad,
    id: u16,
    operation: Operation,
) -> Option<u16> {
    use std::collections::btree_map::Entry;
    Some(match orders.entry((id, operation)) {
        Entry::Occupied(entry) => *entry.get(),
        Entry::Vacant(entry) => {
            *entry.insert(build_order(plan, opts, encounter, squad, id, operation)?)
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn platoon_actions(
    plan: &mut Plan,
    opts: &Options,
    orders: &mut BTreeMap<(u16, Operation), u16>,
    ids: &[Option<u16>],
    encounter_index: u16,
    encounter: &Encounter,
    selection: Selection,
    operation: Operation,
) -> Option<Vec<Action>> {
    let mut actions = Vec::new();
    for index in platoons(encounter, selection) {
        let platoon = Platoon {
            encounter: encounter_index,
            index,
        };
        if operation == Operation::ManeuverEnable {
            actions.push(Action::Enable(platoon));
            continue;
        }
        for (i, squad) in encounter.squads.iter().enumerate() {
            if squad.platoon != i16::from(index) {
                continue;
            }
            let Some(from) = ids[i] else {
                continue;
            };
            if operation == Operation::Maneuver {
                let Some(target) = maneuver_target(encounter, squad) else {
                    continue;
                };
                let to = ids[target]?;
                if to != from {
                    if plan.ai.squads[usize::from(to)].initial_order < 0 {
                        let order = build_order(
                            plan,
                            opts,
                            encounter_index,
                            &encounter.squads[target],
                            to,
                            Operation::Attack,
                        )?;
                        plan.ai.squads[usize::from(to)].initial_order = order as i16;
                    }
                    actions.push(Action::Migrate {
                        from,
                        to,
                        platoon: Some(platoon),
                    });
                }
            } else {
                let order =
                    cached_order(plan, opts, orders, encounter_index, squad, from, operation)?;
                actions.push(Action::SetOrder { squad: from, order });
            }
        }
    }
    Some(actions)
}

// Move a destination's original actors first if it is itself a source.
// Cascading moves would otherwise migrate the same actor twice.
fn order_migrations(mut pending: Vec<Action>) -> Option<Vec<Action>> {
    let mut ordered = Vec::with_capacity(pending.len());
    while !pending.is_empty() {
        let next = pending.iter().position(|action| {
            let Action::Migrate { to, .. } = action else {
                return true;
            };
            !pending
                .iter()
                .any(|other| matches!(other, Action::Migrate { from, .. } if from == to))
        })?;
        ordered.push(pending.remove(next));
    }
    Some(ordered)
}

fn build_order(
    plan: &mut Plan,
    opts: &Options,
    encounter: u16,
    source: &Squad,
    squad: u16,
    operation: Operation,
) -> Option<u16> {
    if operation == Operation::Attack
        && let Ok(index) = u16::try_from(plan.ai.squads[usize::from(squad)].initial_order)
    {
        return Some(index);
    }
    let mask = if operation == Operation::Defend {
        source.groups[3]
    } else {
        source.attacking()
    };
    let zone = &plan.ai.zones[usize::from(encounter)];
    let mut primary_areas = Vec::new();
    for (index, area) in zone.areas.iter().enumerate() {
        let Some(letter) = area
            .name
            .as_bytes()
            .first()
            .and_then(|l| l.checked_sub(b'a'))
        else {
            continue;
        };
        if letter < 26 && mask & (1 << letter) != 0 {
            primary_areas.push(AreaRef {
                kind: opts.area_kind,
                zone: encounter as i16,
                area: index as i16,
            });
        }
    }
    if primary_areas.is_empty() {
        primary_areas.extend((0..zone.areas.len()).map(|i| AreaRef {
            kind: opts.area_kind,
            zone: encounter as i16,
            area: i as i16,
        }));
    }
    let index = i16::try_from(plan.ai.orders.len()).ok()?;
    plan.ai.orders.push(Order {
        name: format!(
            "h1_{}_{squad}",
            if operation == Operation::Defend {
                "defend"
            } else {
                "attack"
            }
        ),
        style: opts.order_style,
        flags: 0,
        force_combat_status: 0,
        entry_script: String::new(),
        entry_script_index: -1,
        follow_squad: -1,
        follow_radius: 0.0,
        primary_areas,
        secondary_areas: Vec::new(),
        endings: 0,
    });
    Some(index as u16)
}

/// Halo 2 uses bit 4 to look up the nearest player for this squad, and bit
/// 5 to get its order from the squad at +0x4e (engine 0x204450, 0x204ec0).
const FOLLOW_PLAYER: u32 = 1 << 4;
const FOLLOW_SQUAD: u32 = 1 << 5;

#[allow(clippy::too_many_arguments)]
fn follow_actions(
    source: &H1Ai,
    plan: &mut Plan,
    opts: &Options,
    orders: &mut BTreeMap<(u16, Operation, Option<u16>), u16>,
    indices: &[Vec<Option<u16>>],
    encounter_index: u16,
    encounter: &Encounter,
    request: Request,
    radius: f32,
) -> Option<Vec<Action>> {
    let target = if request.operation == Operation::FollowAi {
        let (to_encounter, _, Selection::Squad(to)) = decode(source, request.target?)? else {
            return None;
        };
        // Each retail call targets a squad in its own encounter. Those
        // calls also clear the leader's follow order before linking any
        // peers, so a change of leader can't form a native follow cycle.
        if to_encounter != encounter_index {
            return None;
        }
        Some(indices[usize::from(to_encounter)][usize::from(to)]?)
    } else {
        None
    };
    let mut actions = Vec::new();
    // The Halo 1 setters address the encounter even for a squad token.
    for (i, src) in encounter.squads.iter().enumerate() {
        let Some(squad) = indices[usize::from(encounter_index)][i] else {
            continue;
        };
        let order = if request.operation == Operation::FollowDisable || target == Some(squad) {
            build_order(plan, opts, encounter_index, src, squad, Operation::Attack)?
        } else if let Some(&order) = orders.get(&(squad, request.operation, target)) {
            order
        } else {
            let base = build_order(plan, opts, encounter_index, src, squad, Operation::Attack)?;
            let mut order = plan.ai.orders[usize::from(base)].clone();
            order.name = format!("h1_follow_{squad}_{}", target.map_or(-1, i32::from));
            if let Some(target) = target {
                order.flags |= FOLLOW_SQUAD;
                order.follow_squad = target as i16;
            } else {
                order.flags |= FOLLOW_PLAYER;
            }
            order.follow_radius = radius;
            let index = i16::try_from(plan.ai.orders.len()).ok()? as u16;
            plan.ai.orders.push(order);
            orders.insert((squad, request.operation, target), index);
            index
        };
        actions.push(Action::SetOrder { squad, order });
    }
    // A formerly followed squad must stop following before its peers
    // adopt its order. The native resolver follows order links recursively.
    actions.sort_by_key(|a| !matches!(a, Action::SetOrder { squad, .. } if Some(*squad) == target));
    Some(actions)
}

fn selected_squads(encounter: &Encounter, selection: Selection) -> impl Iterator<Item = usize> {
    encounter
        .squads
        .iter()
        .enumerate()
        .filter_map(move |(index, q)| {
            match selection {
                Selection::Encounter => true,
                Selection::Platoon(i) => q.platoon == i16::from(i),
                Selection::Squad(i) => index == usize::from(i),
            }
            .then_some(index)
        })
}

fn migration_actions(
    source: &H1Ai,
    indices: &[Vec<Option<u16>>],
    from_encounter: u16,
    from: &Encounter,
    from_selection: Selection,
    request: Request,
) -> Option<Vec<Action>> {
    let (to_encounter, to, to_selection) = decode(source, request.target?)?;
    // Ordinary encounter/squad -> squad migration works in Halo 2. Leave
    // it on its existing call path, including its existing hook handling.
    if matches!(to_selection, Selection::Squad(_))
        && !matches!(from_selection, Selection::Platoon(_))
    {
        return None;
    }
    let from_ids = &indices[usize::from(from_encounter)];
    let to_ids = &indices[usize::from(to_encounter)];
    let candidates: Vec<_> = selected_squads(to, to_selection)
        .filter_map(|i| to_ids[i].map(|id| (i, id)))
        .collect();
    if candidates.is_empty() {
        return None;
    }
    let mut actions = Vec::new();
    for i in selected_squads(from, from_selection) {
        let Some(from_id) = from_ids[i] else {
            continue;
        };
        let source_actor = from.squads[i].actor;
        // Halo 1's 0x1402a0 chooses a compatible destination for each
        // source squad. Preserve variant matches, then character type and
        // scenario order where the converted roster has fewer candidates.
        let exact = |j: usize| to.squads[j].actor == source_actor && source_actor >= 0;
        let actor_type = |actor| {
            usize::try_from(actor)
                .ok()
                .and_then(|index| source.actor_of(index))
                .map(|actr| actr.kind)
        };
        let kind = actor_type(source_actor);
        let &(j, to_id) = candidates
            .iter()
            .find(|(j, _)| from_encounter == to_encounter && *j == i)
            .or_else(|| candidates.iter().find(|(j, _)| exact(*j)))
            .or_else(|| {
                candidates
                    .iter()
                    .find(|(j, _)| kind.is_some() && actor_type(to.squads[*j].actor) == kind)
            })
            .unwrap_or(&candidates[0]);
        if from_encounter != to_encounter || i != j {
            actions.push(Action::Migrate {
                from: from_id,
                to: to_id,
                platoon: None,
            });
        }
    }
    order_migrations(actions)
}
