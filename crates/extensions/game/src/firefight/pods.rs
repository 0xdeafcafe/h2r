//! Player drop pods (docs/game/firefight.md, "Drop pods"): a player's
//! spawn, layered with a fall in one of the kit's pods.
//!
//! **The rule.** The pod is layered on a spawn the engine has already made.
//! The engine spawns the player at a starting location as it always does;
//! the host then puts a parked pod at the top of that start's fall
//! (`ff_pfall_NN`), seats the new unit in it, flies the pod down in
//! [`FALL_TICKS`], and ejects the rider [`LAND_TICKS`] after it lands. Every
//! failure leaves the player standing at the start: a start with no fall, a
//! column something blocks, no free pod, a seat the engine refuses, a pod
//! that never lands (the watchdog).
//!
//! [`PodDrops`] is the host's state for it, a pure machine the runtime
//! steps after each tick over what the game shows ([`Seen`], [`PodSeen`])
//! and that answers with [`Act`]s the runtime makes at the next tick
//! boundary. The engine side ([`seat`], [`eject`], [`drop`]) is below it.
//! Before the spawn, [`spawn_pick`] prefers the team's open starts
//! (`super::super::rules::world::spawns::open_filter`).
//!
//! The machine runs where the simulation of record does (the host), and
//! its acts are engine calls, not host log entries: a film does not replay
//! a drop.

use std::collections::BTreeMap;

use guest_layout::entry::{Cx, GuestCall};
use guest_layout::view::objects;
use guest_layout::{DatumHandle, GuestMemory, points};

use super::guest::{KitLayout, PodFall, call};
use super::ship::{self, Launch, ShipPut};

/// The fall: 2.5 s at 30 ticks a second.
pub const FALL_TICKS: u32 = 75;
/// The rider is ejected this long after the pod lands.
pub const LAND_TICKS: u32 = 15;
/// A landed pod stays this long, then goes back to its parking place.
pub const WRECK_TICKS: u32 = 300;
/// A pod that has not landed this long after its fall should have is
/// given up on: the rider is ejected and the pod parked.
pub const WATCHDOG_TICKS: u32 = 30;
/// Players spawned in one tick drop this far apart.
pub const DROP_GAP: u32 = 10;
/// A spawned unit belongs to a start's fall when it is this near the
/// fall's ground, horizontally.
pub const SPOT: f32 = 2.0;
/// A pod has landed when it is within this of the fall's ground.
pub const ARRIVE: f32 = 1.0;
/// The first sequence number of a pod's engine call: the pod's index
/// follows. The runtime reads a refusal by it.
pub const SEQ: u32 = 0xf000_0000;

/// Why a player spawned on foot, or a drop stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reason {
    /// The spawn is at no start the kit gave a fall.
    NoFall,
    /// The line of sight from the top to the ground is blocked.
    Blocked,
    /// Every pod is in use or gone.
    NoFreePod,
    /// The engine seated no one.
    SeatRefused,
    /// The pod left the world.
    PodGone,
    /// The pod never landed (the watchdog ejected the rider).
    NeverLanded,
    /// The player died or left before the pod dropped.
    Gone,
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Reason::NoFall => "no_fall",
            Reason::Blocked => "blocked",
            Reason::NoFreePod => "no_free_pod",
            Reason::SeatRefused => "seat_refused",
            Reason::PodGone => "pod_gone",
            Reason::NeverLanded => "never_landed",
            Reason::Gone => "gone",
        }
    }
}

/// One player, read after the tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seen {
    pub slot: u8,
    pub alive: bool,
    pub position: Option<[f32; 3]>,
    /// The unit's datum.
    pub unit: Option<u32>,
}

/// One pod of the pool, read after the tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PodSeen {
    /// The pod's live datum; none when it left the world.
    pub object: Option<u32>,
    pub position: Option<[f32; 3]>,
}

/// What the machine asks of the next tick boundary.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    /// Check the column, put the pod at the top and seat the unit in it;
    /// on success the flight begins ([`PodDrops::dropped`]).
    Drop { pod: usize, object: u32, unit: u32, fall: PodFall },
    /// The rider leaves the pod.
    Eject { unit: u32 },
    /// The pod goes back to its parking place.
    Park { object: u32, at: [f32; 3] },
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Slot {
    Free,
    /// Taken for a spawn that drops at `due`.
    Reserved { player: u8, unit: u32, fall: PodFall, due: u32 },
    /// The drop call is out.
    Dropping { player: u8, unit: u32, fall: PodFall },
    Falling { player: u8, unit: u32, fall: PodFall, started: u32, landed: Option<u32> },
    /// Landed and emptied; parks at the tick.
    Wreck { until: u32 },
}

