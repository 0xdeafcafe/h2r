//! Static map knowledge. Traffic is a proxy: how often a polygon lies on
//! shortest (fewest portals) paths across the mesh from sampled origins.
use crate::brain::Vec3;
use crate::decisions::{distance, supplies::Powerup};
use crate::los::Walls;
use crate::mind::{CHEST, EYE};
use crate::nav::NavMesh;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vantage {
    pub at: Vec3,
    pub watches: Vec3,
    pub score: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PowerPad {
    pub site: u8,
    pub at: Vec3,
    pub period: Option<u32>,
    pub powerup: Option<Powerup>,
    pub model: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Places {
    pub vantages: Vec<Vantage>,
    pub lanes: Vec<Vec3>,
    pub starts: Vec<Vec3>,
    pub pads: Vec<PowerPad>,
}

fn raised(mut p: Vec3, h: f32) -> Vec3 {
    p[2] += h;
    p
}

fn traffic(routes: &[Vec<usize>]) -> Vec<usize> {
    let mut visits = vec![0usize; routes.len()];
    for start in (0..routes.len()).step_by(routes.len().div_ceil(16).max(1)) {
        let mut parent = vec![usize::MAX; routes.len()];
        parent[start] = start;
        let mut order = Vec::new();
        let mut queue = VecDeque::from([start]);
        while let Some(i) = queue.pop_front() {
            order.push(i);
            for &next in &routes[i] {
                if parent[next] == usize::MAX {
                    parent[next] = i;
                    queue.push_back(next);
                }
            }
        }
        let mut count = vec![1; routes.len()];
        for &i in order.iter().rev() {
            if i != start {
                let n = count[i];
                visits[i] += n;
                count[parent[i]] += n;
            }
        }
    }
    visits
}

impl Places {
    pub(crate) fn build(nav: &NavMesh, walls: &Walls, starts: &[Vec3]) -> Self {
        let mut routes = vec![vec![]; nav.polys.len()];
        for e in &nav.edges {
            routes[e.from as usize].push(e.to as usize);
        }
        for exits in &mut routes {
            exits.sort_unstable();
            exits.dedup();
        }
        let visits = traffic(&routes);
        let mut indices: Vec<_> = (0..routes.len())
            .filter(|&i| !routes[i].is_empty())
            .collect();
        indices.sort_by_key(|&i| std::cmp::Reverse(visits[i]));
        let mut lanes = Vec::new();
        for i in indices {
            let at = nav.polys[i].center;
            if lanes.iter().all(|&p| distance(at, p) > 4.0) {
                lanes.push(at);
                if lanes.len() == 32 {
                    break;
                }
            }
        }
        // One representative per patch keeps heavily tessellated ridges
        // from consuming the candidate budget on outdoor maps.
        let mut patches = BTreeMap::new();
        let cell = |at: Vec3| {
            (
                (at[0] / 6.).floor() as i32,
                (at[1] / 6.).floor() as i32,
                (at[2] / 4.).floor() as i32,
            )
        };
        for (i, p) in nav
            .polys
            .iter()
            .enumerate()
            .filter(|(i, _)| routes[*i].len() >= 2)
        {
            let slot = patches.entry(cell(p.center)).or_insert(i);
            if routes[i].len() > routes[*slot].len() {
                *slot = i;
            }
        }
        let stride = patches.len().div_ceil(384).max(1);
        let mut candidates = Vec::new();
        for &i in patches.values().step_by(stride) {
            let at = nav.polys[i].center;
            let eye = raised(at, EYE);
            let mut coverage = 0.;
            let mut height = 0.;
            let mut watch = None;
            for (rank, &lane) in lanes.iter().enumerate() {
                let d = distance(at, lane);
                if !(4.0..=65.0).contains(&d) || !walls.clear(eye, raised(lane, CHEST)) {
                    continue;
                }
                let traffic = 1.0 / (1.0 + rank as f32 / 12.0);
                coverage += traffic;
                height += traffic * ((at[2] - lane[2]) / 4.0).clamp(0., 1.);
                watch.get_or_insert(lane);
            }
            let Some(watches) = watch else {
                continue;
            };
            let cover = [[2., 0.], [-2., 0.], [0., 2.], [0., -2.]]
                .into_iter()
                .filter(|d| !walls.clear(eye, [eye[0] + d[0], eye[1] + d[1], eye[2]]))
                .count();
            let score = 0.50 * coverage / (coverage + 3.)
                + 0.18 * height / coverage
                + 0.20 * (cover.min(2) as f32 / 2.)
                + 0.12 * (routes[i].len().min(4) as f32 / 4.);
            candidates.push(Vantage { at, watches, score });
        }
        candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
        let mut vantages: Vec<Vantage> = Vec::new();
        // Fill underrepresented regions first. A long ridge can contribute
        // several positions, but separated low ground also gets a choice.
        for spacing in [24., 10., 5.] {
            for &v in &candidates {
                if vantages.len() == 32 {
                    break;
                }
                if vantages
                    .iter()
                    .all(|other| distance(other.at, v.at) > spacing)
                {
                    vantages.push(v);
                }
            }
        }
        vantages.sort_by(|a, b| b.score.total_cmp(&a.score));
        let starts = starts
            .iter()
            .filter_map(|&p| nav.nearest(p, 0.8, 0.5, 2.).map(|(_, at)| at))
            .collect();
        Self {
            vantages,
            lanes,
            starts,
            pads: Vec::new(),
        }
    }

    pub(crate) fn strength(&self, site: Vec3) -> f32 {
        self.vantages
            .iter()
            .map(|v| v.score * (1.0 - distance(site, v.at) / 8.).max(0.))
            .fold(0., f32::max)
    }

    pub(crate) fn encode(&self, bytes: &mut Vec<u8>) {
        for len in [
            self.vantages.len(),
            self.lanes.len(),
            self.starts.len(),
            self.pads.len(),
        ] {
            bytes.extend_from_slice(&(len as u32).to_le_bytes());
        }
        for v in &self.vantages {
            for f in v.at.into_iter().chain(v.watches).chain([v.score]) {
                bytes.extend_from_slice(&f.to_le_bytes());
            }
        }
        for &f in self.lanes.iter().chain(&self.starts).flatten() {
            bytes.extend_from_slice(&f.to_le_bytes());
        }
        for p in &self.pads {
            for f in p.at {
                bytes.extend_from_slice(&f.to_le_bytes());
            }
            for w in [
                u32::from(p.site),
                match p.powerup {
                    None => 0,
                    Some(Powerup::Overshield) => 1,
                    Some(Powerup::Camo) => 2,
                    Some(Powerup::Mixed) => 3,
                },
                p.period.unwrap_or(0),
                p.model.unwrap_or(u32::MAX),
            ] {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
        }
    }

    pub(crate) fn decode(bytes: &[u8]) -> Option<Self> {
        let mut words = bytes.chunks_exact(4);
        if !words.remainder().is_empty() {
            return None;
        }
        let mut word = || Some(u32::from_le_bytes(words.next()?.try_into().ok()?));
        let (nv, nl, ns) = (word()? as usize, word()? as usize, word()? as usize);
        let np = word()? as usize;
        if nv > 32
            || nl > 32
            || ns > 65536
            || np > 128
            || bytes.len() / 4 != 4 + 7 * (nv + np) + 3 * (nl + ns)
        {
            return None;
        }
        let mut float = || {
            let f = f32::from_bits(word()?);
            f.is_finite().then_some(f)
        };
        fn point(f: &mut impl FnMut() -> Option<f32>) -> Option<Vec3> {
            Some([f()?, f()?, f()?])
        }
        let vantages = (0..nv)
            .map(|_| {
                Some(Vantage {
                    at: point(&mut float)?,
                    watches: point(&mut float)?,
                    score: float()?,
                })
            })
            .collect::<Option<_>>()?;
        let lanes = (0..nl).map(|_| point(&mut float)).collect::<Option<_>>()?;
        let starts = (0..ns).map(|_| point(&mut float)).collect::<Option<_>>()?;
        let pads = (0..np)
            .map(|_| {
                let at = [
                    f32::from_bits(word()?),
                    f32::from_bits(word()?),
                    f32::from_bits(word()?),
                ];
                if !at.iter().all(|f| f.is_finite()) {
                    return None;
                }
                let site = u8::try_from(word()?).ok()?;
                let powerup = match word()? {
                    0 => None,
                    1 => Some(Powerup::Overshield),
                    2 => Some(Powerup::Camo),
                    3 => Some(Powerup::Mixed),
                    _ => return None,
                };
                let p = word()?;
                let m = word()?;
                Some(PowerPad {
                    site,
                    at,
                    powerup,
                    period: (p != 0).then_some(p),
                    model: (m != u32::MAX).then_some(m),
                })
            })
            .collect::<Option<_>>()?;
        Some(Self {
            pads,
            vantages,
            lanes,
            starts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Me, Perception};
    use crate::cache::{MapData, Stamp};
    use crate::mind::{Doing, Mind, Skill};
    use crate::nav::{Edge, Kind, Poly};

    // Two opposite low bases and raised flanks, as in a converted outdoor
    // H1 CTF map. No coordinates or collision copied from the game.
    fn valley() -> MapData {
        let mut polys = Vec::new();
        let mut edges = Vec::new();
        for y in 0..3 {
            for x in 0..6 {
                let west = x as f32 * 8.;
                let south = y as f32 * 8.;
                let z = if y == 1 { 0. } else { 2. };
                let p = [west + 4., south + 4., z];
                let id = polys.len() as u32;
                polys.push(Poly::new(vec![
                    [west, south, z],
                    [west + 8., south, z],
                    [west + 8., south + 8., z],
                    [west, south + 8., z],
                ]));
                for neighbor in [(x > 0).then(|| id - 1), (y > 0).then(|| id - 6)]
                    .into_iter()
                    .flatten()
                {
                    for (from, to) in [(id, neighbor), (neighbor, id)] {
                        edges.push(Edge {
                            from,
                            to,
                            a: p,
                            b: p,
                            kind: Kind::Walk,
                        });
                    }
                }
            }
        }
        let nav = NavMesh::new(polys, edges);
        let walls = Walls::default();
        let places = Places::build(&nav, &walls, &[[4., 12., 0.], [44., 12., 0.]]);
        MapData { nav, walls, places }
    }

    #[test]
    fn open_h1_valley_keeps_opposite_flanks_and_round_trips_the_cache() {
        let map = valley();
        let vs = &map.places.vantages;
        assert!(vs.iter().any(|v| v.at[1] < 8.));
        assert!(vs.iter().any(|v| v.at[1] > 16.));
        assert!(vs.windows(2).all(|v| v[0].score >= v[1].score));
        assert_eq!(map.places.starts.len(), 2);
        assert!(vs.iter().all(|v| map.places.lanes.contains(&v.watches)));
        let bytes = map.to_bytes(Stamp::now(4));
        assert_eq!(MapData::from_bytes(&bytes).unwrap().0, map);
        assert!(MapData::from_bytes(&bytes[..bytes.len() - 1]).is_none());
    }

    #[test]
    fn a_scoreboard_lead_holds_a_vantage_facing_its_lane() {
        let map = valley();
        let v = map.places.vantages[0];
        let mut p = Perception {
            me: Some(Me {
                position: v.at,
                ..Default::default()
            }),
            ..Default::default()
        };
        p.observations.ahead = true;
        let mut mind = Mind::new(Skill::Legendary, 41);
        let pad = mind.step(&p, Some(&map));
        assert_eq!(mind.doing, Doing::Vantage(v));
        assert_eq!(pad.left_stick, (0., 0.));
    }

    #[test]
    fn h1_valley_base_wall_hides_the_enemy_and_its_item() {
        use crate::brain::Seen;
        use crate::perceive::honesty::paired_worlds;
        let mut map = valley();
        map.walls =
            Walls::from_polygons([&[[8., 8., 0.], [8., 16., 0.], [8., 16., 4.], [8., 8., 4.]][..]]);
        let a = Perception {
            observations: crate::brain::Observations {
                player: Some((0, 0)),
                supplies: Some(Vec::new()),
                ..Default::default()
            },
            me: Some(Me {
                position: [4., 12., 0.],
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut b = a.clone();
        b.others.push(Seen {
            id: 4,
            team: 1,
            position: [12., 12., 0.],
        });
        b.observations
            .supplies
            .as_mut()
            .unwrap()
            .push(crate::decisions::supplies::Supply {
                site: 1,
                object: 71,
                spawn: [12., 12., 0.],
                at: [12., 12., 0.],
                respawn: Some(120 * 30),
                powerup: None,
                on_disturbance: true,
                disabled: false,
            });
        paired_worlds(Some(&map), [(a, b)]);
    }
}
