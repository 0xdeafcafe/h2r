use super::*;

use crate::coop_players::layout::OBJECT_PARENT;
use hs::h1_conversation::{ACTOR_CAPACITY, ROLE_STRIDE};

pub(super) const ACTORS: u32 = 0x004f_55f0;
pub(super) const UNIT_ACTOR: u32 = 0x12c;
pub(super) const ACTOR_FOCUS: u32 = 0x338;
const PROP_REFERENCES: u32 = 0x0050_2418;
const PROPS: u32 = 0x0050_241c;
const FIRST_PROP: u32 = 0x58;
const PROP_TYPE: u32 = 0x18;
const PROP_RECOGNITION: u32 = 0x27;
const NEXT_PROP: u32 = 0x2c;
const UNIT_TEAM: u32 = 0x138;
pub(super) const ACTOR_UNIT: u32 = 0x18;
pub(super) const ACTOR_COMMAND_STACK: u32 = 0x858;
pub(super) const COMMANDS: u32 = 0x0050_2408;
pub(super) const COMMAND_SCRIPT: u32 = 0x34;
pub(super) const COMMAND_NEXT: u32 = 0x3c;
pub(super) const DELETE_COMMAND: u32 = 0x0025_8540;
const OBJECT_ORIGIN: u32 = 0x64;
// The object location update at 0x000b866b mirrors the cluster into the header.
const HEADER_CLUSTER: u32 = 4;
const UNIT_AIM: u32 = 0x168;
const VEHICLE_DRIVER: u32 = 0x248;
const VEHICLE_GUNNER: u32 = 0x24c;
const TEAM_RELATIONSHIPS: u32 = 0x004f_55ec;
const ALLIANCES: u32 = 0xa4;
const RANDOM_SEED: u32 = 0x004e_7408;

#[derive(Clone, Copy)]
pub(super) struct Unit {
    handle: u32,
    parent: u32,
    pub(super) team: i16,
    pub(super) pos: [f32; 3],
    pub(super) cluster: i16,
    forward: [f32; 3],
}

impl Unit {
    pub(super) fn read(m: &dyn GuestMemory, handle: u32) -> Result<Option<Self>, MemFault> {
        Self::read_unit(m, handle, true)
    }

    pub(super) fn read_unit(
        m: &dyn GuestMemory,
        handle: u32,
        require_alive: bool,
    ) -> Result<Option<Self>, MemFault> {
        let Some(body) = object_address(m, handle)? else {
            return Ok(None);
        };
        if !matches!(m.u8(body + 0xaa)?, 0 | 1)
            || (require_alive && m.f32(body + crate::health::BODY_HEALTH)? <= 0.0)
        {
            return Ok(None);
        }
        let parent = m.u32(body + OBJECT_PARENT)?;
        let origin = object_address(m, parent)?.unwrap_or(body);
        let mut root = handle;
        let mut next = parent;
        for _ in 0..ACTOR_CAPACITY {
            let Some(ancestor) = object_address(m, next)? else {
                break;
            };
            root = next;
            next = m.u32(ancestor + OBJECT_PARENT)?;
        }
        let cluster = datum(m, points::OBJECTS.base, root)?
            .map_or(Ok(-1), |header| read_i16(m, header + HEADER_CLUSTER))?;
        Ok(Some(Self {
            handle,
            parent,
            team: read_i16(m, body + UNIT_TEAM)?,
            pos: m.vec3(origin + OBJECT_ORIGIN)?,
            cluster,
            forward: m.vec3(body + UNIT_AIM)?,
        }))
    }

    fn aim_toward(self, target: Self) -> f32 {
        let direction = std::array::from_fn::<_, 3, _>(|i| target.pos[i] - self.pos[i]);
        let length = distance(self.pos, target.pos);
        if length < 0.0001 {
            return 1.0;
        }
        self.forward
            .into_iter()
            .zip(direction)
            .map(|(a, b)| a * b)
            .sum::<f32>()
            / length
    }

