//! Oddball, King of the Hill and Territories played as people play them
//! (docs/game/bots.md, section 3.7): the ball's team screens its carrier
//! and the other team hunts it together and rushes a loose ball; on the
//! hill they crowd in, clear it and play its edges; in Territories a team
//! spreads over the sites, takes and holds what is near and relieves a
//! site under pressure. Every decision here is pure: what the bot sees,
//! its teammates' HUD markers and its name's temper.
//!
//! The fallbacks read the scenario's netgame flags alone, for a rules
//! script that keeps no engine globals (docs/game/rules-engine.md,
//! "Porting the seven engines").

use guest_layout::memory::{GuestMemory, MemFault};
use engine::objectives::{
    FLAG_TEAM, FLAG_TYPE, NETGAME_FLAG_LIMIT, NETGAME_FLAG_STRIDE, NETGAME_FLAGS, check_span, kind,
};
use guest_layout::points;

use super::{Goal, Hill, Objective, Territory, dist, dist_xy, member, nearest};
use crate::brain::{Me, Seen, Vec3};
use crate::decisions::Personality;

/// The name's temper in objective play: continuous traits from the name's
/// band (`bots::name_profile`), the same every match.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Temper {
    /// Waits for teammates before pushing the carrier.
    pub patience: f32,
    /// Plays the hill's edge rather than its middle.
    pub caution: f32,
}

impl Default for Temper {
    fn default() -> Self {
        Self {
            patience: 0.5,
            caution: 0.5,
        }
    }
}

impl From<Personality> for Temper {
    fn from(p: Personality) -> Self {
        Self {
            patience: p.patience,
            caution: p.caution,
        }
    }
}

// ---- Oddball ----

/// The nearest this many of a team run for a loose ball...
pub const RUSHERS: usize = 2;
/// ...and the rest fight within this of it.
pub const BALL_COVER: f32 = 6.0;
/// An escort stands this far out from the carrier, towards the threat.
pub const SCREEN: f32 = 3.5;
/// The enemies within this of our carrier are its threat.
pub const SCREEN_REACH: f32 = 25.0;
/// A hunter this close to the carrier, alone and well ahead of a
/// teammate who is coming, waits for them...
pub const STAGE: f32 = 18.0;
/// ...unless it is already this close...
pub const SHOWDOWN: f32 = 6.0;
/// ...or a teammate is within this of it.
pub const GROUP: f32 = 8.0;
/// How far a waiting hunter may shift.
pub const STAGE_HOLD: f32 = 3.0;

/// Oddball: carrying, keep it; a loose ball, the nearest two take it and
/// the rest cover it; an enemy carrier, hunt it as a group; a teammate's,
/// screen it. Between balls, the closest ball spawn.
pub(super) fn ball(
    o: &Objective,
    me: &Me,
    others: &[Seen],
    mates: &[u32],
    friend: &dyn Fn(u32) -> bool,
    t: Temper,
) -> Option<Goal> {
    if o.items.iter().any(|i| i.carrier == Some(o.me)) {
        return Some(Goal::Keep);
    }
    let at = me.position;
    let mates_at: Vec<(u32, Vec3)> = others
        .iter()
        .filter(|s| mates.contains(&s.id))
        .map(|s| (s.id, s.position))
        .collect();
    if let Some(b) = nearest(
        at,
        o.items
            .iter()
            .filter(|i| i.carrier.is_none())
            .map(|i| (i, i.position)),
    ) {
        let mine = (dist(at, b.position), o.me);
        let ahead = mates_at
            .iter()
            .filter(|&&(id, p)| (dist(p, b.position), id) < mine)
            .count();
        return Some(if ahead < RUSHERS {
            Goal::Go(b.position)
        } else {
            Goal::Hold(b.position, BALL_COVER)
        });
    }
    if let Some(b) = nearest(
        at,
        o.items
            .iter()
            .filter(|i| i.carrier.is_some_and(|c| !friend(c)))
            .map(|i| (i, i.position)),
    ) && let Some(c) = b.carrier
    {
        return Some(regroup(at, b.position, &mates_at, t).unwrap_or(Goal::Hunt(c, b.position)));
    }
    if let Some(b) = o
        .items
        .iter()
        .find(|i| i.carrier.is_some_and(|c| c != o.me && friend(c)))
    {
        return Some(Goal::Escort(screen(
            o.me, b.position, b.carrier, others, mates, friend,
        )));
    }
    nearest(
        at,
        o.spots
            .iter()
            .filter(|s| s.kind == kind::BALL_SPAWN)
            .map(|s| (s, s.position)),
    )
    .map(|s| Goal::Go(s.position))
}

