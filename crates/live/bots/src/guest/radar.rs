//! A phantom has no local HUD slot. Reconstruct the movement-only dots of
//! its motion sensor from the same variant, range and speed threshold as
//! the HUD (0x1a1d55, 0x1a264e). Reduce a dot to a rough horizontal point
//! before passing it to the brain; it can never supply a combat target.

use super::{GuestMemory, OPTIONS, Perception, objects, points};
use crate::brain::Vec3;
use crate::objective::Objective;
use live_proto::hopper::variant;

pub(super) const UNIT_ZOOM: u32 = 0x241;
const VARIANT: u32 = 0x134;
const JUGGERNAUT_SENSOR: u32 = 1;
const SENSOR_RANGE: u32 = 0x290;
const MATG_PLAYER_CONTROL: u32 = 0x134;
const WALK_SPEED: u32 = 0x3c;
// The factors used at 0x1a1d94 and 0x1a1dd9.
const VERTICAL_FACTOR: u32 = 0x0045_e1f4;
const SPEED_FACTOR: u32 = 0x0045_e1f0;
const VELOCITY: u32 = 0x88;
const PHYSICS_FLAGS_HIGH: u32 = 0xc1;

pub(super) fn enabled(
    m: &dyn GuestMemory,
    globals: u32,
    flags: u32,
    objective: Option<&Objective>,
) -> bool {
    if objective.is_some_and(Objective::am_juggernaut) {
        let traits = globals + OPTIONS + VARIANT + variant::TYPE_AREA as u32;
        m.u32(traits).is_ok_and(|f| f & JUGGERNAUT_SENSOR != 0)
    } else {
        flags & variant::flag::MOTION_SENSOR != 0
    }
}

#[derive(Clone, Copy)]
pub(super) struct Sensor {
    range: f32,
    threshold: f32,
    vertical: f32,
}

impl Sensor {
    pub(super) fn read(m: &dyn GuestMemory) -> Option<Self> {
        let hud = m.u32(points::HUD_GLOBALS_TAG.base).ok().filter(|&p| p != 0)?;
        let matg = m.u32(points::GLOBALS_TAG.base).ok().filter(|&p| p != 0)?;
        let control = m.u32(matg.checked_add(MATG_PLAYER_CONTROL)?).ok().filter(|&p| p != 0)?;
        let range = m.f32(hud.checked_add(SENSOR_RANGE)?).ok()?;
        let vertical = m.f32(VERTICAL_FACTOR).ok()?;
        let threshold = m.f32(control.checked_add(WALK_SPEED)?).ok()? * m.f32(SPEED_FACTOR).ok()?;
        if !range.is_finite() || range <= 0.0 || !vertical.is_finite()
            || !threshold.is_finite() || threshold <= 0.0
        {
            return None;
        }
        Some(Self { range, threshold, vertical })
    }

    pub(super) fn moving(self, m: &dyn GuestMemory, mut body: objects::Body, crouching: bool) -> bool {
        if crouching || m.u8(body.addr + PHYSICS_FLAGS_HIGH).is_ok_and(|f| f & 1 != 0) {
            return false;
        }
        // A rider moves with the root vehicle. Resolve the same short
        // vehicle/turret chain as vehicle::read::unit_position.
        for _ in 0..4 {
            let Ok(Some(parent)) = body.parent(m) else { break };
            let Ok(Some((objects::VEHICLE, mount))) = objects::get(m, parent) else { break };
            body = mount;
        }
        m.vec3(body.addr + VELOCITY).is_ok_and(|[x, y, z]| {
            let speed = x.hypot(y).hypot(z * self.vertical);
            speed.is_finite() && speed >= self.threshold
        })
    }

    pub(super) fn contact(self, p: &Perception, moving: u16) -> Option<Vec3> {
        let me = p.me?;
        let contacts = p.others.iter().filter_map(|o| {
            let enemy = p.objective.as_ref()
                .and_then(|g| g.juggernaut_opponent(o.id))
                .unwrap_or(!p.teams || o.team != me.team);
            if !enemy || moving & crate::perceive::player_bit(o.id) == 0 {
                return None;
            }
            let dx = o.position[0] - me.position[0];
            let dy = o.position[1] - me.position[1];
            let distance = dx.hypot(dy);
            (distance < self.range).then_some((dx, dy, distance))
        });
        let (dx, dy, _) = contacts.min_by(|a, b| a.2.total_cmp(&b.2))?;
        // Coarsen the sensor disc to half-unit steps. Two dots on the
        // same horizontal bearing have no separable floor or player ID.
        Some([
            me.position[0] + (2.0 * dx).round() * 0.5,
            me.position[1] + (2.0 * dy).round() * 0.5,
            me.position[2],
        ])
    }
}
