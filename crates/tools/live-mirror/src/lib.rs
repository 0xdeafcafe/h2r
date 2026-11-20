//! Live game samples used by the editor's viewport and mirror API.
//! All guest reads belong to the game's tick observer. This crate consumes
//! the agent protocol's salted handles and world-space units.
mod stream;
pub use stream::Stream;

use agent_proto::live::{Frame, Header, Kind, Object, Player, World};
use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::time::Instant;

/// The same transforms and players the viewport and mirror.objects display.
#[derive(Clone, Debug)]
pub struct Sample {
    pub tick: u64,
    pub epoch: u64,
    pub stale: bool,
    pub objects: Vec<Object>,
    pub players: Vec<Player>,
}

#[derive(Clone)]
struct Received {
    header: Header,
    world: Arc<World>,
    arrived: Instant,
}

impl Received {
    fn sample(&self, stale: bool) -> Sample {
        Sample {
            tick: self.header.tick,
            epoch: self.header.epoch,
            stale,
            objects: self.world.objects.clone(),
            players: self.world.players.clone(),
        }
    }
}

/// Keeps the two delay intervals needed for smooth display. An exact query
/// can read any of those samples without interpolating.
#[derive(Clone)]
pub struct Store {
    samples: VecDeque<Received>,
    seq: Option<u64>,
    tick_rate: f64,
    every: u32,
}

impl Store {
    pub fn new(tick_rate: f64, every: u32) -> io::Result<Self> {
        if !tick_rate.is_finite() || tick_rate <= 0.0 || !(1..=30).contains(&every) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid live sample rate",
            ));
        }
        Ok(Self {
            samples: VecDeque::with_capacity(3),
            seq: None,
            tick_rate,
            every,
        })
    }

    /// A broken delta chain cannot mutate the last good sample. A key is
    /// independent and can always start a new chain after a gap or cut.
    pub fn apply(&mut self, frame: &Frame, arrived: Instant) -> io::Result<()> {
        let header = &frame.header;
        let prior = self.samples.back();
        let continuous = prior.is_some_and(|last| {
            self.seq
                .is_some_and(|seq| seq.wrapping_add(1) == header.seq)
                && last.header.epoch == header.epoch
                && last.header.tick < header.tick
                && !header.cut
        });
        if header.kind == Kind::Notice {
            self.seq = Some(header.seq);
            return Ok(());
        }
        if header.kind == Kind::Delta && !continuous {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "live delta without its base",
            ));
        }
        let mut world = if header.kind == Kind::Delta {
            prior.unwrap().world.as_ref().clone()
        } else {
            World::default()
        };
        frame.apply(&mut world)?;
        if !continuous {
            self.samples.clear();
        }
        if self.samples.len() == 3 {
            self.samples.pop_front();
        }
        self.samples.push_back(Received {
            header: *header,
            world: Arc::new(world),
            arrived,
        });
        self.seq = Some(header.seq);
        Ok(())
    }

    pub fn at_tick(&self, tick: u64) -> Option<Sample> {
        self.samples
            .iter()
            .rev()
            .find(|s| s.header.tick == tick)
            .map(|s| s.sample(false))
    }

    /// Render two sampling intervals behind the estimated game clock. The
    /// header tick measures simulation time, so arrival jitter never changes
    /// how much ground the velocity explains between two captures.
    pub fn sample(&self, now: Instant, latest: bool) -> Option<Sample> {
        let newest = self.samples.back()?;
        let age_ticks =
            now.saturating_duration_since(newest.arrived).as_secs_f64() * self.tick_rate;
        let interval = f64::from(self.every);
        let delay = 2.0 * interval;
        let stale = !newest.header.paused && age_ticks > delay + interval;
        if latest || newest.header.paused {
            return Some(newest.sample(stale));
        }

        let target = newest.header.tick as f64 + age_ticks - delay;
        if target >= newest.header.tick as f64 {
            let mut sample = newest.sample(stale);
            let seconds = (target - newest.header.tick as f64).min(interval) / self.tick_rate;
            // Velocity is world-space. Attached objects already move
            // with their parent; their stored transform is parent-local.
            for object in sample.objects.iter_mut().filter(|o| o.parent.is_none()) {
                for (v, axis) in object.velocity.iter().zip(&mut object.position) {
                    *axis += v * seconds as f32;
                }
            }
            return Some(sample);
        }
        let mut samples = self.samples.iter();
        let mut left = samples.next()?;
        if target <= left.header.tick as f64 {
            return Some(left.sample(false));
        }
        for right in samples {
            if target < right.header.tick as f64 {
                if target == left.header.tick as f64 {
                    return Some(left.sample(false));
                }
                let ticks = (right.header.tick - left.header.tick) as f64;
                let t = ((target - left.header.tick as f64) / ticks) as f32;
                let mut shown = left.sample(false);
                for object in &mut shown.objects {
                    if let Ok(i) = right
                        .world
                        .objects
                        .binary_search_by_key(&object.id, |o| o.id)
                    {
                        blend(
                            object,
                            &right.world.objects[i],
                            t,
                            (ticks / self.tick_rate) as f32,
                        );
                    }
                }
                for player in &mut shown.players {
                    if let Ok(i) = right
                        .world
                        .players
                        .binary_search_by_key(&player.index, |p| p.index)
                    {
                        let next = &right.world.players[i];
                        if player.unit == next.unit {
                            player.aim = unit(lerp(player.aim, next.aim, t)).unwrap_or(player.aim);
                        }
                    }
                }
                return Some(shown);
            }
            left = right;
        }
        Some(newest.sample(false))
    }
}