    fn looks_at(self, target: Self) -> bool {
        self.aim_toward(target) >= std::f32::consts::FRAC_PI_6
    }
}

fn player_preference(player: Unit, unit: Unit) -> f32 {
    let d = distance(player.pos, unit.pos);
    if d > 30.0 {
        return 0.0;
    }
    let distance_bonus = 1.0 + ((15.0 - d) / 12.0).clamp(0.0, 1.0);
    let dot = player.aim_toward(unit);
    let aim_bonus = if dot >= std::f32::consts::FRAC_1_SQRT_2 {
        0.7 - 0.35 * (1.0 - dot.min(1.0)) / (1.0 - std::f32::consts::FRAC_1_SQRT_2)
    } else {
        0.0
    };
    distance_bonus + aim_bonus
}

pub(super) fn distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter()
        .zip(b)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        .sqrt()
}

fn focus(
    m: &dyn GuestMemory,
    anchor: Option<Unit>,
    must_look: bool,
) -> Result<Option<Unit>, MemFault> {
    let mut best = None;
    let mut best_score = f32::NEG_INFINITY;
    for player in crate::coop_players::players(m) {
        let Some(unit) = Unit::read(m, player.unit)? else {
            continue;
        };
        if let Some(anchor) = anchor
            && must_look
            && !unit.looks_at(anchor)
        {
            continue;
        }
        let score = anchor.map_or(0.0, |anchor| player_preference(unit, anchor));
        if score > best_score {
            best = Some(unit);
            best_score = score;
        }
    }
    Ok(best)
}

fn object_global(m: &dyn GuestMemory, index: u32) -> Result<u32, MemFault> {
    match global(m, index, OBJECT)? {
        Some(cell) => m.u32(cell),
        None => Ok(NONE),
    }
}

pub(super) fn actor_handle(m: &dyn GuestMemory, unit: u32) -> Result<Option<u32>, MemFault> {
    let Some(body) = object_address(m, unit)? else {
        return Ok(None);
    };
    if !matches!(m.u8(body + 0xaa)?, 0 | 1) {
        return Ok(None);
    }
    let handle = m.u32(body + UNIT_ACTOR)?;
    let Some(actor) = datum(m, ACTORS, handle)? else {
        return Ok(None);
    };
    Ok((m.u32(actor + ACTOR_UNIT)? == unit).then_some(handle))
}

pub(super) fn focus_object(m: &dyn GuestMemory, actor: u32) -> Result<u32, MemFault> {
    let Some(prop) = datum(m, PROP_REFERENCES, m.u32(actor + ACTOR_FOCUS)?)? else {
        return Ok(NONE);
    };
    m.u32(prop + 0x20)
}

pub(super) fn focusing_on_traitor(m: &dyn GuestMemory, actor: u32) -> Result<bool, MemFault> {
    let Some(reference) = datum(m, PROP_REFERENCES, m.u32(actor + ACTOR_FOCUS)?)? else {
        return Ok(false);
    };
    let Some(prop) = datum(m, PROPS, m.u32(reference + 8)?)? else {
        return Ok(false);
    };
    Ok(m.u8(prop + 0x23)? != 0)
}

pub(super) fn sees_hostile_group(m: &dyn GuestMemory, actor: u32) -> Result<bool, MemFault> {
    let mut count = 0;
    let mut id = m.u32(actor + FIRST_PROP)?;
    while let Some(prop) = datum(m, PROP_REFERENCES, id)? {
        if matches!(m.u16(prop + PROP_TYPE)?, 1 | 6) && m.u8(prop + PROP_RECOGNITION)? >= 2 {
            count += 1;
            if count == 5 {
                return Ok(true);
            }
        }
        let next = m.u32(prop + NEXT_PROP)?;
        if next == id {
            break;
        }
        id = next;
    }
    Ok(false)
}