/// A hunter alone near the carrier, with a teammate coming well behind,
/// holds where it is until they close up: the team arrives together
/// rather than one at a time into the escorts. The patient wait for a
/// smaller lead; a teammate too far off (respawning) is not waited for.
fn regroup(at: Vec3, carrier: Vec3, mates: &[(u32, Vec3)], t: Temper) -> Option<Goal> {
    let d = dist(at, carrier);
    if !(SHOWDOWN..STAGE).contains(&d) || mates.iter().any(|&(_, p)| dist(p, at) < GROUP) {
        return None;
    }
    let lead = 6.0 + 14.0 * (1.0 - t.patience.clamp(0.0, 1.0));
    let coming = mates.iter().any(|&(_, p)| {
        let m = dist(p, carrier);
        m > d + lead && m < d + 2.0 * STAGE
    });
    coming.then_some(Goal::Hold(at, STAGE_HOLD))
}

/// Where an escort stands: between the carrier and the nearest enemy in
/// view (or on the radar's reach of it), fanned out by rank so two escorts
/// do not stand in one spot. With no threat, by the carrier.
fn screen(
    me: u32,
    carrier: Vec3,
    carrier_id: Option<u32>,
    others: &[Seen],
    mates: &[u32],
    friend: &dyn Fn(u32) -> bool,
) -> Vec3 {
    let Some(threat) = nearest(
        carrier,
        others
            .iter()
            .filter(|s| !friend(s.id) && dist(s.position, carrier) < SCREEN_REACH)
            .map(|s| (s, s.position)),
    ) else {
        return carrier;
    };
    let (dx, dy) = (
        threat.position[0] - carrier[0],
        threat.position[1] - carrier[1],
    );
    let l = dx.hypot(dy);
    if l < 1.0 {
        return carrier;
    }
    let mut escorts: Vec<u32> = mates
        .iter()
        .copied()
        .filter(|&id| Some(id) != carrier_id && id != me)
        .collect();
    escorts.push(me);
    escorts.sort_unstable();
    let rank = escorts.iter().position(|&id| id == me).unwrap_or(0);
    let turn = [0.0f32, 0.6, -0.6, 1.2, -1.2][rank % 5];
    let (s, c) = turn.sin_cos();
    let (ux, uy) = (dx / l, dy / l);
    let reach = SCREEN.min(l * 0.5);
    [
        carrier[0] + (ux * c - uy * s) * reach,
        carrier[1] + (ux * s + uy * c) * reach,
        carrier[2],
    ]
}

// ---- King of the Hill ----

/// A bot in the hill keeps within this part of its radius, more for the
/// cautious (who play its edge)...
pub const HILL_KEEP: f32 = 0.4;
pub const HILL_KEEP_CAUTION: f32 = 0.25;
/// ...and, with two teammates already holding it, one this cautious
/// covers it from just outside.
pub const EDGE_CAUTION: f32 = 0.6;

/// King of the Hill: in it, hold it, fighting from it (the cautious nearer
/// its edge); out of it, go in, unless the hill is held by two teammates
/// and nobody contests it, when the cautious cover its edge.
pub(super) fn hill(
    o: &Objective,
    h: &Hill,
    me: &Me,
    others: &[Seen],
    friend: &dyn Fn(u32) -> bool,
    t: Temper,
) -> Goal {
    let c = h.center();
    let r = h.radius();
    if o.in_hill(o.me, me.position) {
        // The engine counts it in. Where the corners agree, it moves about
        // the middle; where they do not (another floor, an odd shape), it
        // keeps the spot the engine counts.
        return if h.corners.len() < 3 || h.contains(me.position) {
            let keep = HILL_KEEP + HILL_KEEP_CAUTION * t.caution.clamp(0.0, 1.0);
            Goal::Hold(c, (r * keep).max(0.5))
        } else {
            Goal::Hold(me.position, 0.0)
        };
    }
    let inside = |s: &&Seen| o.in_hill(s.id, s.position);
    let mates_in = others
        .iter()
        .filter(|s| friend(s.id))
        .filter(inside)
        .count();
    let foes_in = others
        .iter()
        .filter(|s| !friend(s.id))
        .filter(inside)
        .count();
    if mates_in >= 2 && foes_in == 0 && t.caution >= EDGE_CAUTION {
        return Goal::Hold(c, r * (1.0 + t.caution));
    }
    Goal::Go(c)
}