/// Counters for `firefight.state`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Players dropped by pod (the call succeeded).
    pub spawns: u32,
    /// Players who spawned on foot, or whose drop stopped, by reason.
    pub fallbacks: BTreeMap<Reason, u32>,
}

/// The pods of one match.
#[derive(Clone, Debug)]
pub struct PodDrops {
    enabled: bool,
    falls: Vec<(u16, PodFall)>,
    park: [f32; 3],
    slots: Vec<Slot>,
    alive: BTreeMap<u8, bool>,
    counts: Counts,
}

impl PodDrops {
    /// The machine for `kit` with `pods` set on or off: no pods in the kit
    /// or no fall is off however it is set ([`row`]).
    pub fn new(kit: &KitLayout, on: bool) -> Self {
        let enabled = on && matches!(row(Some(kit)), Row::Shown);
        Self {
            enabled,
            falls: kit.player_falls.clone(),
            park: kit.player_falls.first().map_or([0.0; 3], |(_, f)| f.top),
            slots: vec![Slot::Free; kit.player_pods.len()],
            alive: BTreeMap::new(),
            counts: Counts::default(),
        }
    }

    /// Where the pods park.
    pub fn park(&self) -> [f32; 3] {
        self.park
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn counts(&self) -> &Counts {
        &self.counts
    }

    /// The starts a spawn is preferred at: the numbers of the kit's open
    /// starts, none when the pods are off.
    pub fn open_starts(&self) -> Vec<u16> {
        if self.enabled {
            self.falls.iter().map(|(n, _)| *n).collect()
        } else {
            Vec::new()
        }
    }

    fn count(&mut self, reason: Reason) {
        *self.counts.fallbacks.entry(reason).or_default() += 1;
    }

    /// The fall a spawn at `at` belongs to.
    fn fall_at(&self, at: [f32; 3]) -> Option<PodFall> {
        self.falls
            .iter()
            .map(|(_, f)| *f)
            .filter(|f| flat(f.ground, at) <= SPOT)
            .min_by(|a, b| {
                let d = |f: &PodFall| flat(f.ground, at);
                d(a).total_cmp(&d(b))
            })
    }

    /// After a tick: advance the pods in the air, take a pod for each
    /// player who has just spawned, and answer with what the next boundary
    /// does. `pods` is the pool in `ff_ppod_NN` order.
    pub fn step(&mut self, tick: u32, players: &[Seen], pods: &[PodSeen]) -> Vec<Act> {
        let mut acts = Vec::new();
        if !self.enabled {
            return acts;
        }
        let by_slot = |slot: u8| players.iter().find(|p| p.slot == slot);
        for i in 0..self.slots.len() {
            let seen = pods.get(i).copied().unwrap_or(PodSeen { object: None, position: None });
            match self.slots[i] {
                Slot::Free | Slot::Dropping { .. } => {}
                Slot::Reserved { player, unit, due, fall } => {
                    let here = by_slot(player).is_some_and(|p| p.alive && p.unit == Some(unit));
                    if !here || seen.object.is_none() {
                        self.slots[i] = Slot::Free;
                        self.count(if here { Reason::PodGone } else { Reason::Gone });
                    } else if let (true, Some(object)) = (tick >= due, seen.object) {
                        self.slots[i] = Slot::Dropping { player, unit, fall };
                        acts.push(Act::Drop {
                            pod: i,
                            object,
                            unit,
                            fall,
                        });
                    }
                }
                Slot::Falling { player, unit, fall, started, landed } => {
                    let here = by_slot(player).is_some_and(|p| p.alive && p.unit == Some(unit));
                    let landed = landed.or_else(|| {
                        seen.position
                            .filter(|p| distance(*p, fall.ground) <= ARRIVE)
                            .map(|_| tick)
                    });
                    if !here {
                        // The rider died or left: nobody to eject.
                        self.slots[i] = Slot::Wreck { until: tick + WRECK_TICKS };
                    } else if seen.object.is_none() {
                        acts.push(Act::Eject { unit });
                        self.count(Reason::PodGone);
                        self.slots[i] = Slot::Free;
                    } else if landed.is_some_and(|t| tick >= t + LAND_TICKS) {
                        acts.push(Act::Eject { unit });
                        self.slots[i] = Slot::Wreck { until: tick + WRECK_TICKS };
                    } else if landed.is_none() && tick > started + FALL_TICKS + WATCHDOG_TICKS {
                        // The pod never landed: the rider is let out where
                        // it is and the pod goes home.
                        acts.push(Act::Eject { unit });
                        acts.push(Act::Park { object: seen.object.expect("checked"), at: self.park });
                        self.count(Reason::NeverLanded);
                        self.slots[i] = Slot::Free;
                    } else {
                        self.slots[i] = Slot::Falling { player, unit, fall, started, landed };
                    }
                }
                Slot::Wreck { until } => {
                    if tick >= until {
                        if let Some(object) = seen.object {
                            acts.push(Act::Park { object, at: self.park });
                        }
                        self.slots[i] = Slot::Free;
                    }
                }
            }
        }
        // Players who have just spawned, in slot order.
        let mut batch = 0;
        for p in players {
            let was = self.alive.insert(p.slot, p.alive).unwrap_or(false);
            let (true, false, Some(at), Some(unit)) = (p.alive, was, p.position, p.unit) else {
                continue;
            };
            let Some(fall) = self.fall_at(at) else {
                self.count(Reason::NoFall);
                continue;
            };
            let free = (0..self.slots.len())
                .find(|&i| self.slots[i] == Slot::Free && pods.get(i).is_some_and(|s| s.object.is_some()));
            let Some(i) = free else {
                self.count(Reason::NoFreePod);
                continue;
            };
            self.slots[i] = Slot::Reserved {
                player: p.slot,
                unit,
                fall,
                due: tick + DROP_GAP * batch,
            };
            if batch == 0 {
                let Slot::Reserved { player, unit, due, fall } = self.slots[i] else {
                    unreachable!()
                };
                // The first of a tick drops at the next boundary.
                self.slots[i] = Slot::Dropping { player, unit, fall };
                let _ = due;
                acts.push(Act::Drop {
                    pod: i,
                    object: pods[i].object.expect("chosen for it"),
                    unit,
                    fall,
                });
            }
            batch += 1;
        }
        acts
    }

    /// The drop call at pod `pod` succeeded at `tick`: the flight began.
    pub fn dropped(&mut self, pod: usize, tick: u32) {
        if let Some(slot @ Slot::Dropping { .. }) = self.slots.get_mut(pod) {
            let Slot::Dropping { player, unit, fall } = *slot else {
                unreachable!()
            };
            *slot = Slot::Falling { player, unit, fall, started: tick, landed: None };
            self.counts.spawns += 1;
        }
    }

    /// The drop call at pod `pod` failed for `reason`: the player stands
    /// at the start.
    pub fn failed(&mut self, pod: usize, reason: Reason) {
        if let Some(slot @ Slot::Dropping { .. }) = self.slots.get_mut(pod) {
            *slot = Slot::Free;
            self.count(reason);
        }
    }

    /// The flight of a drop, for the match's flights.
    pub fn launch(object: u32, fall: PodFall) -> Launch {
        Launch {
            ship: object,
            path: vec![fall.top, fall.ground],
            ticks: FALL_TICKS,
        }
    }
}

/// The horizontal distance.
fn flat(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]) * (a[0] - b[0]) + (a[1] - b[1]) * (a[1] - b[1])).sqrt()
}

fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|i| (a[i] - b[i]) * (a[i] - b[i])).sum::<f32>().sqrt()
}

/// Whether the Drop Pods row is offered, and why not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Shown,
    Hidden(&'static str),
}

/// The row for a map: hidden when the kit has no pod or no start under
/// open sky.
pub fn row(kit: Option<&KitLayout>) -> Row {
    match kit {
        None => Row::Hidden("the map has no Firefight kit"),
        Some(k) if k.player_pods.is_empty() => Row::Hidden("the map's kit carries no drop pod"),
        Some(k) if k.player_falls.is_empty() => Row::Hidden("no starting location is under open sky"),
        Some(_) => Row::Shown,
    }
}

/// The starts a spawn for `team` prefers: the open ones of the kit when
/// the pods are on, else none (the engine's own pick). `start_teams` is
/// each starting location's team.
pub fn spawn_pick(
    on: bool,
    kit: &KitLayout,
    start_teams: &[i16],
    team: i16,
) -> std::collections::BTreeSet<u32> {
    if !on || row(Some(kit)) != Row::Shown {
        return Default::default();
    }
    kit.player_falls
        .iter()
        .map(|(n, _)| u32::from(*n))
        .filter(|n| start_teams.get(*n as usize) == Some(&team))
        .collect()
}

