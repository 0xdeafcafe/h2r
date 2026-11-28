//! Trigger volumes and players 3 to 16 (docs/live/live-coop-join.md 13.2).
//!
//! The scripts gate progress on `(volume_test_objects v (players))`, which
//! counts every player. Two rules keep the squad together around those
//! gates:
//!
//! * **Gate reached.** When the scripts' `player0` or `player1` enters a
//!   trigger volume neither was in on the last player update, the
//!   stragglers (players 3+ past [`LEASH`](super::LEASH)) are brought over.
//! * **Gate run ahead.** A player 3+ past the leash who stands in a trigger
//!   volume neither scripts' player is in is brought back before the
//!   player update, so a straggler or a runner does not trip it alone.
//!   [guess] The player update (`0x13801f`) runs before the scripts in
//!   `game_tick`, so the scripts never see it inside.
//!
//! Kill-trigger volumes and BSP-switch volumes are not gates (the BSP
//! switch has its own gather). A volume parented to an object moves with
//! it and its stored frame is not in world space: it is left out.

use guest_layout::memory::GuestMemory;
use guest_layout::points;

use super::{FIRST_EXTRA, LEASH, NONE, Player, distance, object, script_players, world_position};

/// Scenario blocks (count, address): trigger volumes (`0x44` each), kill
/// triggers (a `u16` volume each) and BSP switch volumes (`0xe` each, the
/// volume an `i16` at `+0`). Read in docs/editor/scenario-editor.md and
/// `debug_views/debug_static.rs`.
pub const TRIGGER_VOLUMES: u32 = 0x108;
pub const VOLUME_STRIDE: u32 = 0x44;
pub const KILL_TRIGGERS: u32 = 0x230;
pub const BSP_SWITCHES: u32 = 0x130;
pub const BSP_SWITCH_STRIDE: u32 = 0xe;
/// A volume: parent object (`i16`, -1 for none) at `+4`; forward `+0xc`,
/// up `+0x18`, the origin corner `+0x24`, the full size along forward,
/// left and up `+0x30`.
pub const VOLUME_PARENT: u32 = 4;
pub const VOLUME_FORWARD: u32 = 0xc;
pub const VOLUME_UP: u32 = 0x18;
pub const VOLUME_ORIGIN: u32 = 0x24;
pub const VOLUME_SIZE: u32 = 0x30;
/// More volumes than any level has; a bad count reads no further.
pub const MAX_VOLUMES: u32 = 1024;

/// A box: the origin corner, its forward, left and up axes (unit length),
/// the full size along each.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    pub index: u16,
    pub origin: [f32; 3],
    pub axes: [[f32; 3]; 3],
    pub size: [f32; 3],
}

