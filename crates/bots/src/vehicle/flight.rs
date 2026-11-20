//! Banshee flight controls in world coordinates, independent of pad timing.

use crate::brain::Vec3;
use crate::cache::MapData;
use crate::los::Walls;

#[derive(Clone, Debug, Default)]
pub struct Flight {
    health: Option<f32>,
    peel: Option<(Vec3, u32)>,
    evade: Option<(f32, u32)>,
    last_evade: Option<u32>,
    last_bomb: Option<u32>,
}

pub struct FlightIn {
    pub tick: u32,
    pub at: Vec3,
    pub aim: Vec3,
    pub forward: Vec3,
    pub destination: Vec3,
    pub target: Option<Vec3>,
    pub health: f32,
    pub has_bomb: bool,
    pub land: bool,
}

#[derive(Debug)]
pub struct Pilot {
    pub look: Vec3,
    /// Camera-relative controls (right, forward).
    pub stick: [f32; 2],
    pub boost: bool,
    pub flip: bool,
    pub cannon: bool,
    pub bomb: bool,
    pub landed: bool,
}

fn delta(from: Vec3, to: Vec3) -> Vec3 {
    std::array::from_fn(|i| to[i] - from[i])
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    (0..3).map(|i| a[i] * b[i]).sum()
}

pub fn floor(map: &MapData, at: Vec3) -> Option<f32> {
    // BSP/wall data also finds the ground when there is no nav polygon
    // under the flyer (a cliff, a roof, or a platform of the level).
    let start = [at[0], at[1], at[2] + 0.05];
    map.walls
        .hit(start, [at[0], at[1], start[2] - 32.0])
        .map(|(t, _)| start[2] - t * 32.0)
        .or_else(|| {
            let p = &map.nav.polys[map.nav.locate(start, 32.0)? as usize];
            Some(p.z_at(at))
        })
}

impl Flight {
    pub fn step(&mut self, i: &FlightIn, map: Option<&MapData>) -> Pilot {
        self.step_with_floor(i, map, map.and_then(|m| floor(m, i.at)))
    }

    pub(crate) fn step_with_floor(
        &mut self,
        i: &FlightIn,
        map: Option<&MapData>,
        floor: Option<f32>,
    ) -> Pilot {
        let [x, y, z] = i.at;
        let f = super::flat(i.forward);
        let height = floor.map_or(
            if map.is_some() {
                10.0
            } else {
                (z - i.destination[2]).max(0.0)
            },
            |g| z - g,
        );
        let hit = self.health.is_some_and(|h| h - i.health >= 0.02);
        self.health = Some(i.health);
        let (target_delta, range, aligned) =
            i.target.map_or(([0.0; 3], f32::INFINITY, false), |t| {
                let to = delta(i.at, t);
                let range = dot(to, to).sqrt();
                (to, range, dot(i.aim, to) > range * 0.982)
            });
        let side = if f[0] * target_delta[1] - f[1] * target_delta[0] > 0.0 {
            1.0
        } else {
            -1.0
        };
        let fresh = |then, span| i.tick.wrapping_sub(then) < span;
        self.peel = self.peel.filter(|&(_, then)| fresh(then, 42));
        self.evade = self.evade.filter(|&(_, then)| fresh(then, 18));
        if !i.land && (hit || range < 5.0) {
            self.peel.get_or_insert((
                [
                    x + 11.0 * f[0] - side * 6.0 * f[1],
                    y + 11.0 * f[1] + side * 6.0 * f[0],
                    z + 1.8,
                ],
                i.tick,
            ));
        }
        let clear = |to| map.is_none_or(|m| flight_clear(&m.walls, i.at, to, f));
        if !i.land
            && hit
            && height > 2.5
            && self.last_evade.is_none_or(|t| !fresh(t, 105))
            && clear([
                x + 5.0 * f[0] - side * 2.5 * f[1],
                y + 5.0 * f[1] + side * 2.5 * f[0],
                z,
            ])
        {
            self.evade = Some((side, i.tick));
            self.last_evade = Some(i.tick);
        }
        let mut look = i.destination;
        let [dx, dy, _] = delta(i.at, look);
        let dist = dx.hypot(dy);
        let cruise = i.destination[2] + 4.5;
        look[2] = cruise.max(floor.map_or(cruise, |g| g + 3.0));
        if dist < 2.0 {
            look[0] = x + 5.0 * f[0];
            look[1] = y + 5.0 * f[1];
        }
        let landing_here = i.land && dist < 2.0;
        if i.land {
            let land_z = i.destination[2] + 0.35;
            look[2] = if dist < 8.0 { land_z } else { look[2] };
            if landing_here {
                look = [x + f[0] * 2.0, y + f[1] * 2.0, land_z];
            }
        } else if let Some((away, _)) = self.peel {
            look = away;
        } else if let Some(t) = i.target
            && (6.0..=19.0).contains(&range)
            && height >= 2.8
        {
            look = t;
        }
        // Bound the pitch target and test a short hull-width corridor, so
        // a blocked attack run can climb or turn instead of diving at it.
        let horizontal = (look[0] - x).hypot(look[1] - y).max(1.0);
        look[2] = look[2].clamp(z - horizontal * 0.48, z + horizontal * 0.55);
        let lookahead =
            std::array::from_fn(|k| i.at[k] + (look[k] - i.at[k]) * (5.0 / horizontal).min(1.0));
        let blocked = !clear(lookahead);
        if blocked {
            look = [
                x + f[0] * 3.0 - f[1] * 4.0,
                y + f[1] * 3.0 + f[0] * 4.0,
                z + 2.0,
            ];
            if !clear(look) {
                look = [x - f[0] * 4.0, y - f[1] * 4.0, z + 1.0];
            }
        }
        let attack = !i.land
            && !blocked
            && self.peel.is_none()
            && self.evade.is_none()
            && height >= 2.8
            && aligned
            && (6.0..=18.0).contains(&range);
        let bomb = attack
            && i.has_bomb
            && range <= 13.0
            && target_delta[2] < -1.0
            && self.last_bomb.is_none_or(|t| !fresh(t, 90));
        if bomb {
            self.last_bomb = Some(i.tick);
        }
        let (stick, flip) = match self.evade.filter(|_| !i.land && !blocked) {
            Some((side, then)) => ([side, 0.0], fresh(then, 3)),
            None => (
                [0.0, if blocked || landing_here { 0.35 } else { 1.0 }],
                false,
            ),
        };
        Pilot {
            look,
            stick,
            boost: !i.land
                && !blocked
                && height > 2.5
                && self.evade.is_none()
                && (self.peel.is_some()
                    || range > 22.0 && dist > 18.0 && dx * f[0] + dy * f[1] > dist * 0.9),
            flip,
            cannon: attack && !bomb,
            bomb,
            landed: landing_here && height <= 0.9,
        }
    }
}

fn flight_clear(walls: &Walls, from: Vec3, to: Vec3, facing: [f32; 2]) -> bool {
    [-0.85, 0.0, 0.85].into_iter().all(|offset| {
        let shift = |p: Vec3| [p[0] - facing[1] * offset, p[1] + facing[0] * offset, p[2]];
        walls.clear(shift(from), shift(to))
    })
}

#[cfg(test)]
mod tests;