fn blend(object: &mut Object, next: &Object, t: f32, seconds: f32) {
    let displacement = std::array::from_fn(|i| next.position[i] - object.position[i]);
    let speed_squared =
        dot(object.velocity, object.velocity).max(dot(next.velocity, next.velocity));
    // Positions of attached objects are relative to their parent. Switching
    // coordinate spaces, like a teleport, takes effect on the new sample.
    if object.parent != next.parent
        || dot(displacement, displacement) > 16.0 * speed_squared * seconds * seconds
    {
        return;
    }
    object.position = lerp(object.position, next.position, t);
    object.scale += t * (next.scale - object.scale);
    if object.forward == next.forward && object.up == next.up {
        return;
    }
    if let (Some(a), Some(mut b)) = (rotation(object), rotation(next)) {
        // q and -q describe the same orientation. Keep the shorter rotation.
        if a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() < 0.0 {
            b = b.map(|v| -v);
        }
        let q: [f32; 4] = std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        let length = q.iter().map(|v| v * v).sum::<f32>().sqrt();
        let [x, y, z, w] = q.map(|v| v / length);
        object.forward = [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ];
        object.up = [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ];
    }
}

/// Rotation columns in the game's order (forward, left, up) to a unit
/// quaternion. Choosing the largest component also handles a half turn.
fn rotation(object: &Object) -> Option<[f32; 4]> {
    let forward = unit(object.forward)?;
    let left = unit(cross(object.up, forward))?;
    let axes = [forward, left, cross(forward, left)];
    let trace = axes[0][0] + axes[1][1] + axes[2][2];
    let q = if trace > 0.0 {
        [
            axes[1][2] - axes[2][1],
            axes[2][0] - axes[0][2],
            axes[0][1] - axes[1][0],
            1.0 + trace,
        ]
    } else {
        let i = if axes[0][0] >= axes[1][1] && axes[0][0] >= axes[2][2] {
            0
        } else if axes[1][1] >= axes[2][2] {
            1
        } else {
            2
        };
        let j = (i + 1) % 3;
        let k = (i + 2) % 3;
        let mut q = [0.0; 4];
        q[i] = 1.0 + axes[i][i] - axes[j][j] - axes[k][k];
        q[j] = axes[i][j] + axes[j][i];
        q[k] = axes[i][k] + axes[k][i];
        q[3] = axes[j][k] - axes[k][j];
        q
    };
    let length = q.iter().map(|v| v * v).sum::<f32>().sqrt();
    Some(q.map(|v| v / length))
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let length = dot(v, v).sqrt();
    (length > 1e-6 && length.is_finite()).then(|| v.map(|x| x / length))
}

fn lerp(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + t * (b[i] - a[i]))
}

#[cfg(test)]
mod tests;