pub(super) fn mounted(m: &dyn GuestMemory, body: u32) -> Result<bool, MemFault> {
    if m.u16(body + 0x1fc)? == u16::MAX {
        return Ok(false);
    }
    let Some(vehicle) = object_address(m, m.u32(body + OBJECT_PARENT)?)? else {
        return Ok(false);
    };
    Ok(m.u8(vehicle + 0xaa)? == 1)
}

/// The gunner owns a normal vehicle shot. A collision belongs to the driver.
/// Halo 2 resolves the same owner at 0x1c9580.
pub(super) fn damage_owner(
    m: &dyn GuestMemory,
    owner: u32,
    collision: bool,
) -> Result<Option<u32>, MemFault> {
    let Some(body) = object_address(m, owner)? else {
        return Ok(None);
    };
    match m.u8(body + 0xaa)? {
        0 => Ok(Some(owner)),
        1 => {
            if !collision {
                let gunner = m.u32(body + VEHICLE_GUNNER)?;
                if gunner != NONE {
                    return Ok(Some(gunner));
                }
            }
            let driver = m.u32(body + VEHICLE_DRIVER)?;
            Ok(Some(if driver == NONE { owner } else { driver }))
        }
        _ => Ok(None),
    }
}

pub(super) fn team(m: &dyn GuestMemory, unit: u32) -> Result<Option<i16>, MemFault> {
    object_address(m, unit)?
        .map(|body| read_i16(m, body + UNIT_TEAM))
        .transpose()
}

pub(super) fn is_player(m: &dyn GuestMemory, unit: u32) -> Result<bool, MemFault> {
    let Some(body) = object_address(m, unit)? else {
        return Ok(false);
    };
    Ok(m.u32(body + 0x13c)? != NONE)
}

pub(super) fn draw_voice(m: &mut dyn GuestMemory, count: usize) -> Result<usize, MemFault> {
    if count <= 1 {
        return Ok(0);
    }
    let seed = m.u32(RANDOM_SEED)?;
    if seed == 0 {
        return Ok(0);
    }
    let draw = m.u32(seed)?.wrapping_mul(0x19660d).wrapping_add(0x3c6ef35f);
    m.set_u32(seed, draw)?;
    Ok((count * (draw >> 16) as usize) >> 16)
}

pub(super) fn allied(m: &dyn GuestMemory, a: i16, b: i16) -> Result<bool, MemFault> {
    if !(0..16).contains(&a) || !(0..16).contains(&b) {
        return Ok(false);
    }
    let alliances = m.u32(TEAM_RELATIONSHIPS)?;
    if alliances == 0 {
        return Ok(a == b);
    }
    let bit = 16 * a as u32 + b as u32;
    Ok(m.u32(alliances + ALLIANCES + (bit / 32) * 4)? & (1 << (bit % 32)) != 0)
}

fn score_cast(
    unit: Unit,
    player: Option<Unit>,
    peer_distance: f32,
    mode: u16,
    leader: bool,
    allied_to_player: bool,
    riding_as_passenger: bool,
) -> Option<f32> {
    let modifier = match mode {
        0 | 6 if player.is_some() && !allied_to_player => return None,
        2 if unit.parent == NONE || !player.is_some_and(|p| unit.parent == p.parent) => {
            return None;
        }
        2 if riding_as_passenger => 1.0,
        3 if unit.parent != NONE => return None,
        4 | 7 if leader => 1.5,
        _ => 0.0,
    };
    let to_player = player.map_or(0.0, |p| player_preference(p, unit));
    let peer = if peer_distance < 4.5 {
        1.0 - (peer_distance - 1.5) / 3.0
    } else {
        0.0
    };
    Some(modifier + peer + to_player)
}