/// The starting locations a spawn pick is kept to
/// (`PLAYER_SPAWN_LOCATION`, whose free-spot test `SPAWN_LOCATION_FREE`
/// answers "taken" for any other; docs/game/rules-engine.md, "Spawn
/// rules"): by guest address, as the chooser asks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub base: u32,
    pub stride: u32,
    pub live: std::collections::BTreeSet<u32>,
}

impl Pick {
    /// Whether the location at guest address `location` may be picked. An
    /// address that is no entry of the block passes: the test is the
    /// engine's then.
    pub fn passes(&self, location: u32) -> bool {
        let off = location.wrapping_sub(self.base);
        if self.stride == 0 || off % self.stride != 0 {
            return true;
        }
        self.live.contains(&(off / self.stride))
    }
}

/// A starting location's team designator (`+0x10`, `i16`).
const START_TEAM: u32 = 0x10;

/// The loaded scenario's starting locations' block and each one's team.
pub fn starts(m: &dyn GuestMemory) -> Option<(u32, u32, Vec<i16>)> {
    use forge_doc::BlockKind;
    let block = map::scenario_layout::block(BlockKind::StartingLocations);
    let scnr = m.u32(points::SCENARIO.base).ok().filter(|s| *s != 0)?;
    let n = (m.i32(scnr + block.offset).ok()?.max(0) as u32).min(forge_doc::CENSUS_ENTRY_LIMIT);
    let base = m.u32(scnr + block.offset + 4).ok().filter(|b| *b != 0)?;
    let teams = (0..n)
        .map(|i| m.u16(base + i * block.stride + START_TEAM).map(|t| t as i16))
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    Some((base, block.stride, teams))
}

/// The pick for a spawn of `team` on the loaded map: the team's open
/// starts when pods are on and the kit has any, else none (the engine's
/// own pick, untouched).
pub fn pick(m: &dyn GuestMemory, on: bool, kit: &KitLayout, team: i16) -> Option<Pick> {
    let (base, stride, teams) = starts(m)?;
    let live = spawn_pick(on, kit, &teams, team);
    (!live.is_empty()).then_some(Pick { base, stride, live })
}

// --- The engine side -------------------------------------------------------

/// `0xe68c0`: a unit's action request, `ecx` the action, the unit on the
/// stack; action `0x1e` takes a unit out of its vehicle
/// (`crates/extensions/agent/src/sim/calls.rs`).
const UNIT_ACTION: u32 = 0x000e_68c0;
const EXIT_VEHICLE: u32 = 0x1e;
/// The tag table global (`[4e3b44]`: 16-byte entries, the definition at
/// `+8`).
const TAG_TABLE: u32 = 0x004e_3b44;
/// A vehicle definition's seats block and its elements' size and label
/// (`docs/game/bots.md`, "seats").
const SEATS: u32 = 0x1c8;
const SEAT_LABEL: u32 = 0x04;

/// The label (string ID) of the pod's first seat, which the entry routine
/// matches. None when the object is no vehicle or its tag has no seat.
pub fn seat_label(m: &dyn GuestMemory, pod: u32) -> Option<u32> {
    let (kind, body) = objects::get(m, DatumHandle::from_raw(pod)?).ok()??;
    if kind != objects::VEHICLE {
        return None;
    }
    let definition = body.definition(m).ok()??;
    let table = m.u32(TAG_TABLE).ok()?;
    let tag = m.u32(table.checked_add((definition.index() as u32) * 16 + 8)?).ok()?;
    if tag == 0 || m.u32(tag + SEATS).ok()? == 0 {
        return None;
    }
    let seats = m.u32(tag + SEATS + 4).ok()?;
    m.u32(seats.checked_add(SEAT_LABEL)?).ok()
}

/// A unit's animation state sits at the signed word at `+0x12a` from the
/// unit; the state's graph (`jmad`) datum is at `+0x68`; a graph's modes
/// block is at `+0x34`, 20 bytes a mode, its label (a string ID) first.
/// A unit whose byte at `+0xaa` is 1 takes any seat.
const ANIMATION_STATE: u32 = 0x12a;
const STATE_GRAPH: u32 = 0x68;
const GRAPH_MODES: u32 = 0x34;
const MODE_STRIDE: u32 = 20;
const ANY_SEAT: u32 = 0xaa;