// ---- Territories ----

/// A bot in a territory keeps within this of its marker.
pub const TERRITORY_KEEP: f32 = 1.5;
/// Without the engine's occupancy, a player this close to a marker (XY,
/// and within [`super::HILL_HEIGHT`] of it) stands in it.
pub const TERRITORY_REACH: f32 = 3.0;

/// Territories: the team spreads over the sites. Every bot ranks the
/// pairs of teammate and site the same way (a friendly site with an
/// enemy in it, or one being taken, first; then one not ours; then ours;
/// nearer first) and gives each site one taker, two where an enemy
/// stands in it; a bot left over goes where it would alone. In it, hold
/// it; else go.
pub(super) fn territory(
    o: &Objective,
    sites: &[Territory],
    me: &Me,
    others: &[Seen],
    mates: &[u32],
    teams: bool,
    friend: &dyn Fn(u32) -> bool,
) -> Option<Goal> {
    let allied = |s: &Territory| s.owner == Some(o.me) || teams && s.owner_team == Some(me.team);
    let hostile = |s: &Territory| {
        others
            .iter()
            .any(|p| !friend(p.id) && member(s.occupants, p.id))
    };
    let priority = |id: u32, s: &Territory| match (allied(s), hostile(s), member(s.occupants, id)) {
        (true, true, _) | (false, _, true) => 0,
        (false, _, _) => 1,
        _ => 2,
    };
    let team: Vec<(u32, Vec3)> = std::iter::once((o.me, me.position))
        .chain(
            others
                .iter()
                .filter(|s| mates.contains(&s.id))
                .map(|s| (s.id, s.position)),
        )
        .collect();
    let mut pairs: Vec<(u8, f32, u32, usize)> = team
        .iter()
        .flat_map(|&(id, at)| {
            sites
                .iter()
                .enumerate()
                .map(move |(i, s)| (priority(id, s), dist(at, s.at), id, i))
        })
        .collect();
    pairs.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
    });
    let mut load = vec![0u8; sites.len()];
    let mut placed: Vec<u32> = Vec::new();
    let mut mine = None;
    for (_, _, id, i) in pairs {
        let room = if hostile(&sites[i]) { 2 } else { 1 };
        if placed.contains(&id) || load[i] >= room {
            continue;
        }
        load[i] += 1;
        placed.push(id);
        if id == o.me {
            mine = Some(i);
            break;
        }
    }
    let site = match mine {
        Some(i) => &sites[i],
        None => sites.iter().min_by(|a, b| {
            priority(o.me, a)
                .cmp(&priority(o.me, b))
                .then_with(|| dist(a.at, me.position).total_cmp(&dist(b.at, me.position)))
        })?,
    };
    Some(if member(site.occupants, o.me) {
        Goal::Hold(site.at, TERRITORY_KEEP)
    } else {
        Goal::Go(site.at)
    })
}

/// Sites from the markers alone: no owner known, and who stands in one
/// judged by distance (the bot itself and the players it knows).
pub(super) fn sites_from_markers(
    markers: &[Vec3],
    me: u32,
    at: Vec3,
    others: &[Seen],
) -> Vec<Territory> {
    let near = |m: Vec3, p: Vec3| {
        dist_xy(m, p) < TERRITORY_REACH && (m[2] - p[2]).abs() < super::HILL_HEIGHT
    };
    markers
        .iter()
        .map(|&m| {
            let occupants = std::iter::once((me, at))
                .chain(others.iter().map(|s| (s.id, s.position)))
                .filter(|&(id, p)| id < 16 && near(m, p))
                .fold(0u16, |mask, (id, _)| mask | 1 << id);
            Territory {
                at: m,
                owner: None,
                owner_team: None,
                occupants,
            }
        })
        .collect()
}

