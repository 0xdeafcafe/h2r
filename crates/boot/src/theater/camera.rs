//! Replay cameras replace the renderer's view argument. The replay never
//! creates a local player or changes a director, controller or unit.

use agent_proto::theater::{Camera, CameraMode};
use engine::{GuestMemory, points, render_loop::views::VIEW_STRIDE};
use feature::{Builder, Flow};
use serde::{Deserialize, Serialize};

use super::{Mode, Session};

const PITCH_LIMIT: f32 = 1.55;
const DEFAULT_RADIUS: f32 = 4.0;
const RENDER_CAMERA_BYTES: usize = 0x74;
const WORLD_VIEW_RETURN: u32 = 0x0022_3719;
const NEAR_CLIP: u32 = 0x0048_5ad4;
const FAR_CLIP: u32 = 0x0048_5ad8;
const RANDOM_STATE: u32 = 0x004e_7408;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
struct Pose {
    position: [f32; 3],
    yaw: f32,
    pitch: f32,
}

impl Pose {
    fn looking(position: [f32; 3], direction: [f32; 3]) -> Option<Self> {
        let [x, y, z] = direction;
        let planar = x.hypot(y);
        (position.into_iter().chain(direction).all(f32::is_finite) && planar.hypot(z) > 0.0001)
            .then(|| Self {
                position,
                yaw: y.atan2(x),
                pitch: z.atan2(planar).clamp(-PITCH_LIMIT, PITCH_LIMIT),
            })
    }

    fn axes(self) -> ([f32; 3], [f32; 3]) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        ([cy * cp, sy * cp, sp], [-cy * sp, -sy * sp, cp])
    }

    fn rotate(&mut self, look: [f32; 2]) {
        self.yaw = (self.yaw + look[0]).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + look[1]).clamp(-PITCH_LIMIT, PITCH_LIMIT);
    }
}

/// Camera state saved when a seek starts a new replay process.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReplayCamera {
    pub(super) mode: CameraMode,
    pub(super) player: Option<String>,
    pose: Option<Pose>,
    orbit_yaw: f32,
    orbit_pitch: f32,
    radius: f32,
}

impl Default for ReplayCamera {
    fn default() -> Self {
        Self {
            mode: CameraMode::Free,
            player: None,
            pose: None,
            orbit_yaw: 0.0,
            orbit_pitch: -0.28,
            radius: DEFAULT_RADIUS,
        }
    }
}

impl ReplayCamera {
    pub(super) fn control(&mut self, request: Camera) -> Result<(), &'static str> {
        if !request
            .look
            .into_iter()
            .chain(request.movement)
            .chain([request.zoom])
            .all(|n| n.is_finite() && n.abs() <= 64.0)
        {
            return Err("invalid camera movement");
        }
        if request
            .player
            .as_ref()
            .is_some_and(|n| n.is_empty() || n.len() > 96)
        {
            return Err("invalid camera player");
        }
        if request.mode == CameraMode::Orbit
            && self.mode != CameraMode::Orbit
            && let Some(pose) = self.pose
        {
            self.orbit_yaw = pose.yaw;
            self.orbit_pitch = pose.pitch;
        }
        self.mode = request.mode;
        self.player = request.player;
        match self.mode {
            CameraMode::Free => {
                if let Some(pose) = &mut self.pose {
                    pose.rotate(request.look);
                    let (forward, _) = pose.axes();
                    let (sy, cy) = pose.yaw.sin_cos();
                    let right = [sy, -cy, 0.0];
                    for i in 0..3 {
                        pose.position[i] +=
                            right[i] * request.movement[0] + forward[i] * request.movement[1];
                    }
                    pose.position[2] += request.movement[2];
                }
            }
            CameraMode::Orbit => {
                self.orbit_yaw =
                    (self.orbit_yaw + request.look[0]).rem_euclid(std::f32::consts::TAU);
                self.orbit_pitch =
                    (self.orbit_pitch + request.look[1]).clamp(-PITCH_LIMIT, PITCH_LIMIT);
                self.radius = (self.radius + request.zoom).clamp(1.25, 24.0);
            }
            CameraMode::Default | CameraMode::Follow | CameraMode::FirstPerson => {}
        }
        Ok(())
    }

    fn pose(&mut self, guest: &runtime::Guest, retail: Option<Pose>) -> Option<Pose> {
        if self.mode == CameraMode::Free && self.pose.is_some() {
            return self.pose;
        }
        let subject = subject(guest, self.player.as_deref());
        let next = subject.and_then(|unit| {
            let mut position = unit.pos.map(|v| v as f32);
            let crouch = unit.motion.crouch.unwrap_or_default() as f32;
            position[2] += if unit.vehicle.is_some() {
                0.85
            } else {
                0.64 - 0.22 * crouch
            };
            let body = unit.forward.map(|v| v as f32);
            let aim = unit.aim.unwrap_or(unit.forward).map(|v| v as f32);
            let body = Pose::looking(position, body)?;
            if self.mode == CameraMode::FirstPerson {
                return Pose::looking(position, aim);
            }
            let mut pose = if self.mode == CameraMode::Orbit {
                Pose {
                    position,
                    yaw: self.orbit_yaw,
                    pitch: self.orbit_pitch,
                }
            } else {
                Pose {
                    pitch: -0.24,
                    ..body
                }
            };
            let (forward, _) = pose.axes();
            let radius = if self.mode == CameraMode::Orbit {
                self.radius
            } else {
                DEFAULT_RADIUS
            };
            for i in 0..3 {
                pose.position[i] -= forward[i] * radius;
            }
            Some(pose)
        });
        // Keep the last camera when its player dies or disconnects.
        self.pose = next.or(self.pose).or(retail);
        self.pose
    }
}