fn cross([x, y, z]: [f32; 3], [a, b, c]: [f32; 3]) -> [f32; 3] {
    [y * c - z * b, z * a - x * c, x * b - y * a]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(v: [f32; 3]) -> Option<[f32; 3]> {
    let len = dot(v, v).sqrt();
    (len > 1e-6 && len.is_finite()).then(|| v.map(|x| x / len))
}

impl Volume {
    /// A box from the stored frame, as the debug view draws it; `None` when
    /// degenerate.
    pub fn new(
        index: u16,
        origin: [f32; 3],
        forward: [f32; 3],
        up: [f32; 3],
        size: [f32; 3],
    ) -> Option<Volume> {
        if !size.iter().all(|v| *v > 0.0 && v.is_finite()) || !origin.iter().all(|v| v.is_finite())
        {
            return None;
        }
        let forward = unit(forward)?;
        let left = unit(cross(unit(up)?, forward))?;
        Some(Volume {
            index,
            origin,
            axes: [forward, left, cross(forward, left)],
            size,
        })
    }

    pub fn contains(&self, p: [f32; 3]) -> bool {
        let d = [
            p[0] - self.origin[0],
            p[1] - self.origin[1],
            p[2] - self.origin[2],
        ];
        self.axes.iter().zip(self.size).all(|(&axis, len)| {
            let t = dot(d, axis);
            (0.0..=len).contains(&t)
        })
    }
}

fn indices<M: GuestMemory + ?Sized>(m: &M, s: u32, block: u32, stride: u32) -> Vec<u16> {
    let (Ok(n), Ok(base)) = (
        m.i32(s.wrapping_add(block)),
        m.u32(s.wrapping_add(block + 4)),
    ) else {
        return Vec::new();
    };
    (0..(n.max(0) as u32).min(MAX_VOLUMES))
        .map_while(|i| m.u16(base.wrapping_add(i * stride)).ok())
        .collect()
}

/// The loaded scenario's gate volumes: unparented trigger volumes that are
/// neither kill triggers nor BSP switches. None when there is no scenario
/// or a block does not read.
pub fn gates<M: GuestMemory + ?Sized>(m: &M) -> Vec<Volume> {
    let Ok(s) = m.u32(points::SCENARIO.base) else {
        return Vec::new();
    };
    if s == 0 {
        return Vec::new();
    }
    let (Ok(n), Ok(base)) = (
        m.i32(s.wrapping_add(TRIGGER_VOLUMES)),
        m.u32(s.wrapping_add(TRIGGER_VOLUMES + 4)),
    ) else {
        return Vec::new();
    };
    let mut skip = indices(m, s, KILL_TRIGGERS, 2);
    skip.extend(indices(m, s, BSP_SWITCHES, BSP_SWITCH_STRIDE));
    let mut out = Vec::new();
    for i in 0..(n.max(0) as u32).min(MAX_VOLUMES) {
        if skip.contains(&(i as u16)) {
            continue;
        }
        let at = base.wrapping_add(i * VOLUME_STRIDE);
        let (Ok(parent), Ok(f), Ok(u), Ok(o), Ok(z)) = (
            m.u16(at + VOLUME_PARENT),
            m.vec3(at + VOLUME_FORWARD),
            m.vec3(at + VOLUME_UP),
            m.vec3(at + VOLUME_ORIGIN),
            m.vec3(at + VOLUME_SIZE),
        ) else {
            break;
        };
        if (parent as i16) >= 0 {
            continue;
        }
        out.extend(Volume::new(i as u16, o, f, u, z));
    }
    out
}

/// The volumes containing `p`, by index.
pub fn inside(vols: &[Volume], p: [f32; 3]) -> Vec<u16> {
    vols.iter()
        .filter(|v| v.contains(p))
        .map(|v| v.index)
        .collect()
}

/// The volumes either scripts' player stands in, sorted, without repeats.
pub fn named_inside<M: GuestMemory + ?Sized>(m: &M, ps: &[Player], vols: &[Volume]) -> Vec<u16> {
    let mut out: Vec<u16> = script_players(ps)
        .iter()
        .filter_map(|p| world_position(m, p.unit))
        .flat_map(|at| inside(vols, at))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Whether a scripts' player entered a volume since the last update.
/// `prev` is `None` on the first update this console sees: nothing is
/// entered then, so a console that joins does not gather alone.
pub fn entered(prev: Option<&[u16]>, now: &[u16]) -> bool {
    prev.is_some_and(|prev| now.iter().any(|v| !prev.contains(v)))
}

/// Players 3+ alive on foot, past the leash from `at`, inside a volume
/// neither scripts' player is in (handles, in table order).
pub fn ahead<M: GuestMemory + ?Sized>(
    m: &M,
    ps: &[Player],
    vols: &[Volume],
    at: [f32; 3],
) -> Vec<u32> {
    let named = named_inside(m, ps, vols);
    ps.iter()
        .filter(|p| p.index >= FIRST_EXTRA && p.unit != NONE)
        .filter(|p| {
            object(m, p.unit).is_some_and(|o| {
                o.parent == NONE
                    && distance(o.position, at) > LEASH
                    && inside(vols, o.position).iter().any(|v| !named.contains(v))
            })
        })
        .map(|p| p.handle)
        .collect()
}
