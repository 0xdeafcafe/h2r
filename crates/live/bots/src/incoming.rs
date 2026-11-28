//! Live grenades near the bot and getting away from them: seeing one land
//! close and stepping or jumping clear. Throwing is [`crate::grenades`].

use guest_layout::handle::DatumHandle;
use guest_layout::memory::GuestMemory;

use crate::brain::{Me, Vec3};
use crate::cache::MapData;
use crate::mind::{EYE, NOTICE_CONE, Skill};
use crate::moves;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Grenade {
    pub(crate) id: u32,
    pub(crate) at: Vec3,
}

/// The loaded globals' grenade block supplies the projectile tag IDs.
const GLOBALS_TAG: u32 = 0x004e_034c;
const GRENADES: u32 = 0x100;
const GRENADE_STRIDE: u32 = 0x2c;
const PROJECTILE_DATUM: u32 = 0x28;
pub(crate) const PROJECTILE: u8 = 5;
pub(crate) const DANGER: f32 = 3.2;

/// Equipment datum in matg's grenade record, alongside the projectile datum.
const EQUIPMENT_DATUM: u32 = 0x20;

pub(crate) fn grenade_defs(m: &dyn GuestMemory) -> Option<[Option<DatumHandle>; 2]> {
    grenade_references(m, PROJECTILE_DATUM)
}

pub(crate) fn equipment_defs(m: &dyn GuestMemory) -> Option<[Option<DatumHandle>; 2]> {
    grenade_references(m, EQUIPMENT_DATUM)
}

fn grenade_references(m: &dyn GuestMemory, field: u32) -> Option<[Option<DatumHandle>; 2]> {
    let matg = m.u32(GLOBALS_TAG).ok().filter(|&a| a != 0)?;
    let count = m.u32(matg.checked_add(GRENADES)?).ok()?;
    let base = m
        .u32(matg.checked_add(GRENADES + 4)?)
        .ok()
        .filter(|&a| a != 0)?;
    Some(std::array::from_fn(|i| {
        if i as u32 >= count {
            return None;
        }
        DatumHandle::from_raw(
            m.u32(base.checked_add(i as u32 * GRENADE_STRIDE + field)?)
                .ok()?,
        )
    }))
}

fn offset(from: Vec3, to: Vec3) -> Vec3 {
    [to[0] - from[0], to[1] - from[1], to[2] - from[2]]
}

fn length([x, y, z]: Vec3) -> f32 {
    x.hypot(y).hypot(z)
}

#[derive(Clone, Copy, Debug)]
struct Notice {
    id: u32,
    first: u32,
    last: u32,
    jumped: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Evasion(Option<Notice>);

impl Evasion {
    /// Dodge a nearby grenade after noticing it in the bot's view. Check
    /// the floor before jumping and issue one jump per continuous sighting.
    pub(crate) fn dodge(
        &mut self,
        tick: u32,
        skill: Skill,
        me: &Me,
        map: Option<&MapData>,
        grenades: &[Grenade],
    ) -> Option<([f32; 2], bool)> {
        let eye = [me.position[0], me.position[1], me.position[2] + EYE];
        let danger = grenades
            .iter()
            .map(|g| (g, length(offset(me.position, g.at))))
            .filter(|&(g, d)| {
                let from = offset(eye, g.at);
                let dot = from[0] * me.aim[0] + from[1] * me.aim[1] + from[2] * me.aim[2];
                d < DANGER
                    && (d < 0.8 || dot > length(from) * NOTICE_CONE.cos())
                    && map.is_none_or(|m| m.walls.clear(eye, g.at))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        let (g, distance) = danger?;
        if self
            .0
            .is_none_or(|n| n.id != g.id || tick.wrapping_sub(n.last) > 8)
        {
            self.0 = Some(Notice {
                id: g.id,
                first: tick,
                last: tick,
                jumped: false,
            });
        }
        let notice = self.0.as_mut()?;
        notice.last = tick;
        let delay = match skill {
            Skill::Easy => 12,
            Skill::Normal => 9,
            Skill::Heroic => 7,
            Skill::Legendary => 5,
        };
        if tick.wrapping_sub(notice.first) < delay {
            return None;
        }
        let [x, y, _] = offset(g.at, me.position);
        let yaw = if x.hypot(y) < 0.05 {
            me.aim[1].atan2(me.aim[0])
        } else {
            y.atan2(x)
        };
        let walk = [0.0, 0.65, -0.65]
            .into_iter()
            .map(|turn| [(yaw + turn).cos(), (yaw + turn).sin()])
            .find(|&dir| map.is_none_or(|m| moves::standable(m, me.position, dir)))?;
        // Check the jump's longer carry as well as overhead clearance.
        let jump = distance < 2.0
            && !notice.jumped
            && map.is_some_and(|m| {
                let high = [eye[0], eye[1], eye[2] + 0.65];
                let ahead = [high[0] + walk[0] * 0.7, high[1] + walk[1] * 0.7, high[2]];
                moves::jumpable(m, me.position, walk)
                    && m.walls.clear(eye, high)
                    && m.walls.clear(high, ahead)
            });
        notice.jumped |= jump;
        Some((walk, jump))
    }
}

#[cfg(test)]
mod tests;