/// Whether the unit object at `unit` passes the engine's seat test for a
/// seat labelled `label` (`0xc8200`, which `UNIT_ENTER_VEHICLE` asks
/// before it requests the entry, and quietly seats no one when it fails):
/// its animation graph has a mode of that label (`0x1cb920`). None when
/// the unit or its graph does not read.
pub fn takes_seat(m: &dyn GuestMemory, unit: u32, label: u32) -> Option<bool> {
    if m.u8(unit.checked_add(ANY_SEAT)?).ok()? == 1 {
        return Some(true);
    }
    let state = unit.wrapping_add(m.u16(unit.checked_add(ANIMATION_STATE)?).ok()? as i16 as i32 as u32);
    let graph = m.u32(state.checked_add(STATE_GRAPH)?).ok()?;
    if graph == u32::MAX {
        return Some(false);
    }
    let table = m.u32(TAG_TABLE).ok()?;
    let tag = m.u32(table.checked_add((graph & 0xffff) * 16 + 8)?).ok()?;
    if tag == 0 {
        return None;
    }
    let count = m.u32(tag + GRAPH_MODES).ok()?;
    let modes = m.u32(tag + GRAPH_MODES + 4).ok()?;
    for i in 0..count.min(0x400) {
        if m.u32(modes.checked_add(i * MODE_STRIDE)?).ok()? == label {
            return Some(true);
        }
    }
    Some(false)
}

/// The object address of the live object `handle`.
fn object_address(m: &dyn GuestMemory, handle: u32) -> Option<u32> {
    let header = objects::array(m).ok()??.get(m, DatumHandle::from_raw(handle)?).ok()??;
    header.body(m).ok().filter(|b| *b != 0)
}

fn parent_of(m: &dyn GuestMemory, unit: u32) -> Option<u32> {
    let (_, body) = objects::get(m, DatumHandle::from_raw(unit)?).ok()??;
    body.parent(m).ok()?.map(|p| p.raw())
}

/// Put the pod at the top of its fall and seat the unit in its first
/// seat, after the column's line of sight is checked. On a refusal the
/// pod is back at `park` and the error says which kind: `blocked: ...` or
/// `seat refused: ...`. The player is left standing at the start.
pub async fn drop(
    cx: &Cx,
    scratch: u32,
    object: u32,
    unit: u32,
    fall: PodFall,
    park: [f32; 3],
) -> anyhow::Result<()> {
    let clear = super::guest::line_of_sight(cx, scratch, fall.top, fall.ground, [object, unit])
        .await
        .unwrap_or(false);
    anyhow::ensure!(clear, "blocked: something is in the column from {:?} to {:?}", fall.top, fall.ground);
    let down = [0.0, 0.0, -1.0];
    ship::put_ship(cx, ShipPut { object, at: fall.top, forward: down }).await?;
    let sit = async {
        let label = cx
            .read(|m| seat_label(m, object))
            .ok_or_else(|| anyhow::anyhow!("the pod {object:#010x} has no seat"))?;
        // The engine seats no one, and says nothing, when the rider's
        // animation graph has no mode for the seat: name that refusal.
        let takes = cx.read(|m| takes_seat(m, object_address(m, unit)?, label));
        anyhow::ensure!(
            takes != Some(false),
            "unit {unit:#010x}'s animation graph has no mode for the pod's seat label {label:#010x}, so the engine's seat test (0xc8200) takes it in no seat"
        );
        call(
            cx,
            GuestCall::new(points::UNIT_ENTER_VEHICLE.base)
                .ebx(object)
                .args([unit, label]),
        )
        .await?;
        anyhow::ensure!(
            cx.read(|m| parent_of(m, unit)) == Some(object),
            "the engine seated unit {unit:#010x} in no seat of the pod: its graph has the seat's mode, so the entry it requested (action 0x1c) did not finish at once"
        );
        anyhow::Ok(())
    };
    if let Err(e) = sit.await {
        let _ = ship::put_ship(cx, ShipPut { object, at: park, forward: [1.0, 0.0, 0.0] }).await;
        anyhow::bail!("seat refused: {e}");
    }
    Ok(())
}

/// The unit leaves its vehicle (`0xe68c0`, action `0x1e`).
pub async fn eject(cx: &Cx, unit: u32) -> anyhow::Result<()> {
    anyhow::ensure!(
        cx.read(|m| objects::get(m, DatumHandle::from_raw(unit)?).ok()??.1.parent(m).ok()?).is_some(),
        "unit {unit:#010x} is not in a vehicle"
    );
    call(cx, GuestCall::new(UNIT_ACTION).ecx(EXIT_VEHICLE).args([unit])).await?;
    Ok(())
}

/// The pod goes back to its parking place.
pub async fn park(cx: &Cx, object: u32, at: [f32; 3]) -> anyhow::Result<()> {
    ship::put_ship(cx, ShipPut { object, at, forward: [1.0, 0.0, 0.0] }).await
}

#[cfg(test)]
mod tests;
