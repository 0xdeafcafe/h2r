//! The camera and lens live in host presentation state across replay restarts.
use agent_proto::theater::{Camera, CameraFallback, CameraMode, CameraPose};
use serde::{Deserialize, Serialize};

use crate::spectator_view::{PITCH_LIMIT, Pose};

pub(super) const DEFAULT_FOV: f32 = 70.0;
const DEFAULT_RADIUS: f32 = 4.0;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct ReplayCamera {
    pub(super) mode: CameraMode,
    pub(super) seat: Option<u8>,
    pub(super) player: Option<String>,
    pub(super) pose: Option<Pose>,
    pub(super) fov: Option<f32>,
    lens: f32,
    orbit_yaw: f32,
    orbit_pitch: f32,
    radius: f32,
    pub(super) initialized: bool,
}

impl Default for ReplayCamera {
    fn default() -> Self {
        Self {
            mode: CameraMode::Default,
            seat: None,
            player: None,
            pose: None,
            fov: None,
            lens: DEFAULT_FOV,
            orbit_yaw: 0.0,
            orbit_pitch: -0.28,
            radius: DEFAULT_RADIUS,
            initialized: false,
        }
    }
}

impl ReplayCamera {
    pub(super) fn control(
        &mut self,
        request: Camera,
        seat: Option<u8>,
    ) -> Result<(), &'static str> {
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
        if request
            .fov
            .is_some_and(|fov| !(20.0..=140.0).contains(&fov))
        {
            return Err("camera fov must be 20 to 140 degrees");
        }
        if request.mode == CameraMode::Orbit
            && self.mode != CameraMode::Orbit
            && let Some(pose) = self.pose
        {
            self.orbit_yaw = pose.yaw;
            self.orbit_pitch = pose.pitch;
        }
        self.mode = request.mode;
        self.seat = seat;
        self.player = request.player;
        self.fov = request.fov.or(self.fov);
        match self.mode {
            CameraMode::Free => {
                if let Some(pose) = &mut self.pose {
                    pose.rotate(request.look);
                    let (forward, _) = pose.axes();
                    let (sy, cy) = pose.yaw.sin_cos();
                    let right = [sy, -cy, 0.0];
                    for (i, coordinate) in pose.position.iter_mut().enumerate() {
                        *coordinate +=
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

    pub(super) fn effective(&self, playerless: bool, has_viewport: bool) -> CameraMode {
        if self.fallback(playerless, has_viewport).is_some() {
            CameraMode::Follow
        } else {
            self.mode
        }
    }

    pub(super) fn fallback(&self, playerless: bool, has_viewport: bool) -> Option<CameraFallback> {
        (self.mode == CameraMode::FirstPerson && !playerless && !has_viewport)
            .then_some(CameraFallback::NoRecordedViewport)
    }

    pub(super) fn use_view(&mut self, view: (Pose, f32)) {
        (self.pose, self.lens) = (Some(view.0), view.1);
    }

    pub(super) fn resolve(
        &mut self,
        mode: CameraMode,
        body: Option<Pose>,
        retail: Option<(Pose, f32)>,
    ) -> Option<Pose> {
        self.pose = match mode {
            CameraMode::Default | CameraMode::FirstPerson => retail.map(|view| view.0),
            CameraMode::Free => self.pose.or_else(|| {
                retail
                    .map(|view| view.0)
                    .or_else(|| body.map(|pose| self.offset(CameraMode::Follow, pose)))
            }),
            CameraMode::Follow | CameraMode::Orbit => body
                .map(|pose| self.offset(mode, pose))
                .or(self.pose)
                .or_else(|| retail.map(|view| view.0)),
        };
        if let Some((_, lens)) = retail {
            self.lens = lens;
        }
        self.pose
    }

    fn offset(&self, mode: CameraMode, body: Pose) -> Pose {
        let mut pose = if mode == CameraMode::Orbit {
            Pose {
                position: body.position,
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
        let radius = if mode == CameraMode::Orbit {
            self.radius
        } else {
            DEFAULT_RADIUS
        };
        for (coord, axis) in pose.position.iter_mut().zip(forward) {
            *coord -= axis * radius;
        }
        pose
    }

    pub(super) fn reported_pose(&self) -> Option<CameraPose> {
        let pose = self.pose?;
        Some(CameraPose {
            position: pose.position,
            yaw: pose.yaw,
            pitch: pose.pitch,
            fov: self.fov.unwrap_or(self.lens),
            radius: self.radius,
        })
    }
}
