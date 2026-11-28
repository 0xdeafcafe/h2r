//! Read retail control state. The only writes here are to a private copy
//! of a render view and the render call's argument naming that copy.
use std::f32::consts::{FRAC_PI_2, TAU};

use guest_layout::view::{Record, objects, players};
use guest_layout::{DatumHandle, GuestMemory, points};
use live_telemetry::layout;

use super::prediction::Aim;
use crate::{coop_jip::at::PLAYERS_GLOBALS, touch_glue as pc};

pub const STRIDE: u32 = engine::render_loop::views::VIEW_STRIDE;
pub const FIRST_PERSON: u32 = 0x0023_c110;
const DIRECTOR_CALLBACK: u32 = 0x004e_8c50;

pub fn ptr(m: &dyn GuestMemory, at: u32) -> Option<u32> {
    m.u32(at).ok().filter(|p| *p != 0 && *p != u32::MAX)
}

pub fn mode(m: &dyn GuestMemory) -> Option<u32> {
    m.u32(ptr(m, points::SIM_GLOBALS.base)?.checked_add(8)?)
        .ok()
}

pub(super) fn angles(m: &dyn GuestMemory, at: u32) -> Option<Aim> {
    let yaw = m.f32(at).ok()?;
    let pitch = m.f32(at.checked_add(4)?).ok()?;
    (yaw.is_finite() && yaw.abs() <= TAU + 0.001 && pitch.abs() <= FRAC_PI_2 + 0.001)
        .then_some(Aim { yaw, pitch })
}

pub fn control(m: &dyn GuestMemory, user: usize) -> Option<u32> {
    ptr(m, pc::PLAYER_CONTROL)?.checked_add(pc::PC_HEADER + pc::PC_STRIDE * user as u32)
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(remote = "DatumHandle")]
struct FilmDatum(u32);

#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Subject {
    #[serde(with = "FilmDatum")]
    pub player: DatumHandle,
    #[serde(with = "FilmDatum")]
    pub unit: DatumHandle,
    pub position: [f32; 3],
    pub zoom: u16,
}

impl Subject {
    pub fn cut(self, other: Self) -> bool {
        self.player != other.player
            || self.unit != other.unit
            || self.zoom != other.zoom
            || self
                .position
                .into_iter()
                .zip(other.position)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f32>()
                > 4.0
    }
}

pub fn subject(m: &dyn GuestMemory, user: usize) -> Result<Subject, &'static str> {
    read_subject(m, user).unwrap_or(Err("no local control"))
}

fn read_subject(m: &dyn GuestMemory, user: usize) -> Option<Result<Subject, &'static str>> {
    if user >= 4 {
        return None;
    }
    let cinematic = ptr(m, layout::CINEMATIC_GLOBALS_PTR)?;
    if m.u8(cinematic.checked_add(layout::CINEMATIC_IN_PROGRESS)?)
        .ok()?
        != 0
    {
        return Some(Err("cinematic"));
    }
    let id = m
        .u32(ptr(m, PLAYERS_GLOBALS)?.checked_add(12 + 4 * user as u32)?)
        .ok()?;
    let id = DatumHandle::from_raw(id)?;
    if id.index() >= 16 {
        return None;
    }
    let p = players::array(m).ok()??.get(m, id).ok()??;
    if p.local_user(m).ok()? != user as i16 || !p.listed(m).ok()? {
        return None;
    }
    let Some(unit) = p.unit(m).ok()? else {
        return Some(Err("dead"));
    };
    let (kind, body) = objects::get(m, unit).ok()??;
    if body.damage_flags(m).ok()? & layout::BODY_DEPLETED != 0 {
        return Some(Err("dead"));
    }
    if kind != objects::BIPED || body.parent(m).ok()?.is_some() {
        return Some(Err("vehicle"));
    }
    if m.u32(body.addr().checked_add(layout::UNIT_PLAYER)?).ok()? != id.raw()
        || m.u32(control(m, user)?).ok()? != unit.raw()
    {
        return None;
    }
    let stride = pc::LOOK_LOCKED_STRIDE * user as u32;
    if m.u8(pc::LOOK_LOCKED + stride).ok()? != 0
        || m.u32(DIRECTOR_CALLBACK + stride).ok()? != FIRST_PERSON
    {
        return Some(Err("director"));
    }
    let position = body.position(m).ok()?;
    if !position.into_iter().all(f32::is_finite) {
        return None;
    }
    Some(Ok(Subject {
        player: id,
        unit,
        position,
        zoom: m.u16(control(m, user)?.checked_add(0x2e)?).ok()?,
    }))
}

fn vector(record: &[u8], at: usize) -> [f32; 3] {
    std::array::from_fn(|i| {
        f32::from_le_bytes(record[at + i * 4..at + i * 4 + 4].try_into().unwrap())
    })
}

pub(super) fn facing(v: [f32; 3]) -> Option<Aim> {
    let [x, y, z] = v;
    let length = x * x + y * y + z * z;
    (0.96..=1.04).contains(&length).then(|| Aim {
        yaw: y.atan2(x).rem_euclid(TAU),
        pitch: z.atan2(x.hypot(y)),
    })
}

/// Rotate both axes rigidly about camera-left, then about world-up.
/// Camera roll, position, weapon FOV, viewports and clip distances survive.
fn rotate(record: &mut [u8], camera: usize, offset: Aim) -> Option<()> {
    let forward = vector(record, camera + 12);
    let yaw = facing(forward)?.yaw;
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = offset.pitch.sin_cos();
    let (sd, cd) = offset.yaw.sin_cos();
    for at in [camera + 12, camera + 24] {
        let [x, y, z] = vector(record, at);
        let horizontal = cy * x + sy * y;
        let side = -sy * x + cy * y;
        let h = cp * horizontal - sp * z;
        let (x, y) = (cy * h - sy * side, sy * h + cy * side);
        let v = [cd * x - sd * y, sd * x + cd * y, sp * horizontal + cp * z];
        if !v.into_iter().all(f32::is_finite) {
            return None;
        }
        for (i, value) in v.into_iter().enumerate() {
            record[at + 4 * i..at + 4 * i + 4].copy_from_slice(&value.to_le_bytes());
        }
    }
    Some(())
}

pub(super) fn draw_copy(
    m: &mut dyn GuestMemory,
    arg: u32,
    original: u32,
    private: u32,
    offset: Aim,
) -> Option<()> {
    let mut record = [0u8; STRIDE as usize];
    m.read(original, &mut record).ok()?;
    rotate(&mut record, 0x0c, offset)?;
    rotate(&mut record, 0x80, offset)?;
    m.write(private, &record).ok()?;
    m.set_u32(arg, private).ok()
}