fn choose_voice<E>(
    profile: i16,
    voices: [i16; 6],
    random: impl FnOnce(usize) -> Result<usize, E>,
) -> Result<Option<(i16, f32)>, E> {
    if voices.iter().all(|&voice| voice < 0) {
        return Ok(Some((-1, 0.0)));
    }
    if profile >= 0 && voices.contains(&profile) {
        return Ok(Some((profile, 0.7)));
    }
    if voices.contains(&0) {
        return Ok(Some((0, 0.7)));
    }
    let generic: Vec<_> = voices
        .into_iter()
        .filter(|voice| (1..100).contains(voice))
        .collect();
    // The source cast falls back to generic recordings only for a
    // generic unit whose existing selector has no exact line.
    if profile < 100 && !generic.is_empty() {
        return Ok(Some((generic[random(generic.len())?], 0.0)));
    }
    Ok(None)
}

struct Candidate {
    unit: Unit,
    peer_distance: f32,
    rank: u32,
    voice: u32,
    profile: voices::Profile,
    voices: [i16; 6],
}

fn candidate(
    m: &mut dyn GuestMemory,
    candidate: u32,
    voice_base: u32,
    options: u32,
) -> Result<Option<Candidate>, MemFault> {
    let (Some(rank), Some(voice), Some(profiles), Some(v0), Some(v1), Some(v2)) = (
        global(m, candidate.saturating_add(1), REAL)?,
        global(m, candidate.saturating_add(2), SHORT)?,
        global(m, candidate.saturating_add(3), LONG)?,
        global(m, voice_base, LONG)?,
        global(m, voice_base.saturating_add(1), LONG)?,
        global(m, voice_base.saturating_add(2), LONG)?,
    ) else {
        return Ok(None);
    };
    m.set_f32(rank, f32::MIN)?;
    let unit = object_global(m, candidate)?;
    let Some(spoken) = Unit::read(m, unit)? else {
        return Ok(None);
    };
    if actor_handle(m, unit)?.is_none() {
        return Ok(None);
    }
    let Some(r) = voices::Registry::read(m)? else {
        return Ok(None);
    };
    if r.state != m.u32(profiles)? {
        return Ok(None);
    }
    let profile = voices::Table::Globals(r.state).get(m, unit)?;
    let count = (options >> 8) & 0xff;
    let first = options >> 16;
    let mut peer_dist = f32::INFINITY;
    for index in 0..count {
        let peer_index = first + index * ROLE_STRIDE;
        if peer_index + 2 == voice_base {
            continue;
        }
        let peer = object_global(m, peer_index)?;
        if peer != unit
            && let Some(peer) = Unit::read(m, peer)?
        {
            peer_dist = peer_dist.min(distance(peer.pos, spoken.pos));
        }
    }
    let words = [m.u32(v0)?, m.u32(v1)?, m.u32(v2)?];
    let voices = std::array::from_fn(|i| (words[i / 2] >> (i % 2 * 16)) as i16);
    Ok(Some(Candidate {
        unit: spoken,
        peer_distance: peer_dist,
        rank,
        voice,
        profile,
        voices,
    }))
}