// ---- in a fight ----

/// Footwork inside a held zone: free in its middle; past half its radius
/// the step loses the part that leads out, so a fight never walks the
/// bot out of the hill, the territory or its place by the ball.
pub(crate) fn within(walk: [f32; 2], at: Vec3, anchor: Vec3, radius: f32) -> [f32; 2] {
    let out = [at[0] - anchor[0], at[1] - anchor[1]];
    let d = out[0].hypot(out[1]);
    if d <= 0.5 * radius || d < 1e-4 {
        return walk;
    }
    let n = [out[0] / d, out[1] / d];
    let radial = walk[0] * n[0] + walk[1] * n[1];
    if radial <= 0.0 {
        return walk;
    }
    [walk[0] - n[0] * radial, walk[1] - n[1] * radial]
}

/// The smallest held radius in which a fight keeps its footwork.
pub const FOOTWORK_ROOM: f32 = 1.0;

// ---- without the engine's globals ----

/// The scenario's netgame flags with their identifier (`+0x14`).
fn flags_with_identifier(m: &dyn GuestMemory) -> Result<Vec<(Vec3, u16, u16, u16)>, MemFault> {
    let s = m.u32(points::SCENARIO.base)?;
    if s == 0 {
        return Ok(Vec::new());
    }
    check_span(s, NETGAME_FLAGS + 8)?;
    let n = m.i32(s + NETGAME_FLAGS)?.clamp(0, NETGAME_FLAG_LIMIT) as u32;
    let at = m.u32(s + NETGAME_FLAGS + 4)?;
    check_span(at, n * NETGAME_FLAG_STRIDE)?;
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let f = at + i * NETGAME_FLAG_STRIDE;
        out.push((
            m.vec3(f)?,
            m.u16(f + FLAG_TYPE)?,
            m.u16(f + FLAG_TEAM)?,
            m.u16(f + FLAG_TEAM + 2)?,
        ));
    }
    Ok(out)
}

/// Territory markers as the engine's init picks them (`0x2bf5b0`): for
/// each identifier 0 to 7, the first type-10 flag with it.
pub(super) fn territory_markers(m: &dyn GuestMemory) -> Result<Vec<Vec3>, MemFault> {
    let flags = flags_with_identifier(m)?;
    Ok((0..8u16)
        .filter_map(|id| {
            flags
                .iter()
                .find(|f| f.1 == kind::TERRITORY && f.3 == id)
                .map(|f| f.0)
        })
        .collect())
}

/// The lowest-numbered hill (flag types 11 to 18), the hill a game starts
/// on (`0x2bd20b`): the area of its first flag that has one in `areas`
/// (flag index and area), else its corners when there are three or more.
pub(super) fn first_hill(
    flags: &[(Vec3, u16, u16)],
    areas: &[(u32, game_markers::Area)],
) -> Option<Hill> {
    (0..kind::HILLS).find_map(|n| {
        let shaped = flags.iter().enumerate().find_map(|(i, f)| {
            let area = areas.iter().find(|(a, _)| *a as usize == i)?;
            (f.1 == kind::HILL + n).then(|| (f.0, area.1.clone()))
        });
        if let Some((at, area)) = shaped {
            return Some(Hill {
                corners: vec![at],
                area: Some((at, area)),
            });
        }
        let corners: Vec<Vec3> = flags
            .iter()
            .filter(|f| f.1 == kind::HILL + n)
            .map(|f| f.0)
            .collect();
        (corners.len() >= 3).then_some(Hill {
            corners,
            area: None,
        })
    })
}

/// The neutral ball spawns (type 4, designator 8), as Oddball's init
/// queries them (`0x2bc2e0`), at most three.
pub(super) fn ball_spawns(flags: &[(Vec3, u16, u16)]) -> Vec<Vec3> {
    flags
        .iter()
        .filter(|f| f.1 == kind::BALL_SPAWN && f.2 == 8)
        .take(3)
        .map(|f| f.0)
        .collect()
}

#[cfg(test)]
mod tests;
