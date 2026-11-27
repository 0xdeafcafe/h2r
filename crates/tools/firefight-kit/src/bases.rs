//! The two bases a kit arms (docs/game/firefight.md, "The kit"): base 0 is
//! the home base, the players' base in one-team Firefight and team 0's with
//! two teams; base 1 is the other. They come from the map's own scenario
//! rows, never from coordinates of ours.
use pathfind::geom::{V3, dot, sub};
use serde::Serialize;

/// Netgame flags (`scnr+0x118`): position at `+0`, type at `+0x10`, team
/// at `+0x12` (`map::scenario_layout`, `forge_doc::labels::rows_of_file`).
pub(crate) const NETGAME_FLAGS: u32 = 0x118;
pub(crate) const NETGAME_FLAG: usize = 0x20;
const FLAG_TYPE: usize = 0x10;
const FLAG_TEAM: usize = 0x12;
/// The netgame flag type of a CTF flag stand (`forge_doc::labels::builtin_label`).
pub(crate) const CTF_FLAG_SPAWN: u16 = 0;
/// Starting location: the team designator, `i16` (`scnr+0x100`, element
/// `0x34`).
pub(crate) const START_TEAM: usize = 0x10;

/// A map team, as the scenario's team fields number them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Team {
    /// Team 0.
    #[default]
    Red,
    /// Team 1.
    Blue,
}

impl Team {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "red" | "0" => Some(Self::Red),
            "blue" | "1" => Some(Self::Blue),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Blue => 1,
        }
    }
}

/// What the kit builder is asked for besides the base map.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Config {
    /// Which map team's base becomes the home base, base 0. A layout names
    /// its own.
    pub home: Team,
    /// The authored layout the drops come from, for a map that has one
    /// (`crate::builtin::for_map`); the generator picks them otherwise.
    pub layout: Option<&'static crate::builtin::Layout>,
    /// Which dropship the kit carries ([`crate::Dropship`]).
    pub dropship: crate::Dropship,
    /// Leave the drop pods out: no pod tag, no pools, no falls
    /// (`crate::sky`).
    pub no_pods: bool,
    /// Collision material indices to count as sky besides the ones whose
    /// shader is a sky (`crate::sky::sky_materials`); empty on the retail
    /// maps, where no shader is.
    pub sky_materials: &'static [u16],
    /// How high the retail campaign holds each kind of ship over the floor
    /// (`crate::ship::HoverSurvey`): the median is the ship's hover. None
    /// keeps the hover the ship's own tag gives.
    pub ship_hovers: Option<&'static crate::ship::HoverSurvey>,
}

/// Where the two base points came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The mean of each team's CTF flag stands.
    FlagStands,
    /// The mean of each team's starting locations.
    TeamStarts,
    /// The two clusters of all starting locations farthest apart.
    StartClusters,
    /// The placed `ff_base` labels, one for each team.
    Labels,
}