pub(super) async fn score(
    cx: &Cx,
    scratch: u32,
    cell: u32,
    voice_base: u32,
    options: u32,
) -> Result<(), EntryError> {
    let Some(mut c) = cx.write(|m| candidate(m, cell, voice_base, options))? else {
        return Ok(());
    };
    let first = !c.peer_distance.is_finite();
    let players = cx.read(|m| {
        let mut players = Vec::new();
        for p in crate::coop_players::players(m) {
            let Some(p) = Unit::read(m, p.unit)? else {
                continue;
            };
            if !first || visibility::in_pvs(m, c.unit.cluster, p.cluster)? {
                players.push(p);
            }
        }
        Ok::<_, MemFault>(players)
    })?;
    let mut player = None;
    let mut player_bonus = 0.0;
    let mut best = f32::NEG_INFINITY;
    if !players.is_empty() {
        c.unit.pos = visibility::head(cx, scratch, c.unit.handle).await?;
    }
    for mut p in players {
        p.pos = visibility::head(cx, scratch, p.handle).await?;
        let d = distance(c.unit.pos, p.pos);
        if d >= 30.0 {
            continue;
        }
        let bonus = if first {
            visibility::sight_bonus(cx, scratch, p.pos, c.unit.pos, d).await?
        } else {
            0.0
        };
        let score = player_preference(p, c.unit) + bonus;
        if score > best {
            best = score;
            player = Some(p);
            player_bonus = bonus;
        }
    }
    cx.write(|m| {
        let mode = options as u16 & 0xff;
        // The source only allows actor-bound radio roles to cast without
        // an eligible player (0x00143c8b, 0x00143f34).
        if player.is_none() && !matches!(mode, 6 | 7) {
            return Ok(());
        }
        let allied_to_player = match player {
            Some(p) if matches!(mode, 0 | 6) => allied(m, c.unit.team, p.team)?,
            _ => true,
        };
        let mut passenger = false;
        if mode == 2
            && let Some(vehicle) = object_address(m, c.unit.parent)?
        {
            passenger =
                m.u8(vehicle + 0xaa)? == 1 && m.u32(vehicle + VEHICLE_DRIVER)? != c.unit.handle;
        }
        let Some(base) = score_cast(
            c.unit,
            player,
            c.peer_distance,
            mode,
            c.profile.leader,
            allied_to_player,
            passenger,
        ) else {
            return Ok(());
        };
        let desired = choose_voice(c.profile.voice, c.voices, |count| draw_voice(m, count))?;
        if let Some((voice_id, bonus)) = desired {
            m.set_u16(c.voice, voice_id as u16)?;
            m.set_f32(c.rank, base + bonus + player_bonus)?;
        }
        Ok::<_, MemFault>(())
    })?;
    Ok(())
}

pub(super) fn operate(
    m: &mut dyn GuestMemory,
    op: Op,
    a: u32,
    b: u32,
    c: u32,
) -> Result<(), MemFault> {
    match op {
        Op::Actor => {
            if let Some(out) = global(m, b, AI)? {
                let handle = actor_handle(m, object_global(m, a)?)?;
                let ai = handle.map_or(NONE, |handle| 0x8000_0000 | u32::from(handle as u16));
                m.set_u32(out, ai)?;
            }
        }
        Op::Focus => {
            if let Some(out) = global(m, b, OBJECT)? {
                let count = c & 0xff;
                let must_look = c & 0x100 != 0;
                let mut closest = NONE;
                let mut best_score = f32::NEG_INFINITY;
                if count == 0 && !must_look {
                    closest = focus(m, None, false)?.map_or(NONE, |p| p.handle);
                }
                for index in 0..count {
                    let Some(anchor) =
                        Unit::read(m, object_global(m, a.saturating_add(index * ROLE_STRIDE))?)?
                    else {
                        continue;
                    };
                    if let Some(player) = focus(m, Some(anchor), must_look)? {
                        let score = player_preference(player, anchor);
                        if score > best_score {
                            closest = player.handle;
                            best_score = score;
                        }
                    }
                }
                m.set_u32(out, closest)?;
            }
        }

        Op::RememberVoice => {
            let unit = object_global(m, a)?;
            if unit != NONE
                && voices::Registry::read(m)?.is_some_and(|r| r.state == c)
                && let Some(voice) = global(m, b, SHORT)?
            {
                let voice = read_i16(m, voice)?;
                if voice >= 0 {
                    let table = voices::Table::Globals(c);
                    let leader = table.get(m, unit)?.leader;
                    table.put(m, unit, voices::Profile { voice, leader })?;
                }
            }
        }
        op => unreachable!("sound operation {} in casting", op as u32),
    }
    Ok(())
}