fn subject(guest: &runtime::Guest, name: Option<&str>) -> Option<live_telemetry::ObjectState> {
    let players = live_telemetry::players(guest).ok()?;
    let mut roster = players.iter();
    let player = if let Some(name) = name {
        roster.find(|player| player.player.name == name)?
    } else {
        // On the first world frames a remote may be listed before it spawns.
        // Let the camera start at the first available unit.
        roster.find(|player| player.unit != u32::MAX)?
    };
    let unit = live_telemetry::object(guest, player.unit).ok()??;
    (!unit.vitality.is_some_and(|(body, _)| body <= 0.0)).then_some(unit)
}

fn put_view(record: &mut [u8; VIEW_STRIDE as usize], pose: Pose, clip: [f32; 2]) {
    let (forward, up) = pose.axes();
    let put = |bytes: &mut [u8], at: usize, values: &[f32]| {
        for (slot, value) in bytes[at..].chunks_exact_mut(4).zip(values) {
            slot.copy_from_slice(&value.to_le_bytes());
        }
    };
    for at in [0x0c, 0x80] {
        let camera = &mut record[at..at + RENDER_CAMERA_BYTES];
        put(camera, 0, &pose.position);
        put(camera, 0x0c, &forward);
        put(camera, 0x18, &up);
        put(camera, 0x40, &clip);
    }
}