/// The two bases, index 0 the home base.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Bases {
    pub points: [[f32; 3]; 2],
    pub source: Source,
    /// The map team whose base is base 0.
    pub home: Team,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StartRow {
    pub position: V3,
    pub team: i16,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct FlagRow {
    pub position: V3,
    pub kind: u16,
    pub team: u16,
}

fn f32_at(row: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(row[at..at + 4].try_into().unwrap())
}

fn u16_at(row: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(row[at..at + 2].try_into().unwrap())
}

fn position(row: &[u8]) -> V3 {
    std::array::from_fn(|k| f32_at(row, 4 * k))
}

/// One starting location row (`0x34` bytes).
pub(crate) fn start_row(row: &[u8]) -> StartRow {
    StartRow {
        position: position(row),
        team: u16_at(row, START_TEAM) as i16,
    }
}

/// One netgame flag row (`0x20` bytes).
pub(crate) fn flag_row(row: &[u8]) -> FlagRow {
    FlagRow {
        position: position(row),
        kind: u16_at(row, FLAG_TYPE),
        team: u16_at(row, FLAG_TEAM),
    }
}

pub(crate) fn distance2(a: V3, b: V3) -> f32 {
    let d = sub(a, b);
    dot(d, d)
}

/// Two points closer than this are one base.
const SAME_POINT: f32 = 1.0;

/// The bases: the CTF flag stands of teams 0 and 1, else the mean of each
/// team's starting locations, else the two clusters of all starting
/// locations farthest apart. `home` picks the map team whose base is base 0.
/// None when the rows give no two distinct points.
pub(crate) fn find(starts: &[StartRow], flags: &[FlagRow], home: Team) -> Option<Bases> {
    let finite = |p: &V3| p.iter().all(|x| x.is_finite());
    let stands = flags
        .iter()
        .filter(|f| f.kind == CTF_FLAG_SPAWN && finite(&f.position))
        .map(|f| (f.position, i32::from(f.team)));
    let (teams, source) = if let Some(p) = by_team(stands) {
        (p, Source::FlagStands)
    } else if let Some(p) = by_team(
        starts
            .iter()
            .filter(|s| finite(&s.position))
            .map(|s| (s.position, i32::from(s.team))),
    ) {
        (p, Source::TeamStarts)
    } else {
        let all: Vec<V3> = starts.iter().map(|s| s.position).filter(finite).collect();
        (clusters(&all)?, Source::StartClusters)
    };
    let points = match home {
        Team::Red => teams,
        Team::Blue => [teams[1], teams[0]],
    };
    Some(Bases {
        points,
        source,
        home,
    })
}

/// The bases the `ff_base` labels place: team 0's and team 1's point. None
/// when the two are less than a unit apart.
pub(crate) fn from_labels(red: V3, blue: V3, home: Team) -> Option<Bases> {
    (distance2(red, blue) >= SAME_POINT.powi(2)).then_some(Bases {
        points: match home {
            Team::Red => [red, blue],
            Team::Blue => [blue, red],
        },
        source: Source::Labels,
        home,
    })
}

/// The mean point of team 0's rows and of team 1's, when both have one and
/// they are apart.
fn by_team(rows: impl Iterator<Item = (V3, i32)>) -> Option<[V3; 2]> {
    let mut sum = [[0f64; 3]; 2];
    let mut count = [0usize; 2];
    for (p, team) in rows {
        let Ok(t) = usize::try_from(team) else {
            continue;
        };
        if t < 2 {
            count[t] += 1;
            for k in 0..3 {
                sum[t][k] += f64::from(p[k]);
            }
        }
    }
    if count.contains(&0) {
        return None;
    }
    let mean = |t: usize| std::array::from_fn(|k| (sum[t][k] / count[t] as f64) as f32);
    let points = [mean(0), mean(1)];
    (distance2(points[0], points[1]) >= SAME_POINT.powi(2)).then_some(points)
}

/// Split the points into two clusters: seeded at the pair farthest apart
/// (the first such pair in row order), then each point to its nearer mean
/// until the split settles. The first seed's cluster is team 0's.
pub(crate) fn clusters(points: &[V3]) -> Option<[V3; 2]> {
    let mut far = None;
    let mut best = SAME_POINT.powi(2);
    for i in 0..points.len() {
        for j in i + 1..points.len() {
            let d = distance2(points[i], points[j]);
            if d > best {
                best = d;
                far = Some((i, j));
            }
        }
    }
    let (i, j) = far?;
    let mut means = [points[i], points[j]];
    for _ in 0..32 {
        let mut sum = [[0f64; 3]; 2];
        let mut count = [0usize; 2];
        for &p in points {
            let side = nearest(&means, p);
            count[side] += 1;
            for k in 0..3 {
                sum[side][k] += f64::from(p[k]);
            }
        }
        if count.contains(&0) {
            break;
        }
        let next: [V3; 2] =
            std::array::from_fn(|t| std::array::from_fn(|k| (sum[t][k] / count[t] as f64) as f32));
        if next == means {
            break;
        }
        means = next;
    }
    Some(means)
}

/// The index of the base nearest `p`; the lower index on a tie.
pub(crate) fn nearest(bases: &[V3], p: V3) -> usize {
    let mut best = 0;
    for (i, &b) in bases.iter().enumerate().skip(1) {
        if distance2(p, b) < distance2(p, bases[best]) {
            best = i;
        }
    }
    best
}

/// Each starting location's new team designator: the base it is nearest,
/// so team 0 spawns at the home base and team 1 at the other.
pub(crate) fn start_teams(bases: &[V3], starts: &[StartRow]) -> Vec<i16> {
    starts
        .iter()
        .map(|s| nearest(bases, s.position) as i16)
        .collect()
}

#[cfg(test)]
mod tests;
