use agent_proto::route::Sample;
use bots::autonav::{self, MAP_HEADER_LEN};
use bots::cache::Stamp;
use bots::guest::{self, BODY_DEPLETED};
use guest_layout::GuestMemory;
use guest_layout::view::{objects, players};

use crate::telemetry::layout;

// The resident cache header's name is at +0x198.
const CACHE_HEADER: u32 = layout::CACHE_MAP_NAME - 0x198;
const CACHE_CHECKSUM: u32 = CACHE_HEADER + 0x2d0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Region {
    map: [u8; 32],
    checksum: u32,
    pub bsp: u16,
}

impl Region {
    pub fn read(m: &dyn GuestMemory) -> Option<Self> {
        let globals = globals(m)?;
        let bsp = m.u16(globals + layout::GLOBALS_ACTIVE_BSP).ok()?;
        let bsp = u16::try_from(bsp as i16).ok()?;
        let mut map = [0; 32];
        m.read(layout::CACHE_MAP_NAME, &mut map).ok()?;
        if map[0] == 0 {
            return None;
        }
        let checksum = m.u32(CACHE_CHECKSUM).ok()?;
        Some(Self { map, checksum, bsp })
    }

    pub fn name(&self) -> String {
        let end = self
            .map
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(self.map.len());
        String::from_utf8_lossy(&self.map[..end]).into_owned()
    }
}

pub(super) fn map_stamp(m: &dyn GuestMemory) -> Option<Stamp> {
    let mut bytes = [0; MAP_HEADER_LEN];
    m.read(CACHE_HEADER, &mut bytes).ok()?;
    Some(Stamp::now(autonav::fingerprint(&bytes)))
}

fn globals(m: &dyn GuestMemory) -> Option<u32> {
    m.u32(layout::GAME_GLOBALS_PTR).ok().filter(|&p| p != 0)
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Biped {
    pub unit: u32,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    pub aim: [f32; 3],
    pub airborne: bool,
    pub supported: bool,
    pub crouch: f32,
    pub crouch_request: bool,
    pub ticks_per_second: f32,
}

impl Biped {
    pub fn read(m: &dyn GuestMemory, port: u32) -> Option<Self> {
        let players = players::array(m).ok()??;
        let player = players
            .live_iter(m, 16)
            .ok()?
            .flatten()
            .find(|(_, player)| {
                player.listed(m) == Ok(true)
                    && player.local_user(m).is_ok_and(|user| user >= 0)
                    && m.u32(player.addr + layout::PLAYER_CONTROLLER) == Ok(port)
            })?
            .1;
        let handle = player.unit(m).ok()??;
        let (kind, body) = objects::get(m, handle).ok()??;
        if kind != objects::BIPED
            || body.damage_flags(m).ok()? & BODY_DEPLETED != 0
            || m.u16(body.addr + layout::UNIT_SEAT).ok()? as i16 != -1
        {
            return None;
        }

        let position = body.position(m).ok()?;
        let velocity = m.vec3(body.addr + layout::BODY_VELOCITY).ok()?;
        let aim = m.vec3(body.addr + guest::UNIT_AIM).ok()?;
        let crouch = m.f32(body.addr + layout::UNIT_CROUCH).ok()?;
        if !position
            .into_iter()
            .chain(velocity)
            .chain(aim)
            .chain([crouch])
            .all(f32::is_finite)
        {
            return None;
        }
        let biped_flags = m.u16(body.addr + layout::BIPED_FLAGS).ok()?;
        let support_flags = m.u8(body.addr + layout::BIPED_SUPPORT_FLAGS).ok()?;
        let unit_flags = m.u32(body.addr + layout::UNIT_FLAGS).ok()?;
        let rate = m
            .u16(globals(m)? + layout::OPTIONS + layout::OPTIONS_TICK_RATE)
            .ok()?;
        Some(Self {
            unit: handle.raw(),
            position,
            velocity,
            aim,
            airborne: biped_flags & 1 != 0,
            supported: support_flags & layout::BIPED_SUPPORT_COPIED != 0,
            crouch,
            crouch_request: unit_flags & layout::UNIT_FLAG_CROUCH != 0,
            ticks_per_second: f32::from(rate.max(1)),
        })
    }

    pub fn yaw(self) -> f32 {
        self.aim[1].atan2(self.aim[0])
    }

    pub fn horizontal_speed(self) -> f32 {
        self.velocity[0].hypot(self.velocity[1])
    }

    pub fn sample(self) -> Sample {
        let [vx, vy, vz] = self.velocity;
        Sample {
            position: self.position,
            velocity: self.velocity,
            speed: vx.hypot(vy).hypot(vz),
        }
    }
}