fn blank_view() -> [u8; VIEW_STRIDE as usize] {
    // View kind and slot are zero. The player index must be -1: the
    // retail view reader otherwise indexes the local player's object.
    let mut record = [0u8; VIEW_STRIDE as usize];
    record[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    let screen = [0i16, 0, 480, 640];
    let fov = 2.0 * ((35.0f32.to_radians()).tan() / (640.0 / 480.0)).atan();
    for at in [0x0c, 0x80] {
        record[at + 0x28..at + 0x2c].copy_from_slice(&fov.to_le_bytes());
        record[at + 0x2c..at + 0x30].copy_from_slice(&1.0f32.to_le_bytes());
        for viewport in [0x30, 0x38] {
            for (i, edge) in screen.into_iter().enumerate() {
                let offset = at + viewport + i * 2;
                record[offset..offset + 2].copy_from_slice(&edge.to_le_bytes());
            }
        }
    }
    record
}

fn clip_distances(guest: &dyn GuestMemory) -> [f32; 2] {
    let near = guest.f32(NEAR_CLIP).unwrap_or_default();
    let far = guest.f32(FAR_CLIP).unwrap_or_default();
    if near.is_finite() && far.is_finite() && near > 0.0 && far > near {
        [near, far]
    } else {
        [0.06, 1024.0]
    }
}

pub(super) fn install(
    b: &mut Builder,
    replay: &std::sync::Arc<Session>,
) -> Result<(), feature::BuildError> {
    if !matches!(&replay.lock().mode, Mode::Replay(_, opts) if opts.render_every != 0) {
        return Ok(());
    }

    let scratch = b.guest_alloc(VIEW_STRIDE, runtime::guest::Permissions::RW)?;
    let session = replay.clone();
    b.raw()
        .before(&points::RENDER_VIEWS, "draw Theater camera", move |call| {
            let mut guest = call.ctx.memory();
            let esp = call.cpu.esp;
            if GuestMemory::u32(&*guest, esp).ok() != Some(WORLD_VIEW_RETURN) {
                return Ok(Flow::Continue);
            }
            let mut state = session.lock();
            if state.presentation.camera.mode == CameraMode::Default {
                return Ok(Flow::Continue);
            }
            let arg = esp.wrapping_add(12);
            let original = GuestMemory::u32(&*guest, arg)
                .ok()
                .filter(|&addr| addr != 0);
            let template = original.filter(|&addr| {
                call.cpu.eax > 0
                    && GuestMemory::u32(&*guest, addr).ok() == Some(0)
                    && GuestMemory::u32(&*guest, addr.wrapping_add(4)).ok() == Some(0)
            });
            let retail = template.and_then(|addr| {
                Pose::looking(
                    guest.vec3(addr.wrapping_add(0x0c)).ok()?,
                    guest.vec3(addr.wrapping_add(0x18)).ok()?,
                )
            });
            if let Some(pose) = state.presentation.camera.pose(&guest, retail) {
                let mut record = template
                    .and_then(|addr| guest.read::<{ VIEW_STRIDE as usize }>(addr).ok())
                    .unwrap_or_else(blank_view);
                // A private view is never associated with a local user.
                record[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
                put_view(&mut record, pose, clip_distances(&*guest));
                if GuestMemory::write(&mut *guest, scratch, &record).is_ok()
                    && GuestMemory::set_u32(&mut *guest, arg, scratch).is_ok()
                {
                    call.cpu.eax = 1;
                }
            }
            Ok(Flow::Continue)
        });

    // The original light host skipped world frames. Extra render draws here
    // use private RNG seeds, and never advance either of the sim's seeds.
    // Menus and loading still share the original stream, as in recording.
    let seeds = std::sync::Arc::new(std::sync::Mutex::new(Seeds::default()));
    let frame_seeds = seeds.clone();
    b.raw()
        .before(
            &points::RENDER_FRAME,
            "enter Theater render RNG",
            move |call| {
                let mut guest = call.ctx.memory();
                if feature::sim_frame::frame_kind(&feature::sim_frame::Addrs::HALO2, &*guest)
                    == feature::sim_frame::FrameKind::World
                {
                    frame_seeds
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .enter(&mut *guest);
                }
                Ok(Flow::Continue)
            },
        )
        .after_feature(crate::bot_glue::ID);
    b.raw()
        .after(
            &points::RENDER_FRAME,
            "restore Theater sim RNG",
            move |call| {
                seeds
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .leave(&mut *call.ctx.memory());
                Ok(())
            },
        )
        .after_feature(crate::bot_glue::ID);
    Ok(())
}

#[derive(Default)]
struct Seeds {
    restore: Option<(u32, [u8; 8])>,
    private: Option<[u8; 8]>,
}

impl Seeds {
    fn enter(&mut self, guest: &mut dyn GuestMemory) {
        let Ok(addr) = guest.u32(RANDOM_STATE) else {
            return;
        };
        if addr == 0 {
            return;
        }
        let mut original = [0; 8];
        if guest.read(addr, &mut original).is_ok()
            && guest
                .write(addr, self.private.as_ref().unwrap_or(&original))
                .is_ok()
        {
            self.restore = Some((addr, original));
        }
    }

    fn leave(&mut self, guest: &mut dyn GuestMemory) {
        let Some((addr, original)) = self.restore.take() else {
            return;
        };
        let mut advanced = [0; 8];
        if guest.read(addr, &mut advanced).is_ok() {
            self.private = Some(advanced);
        }
        let _ = guest.write(addr, &original);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_motion_and_orbit_keep_a_valid_view_basis() {
        let mut rig = ReplayCamera {
            pose: Pose::looking([0.0; 3], [1.0, 0.0, 0.0]),
            ..ReplayCamera::default()
        };
        rig.control(Camera {
            mode: CameraMode::Free,
            player: None,
            look: [0.0, 0.0],
            movement: [1.0, 2.0, 3.0],
            zoom: 0.0,
        })
        .unwrap();
        let pose = rig.pose.unwrap();
        assert_eq!(pose.position, [2.0, -1.0, 3.0]);
        let mut record = blank_view();
        put_view(&mut record, pose, [0.1, 1000.0]);
        assert_eq!(&record[8..12], &u32::MAX.to_le_bytes());
        assert_eq!(&record[0x0c..0x80], &record[0x80..0xf4]);
        assert_eq!(&record[0x18..0x1c], &1.0f32.to_le_bytes());

        rig.control(Camera {
            mode: CameraMode::Orbit,
            player: Some("Player".into()),
            look: [0.0, 60.0],
            movement: [0.0; 3],
            zoom: -60.0,
        })
        .unwrap();
        assert_eq!(rig.orbit_pitch, PITCH_LIMIT);
        assert_eq!(rig.radius, 1.25);
    }
}
