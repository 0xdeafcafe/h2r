//! Where players 3 to 16 stand (docs/live/live-coop-join.md 13.2): a grid
//! of spots in rings around a point, none closer than [`MIN_GAP`] to a
//! player's unit, and the group's anchor for a respawn.
//!
//! Every value here is a sum, product or square root of guest floats taken
//! in table order, so every console computes the same spots bit for bit.

use guest_layout::memory::GuestMemory;

use super::{NONE, Player, distance, object, world_position};

/// Distance (world units) between two grid spots. [guess] A biped's
/// standing capsule is about 0.2 units across; 0.6 leaves a gap.
pub const SPACING: f32 = 0.6;
/// Rings of the grid around a point: 8, 16 and 24 spots.
pub const RINGS: i32 = 3;
/// No spot is taken nearer than this to any player's unit.
pub const MIN_GAP: f32 = 0.45;

/// Ring `r` (1 or more) of the square grid: every `(x, y)` with
/// `max(|x|, |y|) == r`, nearest first, then by `y`, then by `x`.
pub fn ring(r: i32) -> Vec<(i32, i32)> {
    let mut out: Vec<(i32, i32)> = (-r..=r)
        .flat_map(|y| (-r..=r).map(move |x| (x, y)))
        .filter(|&(x, y)| x.abs().max(y.abs()) == r)
        .collect();
    out.sort_by_key(|&(x, y)| (x * x + y * y, y, x));
    out
}

/// A grid spot `(x, y)` steps from `at`, at `at`'s height.
pub fn offset(at: [f32; 3], (x, y): (i32, i32)) -> [f32; 3] {
    [
        at[0] + x as f32 * SPACING,
        at[1] + y as f32 * SPACING,
        at[2],
    ]
}

/// A spot at a level start: on ring `ring` around starting location
/// `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub start: u16,
    pub ring: i32,
    pub at: [f32; 3],
}

/// The level-start spots, in the order they are tried: ring 1 before ring
/// 2, and in a ring each grid step around starting location 0 and then
/// around 1, so the squad fills both spots evenly. Only the first two
/// starting locations are used (every campaign level has two; 00a one).
pub fn start_candidates(starts: &[[f32; 3]]) -> Vec<Candidate> {
    let starts = &starts[..starts.len().min(2)];
    let mut out = Vec::new();
    for r in 1..=RINGS {
        for step in ring(r) {
            for (s, &at) in starts.iter().enumerate() {
                out.push(Candidate {
                    start: s as u16,
                    ring: r,
                    at: offset(at, step),
                });
            }
        }
    }
    out
}

/// Every player's unit position (a rider's is its vehicle's), but `except`'s
/// (a player handle).
pub fn unit_positions<M: GuestMemory + ?Sized>(m: &M, ps: &[Player], except: u32) -> Vec<[f32; 3]> {
    ps.iter()
        .filter(|p| p.handle != except && p.unit != NONE)
        .filter_map(|p| world_position(m, p.unit))
        .collect()
}

/// Whether `at` is at least [`MIN_GAP`] from every one of `units`.
pub fn clear_of(at: [f32; 3], units: &[[f32; 3]]) -> bool {
    units.iter().all(|&u| distance(at, u) >= MIN_GAP)
}

/// The grid spots around `at` clear of `units`, ring by ring: `(ring, spot)`.
pub fn spots_around(at: [f32; 3], units: &[[f32; 3]]) -> Vec<(i32, [f32; 3])> {
    (1..=RINGS)
        .flat_map(|r| ring(r).into_iter().map(move |s| (r, offset(at, s))))
        .filter(|&(_, p)| clear_of(p, units))
        .collect()
}

/// The respawn's anchor: of the living players on foot (not `except`, a
/// player handle), the one nearest the middle of every living player's
/// position, the lower index on a tie. `(unit, position)`.
pub fn group_anchor<M: GuestMemory + ?Sized>(
    m: &M,
    ps: &[Player],
    except: u32,
) -> Option<(u32, [f32; 3])> {
    let alive: Vec<(&Player, [f32; 3])> = ps
        .iter()
        .filter(|p| p.handle != except && p.unit != NONE)
        .filter_map(|p| Some((p, world_position(m, p.unit)?)))
        .collect();
    if alive.is_empty() {
        return None;
    }
    let mut sum = [0.0f32; 3];
    for (_, at) in &alive {
        for k in 0..3 {
            sum[k] += at[k];
        }
    }
    let n = alive.len() as f32;
    let middle = sum.map(|v| v / n);
    let mut best: Option<(u32, [f32; 3], f32)> = None;
    for (p, at) in alive {
        if object(m, p.unit).is_none_or(|o| o.parent != NONE) {
            continue;
        }
        let d = distance(at, middle);
        if best.is_none_or(|(_, _, bd)| d < bd) {
            best = Some((p.unit, at, d));
        }
    }
    best.map(|(u, at, _)| (u, at))
}