pub(super) async fn cancel_command(cx: &Cx, unit: u32, script: u32) -> Result<(), EntryError> {
    let command = cx.read(|m| {
        let Some(actor) = actor_handle(m, object_global(m, unit)?)? else {
            return Ok::<_, MemFault>(None);
        };
        let Some(row) = datum(m, ACTORS, actor)? else {
            return Ok(None);
        };
        let mut handle = m.u32(row + ACTOR_COMMAND_STACK)?;
        for _ in 0..ACTOR_CAPACITY {
            let Some(command) = datum(m, COMMANDS, handle)? else {
                break;
            };
            if m.u32(command + COMMAND_SCRIPT)? == script {
                return Ok(Some((actor, handle)));
            }
            handle = m.u32(command + COMMAND_NEXT)?;
        }
        Ok(None)
    })?;
    if let Some((actor, command)) = command {
        cx.call(GuestCall::new(DELETE_COMMAND).ecx(actor).args([command]))
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(team: i16, x: f32, parent: u32) -> Unit {
        Unit {
            handle: 0,
            parent,
            team,
            pos: [x, 0.0, 0.0],
            cluster: -1,
            forward: [1.0, 0.0, 0.0],
        }
    }

    #[test]
    fn cast_prefers_nearby_and_distinguished_actors_and_honors_vehicle_modes() {
        let player = unit(1, 0.0, 25);
        let ally = unit(1, 4.0, 25);
        assert!(
            score_cast(ally, Some(player), 4.0, 0, false, true, false).unwrap()
                > score_cast(unit(1, 9.0, NONE), Some(player), 4.0, 0, false, true, false).unwrap()
        );
        assert!(score_cast(unit(2, 1.0, 25), Some(player), 4.0, 6, false, false, false).is_none());
        assert!(score_cast(unit(1, 2.0, NONE), Some(player), 4.0, 2, false, true, false).is_none());
        assert!(score_cast(ally, Some(player), 4.0, 3, false, true, false).is_none());
        assert!(
            score_cast(ally, Some(player), 4.0, 7, true, true, false).unwrap()
                > score_cast(ally, Some(player), 4.0, 7, false, true, false).unwrap()
        );
        assert!(player.looks_at(ally));
        assert!(!ally.looks_at(player));
    }

    #[test]
    fn live_campaign_alliance_and_passenger_preference() {
        let mut m = guest_layout::testing::FakeGuest::new();
        let table = 0x1000;
        m.put_u32(TEAM_RELATIONSHIPS, table);
        // Human (2) and player (1) can be allied as distinct teams.
        m.put_u32(table + ALLIANCES + 4, 2);
        assert!(allied(&m, 2, 1).unwrap());
        m.put_u32(table + ALLIANCES + 4, 0);
        assert!(!allied(&m, 2, 1).unwrap());

        let player = unit(1, 0.0, 25);
        let candidate = unit(2, 0.0, 25);
        let driver = score_cast(candidate, Some(player), 4.0, 2, false, true, false).unwrap();
        let passenger = score_cast(candidate, Some(player), 4.0, 2, false, true, true).unwrap();
        assert_eq!(passenger - driver, 1.0);
    }

    #[test]
    fn seeded_generic_voice_beats_the_first_scripted_alternative() {
        let voices = [2, 1, 4, 6, 5, -1];
        assert_eq!(
            choose_voice::<()>(4, voices, |_| panic!()),
            Ok(Some((4, 0.7)))
        );
    }

    #[test]
    fn exact_voice_precedes_wildcard_and_generic_alternatives() {
        let voices = [-1, 0, 117, 4, 7, -1];
        assert_eq!(
            choose_voice::<()>(117, voices, |_| panic!()),
            Ok(Some((117, 0.7)))
        );
        assert_eq!(
            choose_voice::<()>(116, voices, |_| panic!()),
            Ok(Some((0, 0.7)))
        );
        let voices = [-1, 4, 117, -1, 7, -1];
        assert_eq!(choose_voice::<()>(0, voices, |_| Ok(1)), Ok(Some((7, 0.0))));
        assert_eq!(choose_voice::<()>(116, voices, |_| panic!()), Ok(None));
    }
}
