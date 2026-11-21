//! Editor camera modes over the displayed game sample and scoped player moves.

use agent_proto::sim;
use scenario_edit::geometry::{Camera, Ray, Vec3, add, dot, scale, sub};
use serde_json::json;

use crate::App;
use crate::play::{Play, link::Purpose, select, surface_under_ray};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CameraMode {
    #[default]
    Free,
    Follow,
    Eyes,
}

impl CameraMode {
    pub(crate) const ALL: [Self; 3] = [Self::Free, Self::Follow, Self::Eyes];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Follow => "follow",
            Self::Eyes => "eyes",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Follow => "Follow player",
            Self::Eyes => "Player's eyes",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SharedCamera {
    pub(crate) mode: CameraMode,
    /// Map revision and stream epoch whose coordinate system the camera uses.
    scope: Option<(u64, u64)>,
    target: Option<u16>,
    /// The hovered ray in the document and camera pose used to draw the frame.
    pub(crate) cursor: Option<(Ray, u64)>,
}

fn look_along(cam: &mut Camera, dir: Vec3) {
    let horizontal = dir[0].hypot(dir[1]);
    if horizontal > 0.0 {
        cam.yaw = (-dir[1]).atan2(-dir[0]);
    }
    // The view matrix takes its right axis from forward × world-up.
    let vertical = std::f32::consts::FRAC_PI_2 - 1e-6;
    cam.pitch = (-dir[2]).atan2(horizontal).clamp(-vertical, vertical);
}

/// Enter orbit about the player without moving the eye. The orbit's
/// distance and angles carry its offset into subsequent samples.
fn enter_follow(cam: &mut Camera, player: Vec3) {
    let dir = sub(player, cam.eye());
    let len = dot(dir, dir).sqrt();
    if len > 1e-4 {
        look_along(cam, dir);
    }
    cam.distance = len.max(1e-4);
    follow(cam, player);
}

fn follow(cam: &mut Camera, player: Vec3) {
    cam.target = player;
}

fn eyes(cam: &mut Camera, ray: Ray) {
    look_along(cam, ray.dir);
    cam.distance = 1.0;
    cam.target = add(ray.origin, scale(cam.forward(), cam.distance));
}

impl Play {
    pub(crate) fn camera_target(app: &App) -> Option<u16> {
        let (player, _) = app.pairing.player()?;
        player.local_user?;
        Some(player.index)
    }

    pub(crate) fn set_camera_mode(app: &mut App, mode: CameraMode) -> Result<(), &'static str> {
        if mode == CameraMode::Free {
            app.pairing.camera.mode = mode;
            app.pairing.camera.target = None;
            app.pairing.camera.scope = None;
            return Ok(());
        }
        let sample = app
            .pairing
            .shown
            .as_ref()
            .ok_or("waiting for a game sample")?;
        let scope = (app.session.world_rev(), sample.epoch);
        let old = &app.pairing.camera;
        let enter = old.mode != mode || old.scope != Some(scope);
        let index = match mode {
            CameraMode::Follow => {
                let (index, pos, _) = select::local_pose(app)
                    .ok_or("Follow needs the local player on the open map")?;
                if enter || old.target != Some(index) {
                    enter_follow(app.session.camera_mut(), pos);
                } else {
                    follow(app.session.camera_mut(), pos);
                }
                index
            }
            CameraMode::Eyes => {
                let ray = select::player_ray(app)
                    .ok_or("Player's eyes needs the local player's view on the open map")?;
                let (player, _) = app.pairing.player().ok_or("no local player")?;
                let index = player.index;
                eyes(app.session.camera_mut(), ray);
                index
            }
            CameraMode::Free => unreachable!(),
        };
        app.pairing.camera.mode = mode;
        app.pairing.camera.scope = Some(scope);
        app.pairing.camera.target = Some(index);
        Ok(())
    }

    pub(in crate::play) fn update_shared_camera(app: &mut App) {
        app.pairing.camera.cursor = None;
        let mode = app.pairing.camera.mode;
        if mode == CameraMode::Free {
            return;
        }
        let scope = app
            .pairing
            .shown
            .as_ref()
            .map(|s| (app.session.world_rev(), s.epoch));
        if scope != app.pairing.camera.scope || Self::set_camera_mode(app, mode).is_err() {
            app.pairing.camera = SharedCamera::default();
        }
    }

    pub(crate) fn cursor_destination(app: &App) -> Option<Vec3> {
        let (ray, world) = app.pairing.camera.cursor?;
        if world != app.session.world_rev() {
            return None;
        }
        surface_under_ray(app, &ray)
    }

    pub(crate) fn player_move_refusal(&self, app: &App) -> Option<String> {
        if !self.is_paired() {
            return Some("not paired with a game".into());
        }
        self.require_method(sim::method::STATE_SET)
            .err()
            .or_else(|| self.edits_blocked(app))
            .or_else(|| {
                self.session
                    .and_then(|s| s.writes().err())
                    .map(str::to_owned)
            })
    }

    fn player_move_scope(&self, app: &App) -> Result<(u64, u64, u64), String> {
        if let Some(why) = self.player_move_refusal(app) {
            return Err(why);
        }
        let epoch = self.edits.epoch.ok_or("waiting for the loaded map")?;
        Ok((self.pairing_id, app.session.world_rev(), epoch))
    }

    /// Resolve the controller again just before dispatch. A queued command cannot
    /// move a respawned unit or run on a replacement map or socket.
    pub(crate) fn send_player_move(&self, app: &App, request: PlayerMove) -> Result<(), String> {
        if self.player_move_scope(app)? != request.scope
            || local_unit(app, request.scope) != Some((request.player, request.unit))
        {
            return Err("the game, map or local player changed before the move".into());
        }
        self.send_move(request);
        Ok(())
    }

    pub(crate) fn player_here(&self, app: &App, pos: Vec3, facing: bool) -> Result<(), String> {
        let request = PlayerMove::at(app, self.player_move_scope(app)?, pos, facing)?;
        self.send_move(request);
        Ok(())
    }

    fn send_move(&self, request: PlayerMove) {
        let mut params = json!({"player":format!("player.{}", request.player), "pos":request.pos});
        if let Some((yaw, pitch)) = request.facing {
            params["yaw"] = json!(yaw);
            params["pitch"] = json!(pitch);
        }
        self.call(sim::method::STATE_SET, params, Purpose::Teleport);
    }
}

/// The world point and optional camera facing as requested, on this pairing,
/// in this document and epoch, for this exact local unit.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayerMove {
    scope: (u64, u64, u64),
    player: u16,
    unit: u32,
    pos: Vec3,
    facing: Option<(f64, f64)>,
}

fn local_unit(app: &App, (_, world, epoch): (u64, u64, u64)) -> Option<(u16, u32)> {
    let sample = app.pairing.shown.as_ref()?;
    if app.session.world_rev() != world
        || sample.epoch != epoch
        || !app.pairing.matches_sample(app, sample)
    {
        return None;
    }
    let (player, unit) = app.pairing.player()?;
    player.local_user?;
    Some((player.index, unit.id))
}

impl PlayerMove {
    pub(crate) fn capture(app: &App, pos: Vec3, facing: bool) -> Result<Self, String> {
        let scope = app.pairing.player_scope.ok_or_else(|| {
            app.pairing
                .published
                .as_ref()
                .and_then(|status| {
                    if status["state"] == "connected" && !app.pairing.player_move_available {
                        Some("moving the player requires sim.state.set")
                    } else {
                        status["edits_blocked"].as_str()
                    }
                })
                .unwrap_or("waiting for the editor to pair in Edit mode")
                .to_owned()
        })?;
        Self::at(app, scope, pos, facing)
    }

    fn at(app: &App, scope: (u64, u64, u64), pos: Vec3, facing: bool) -> Result<Self, String> {
        let (player, unit) =
            local_unit(app, scope).ok_or("moving the player needs a local unit on the open map")?;
        if !pos.iter().all(|v| v.is_finite()) {
            return Err("player position must be finite".into());
        }
        let facing = if facing {
            let (yaw, pitch) = sim::yaw_pitch(app.session.camera().forward().map(f64::from))
                .ok_or("camera has no facing direction")?;
            Some((yaw, pitch.clamp(-89.0, 89.0)))
        } else {
            None
        };
        Ok(Self {
            scope,
            player,
            unit,
            pos,
            facing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) {
        for (actual, expected) in a.into_iter().zip(b) {
            assert!((actual - expected).abs() < 1e-4, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn follow_keeps_the_eye_offset_as_the_player_moves() {
        let mut cam = Camera {
            distance: 8.0,
            ..Camera::default()
        };
        let eye = cam.eye();
        let first = [5.0, 3.0, 2.0];
        enter_follow(&mut cam, first);
        close(cam.eye(), eye);
        close(cam.target, first);
        let next = [2.0, -3.0, 7.0];
        follow(&mut cam, next);
        close(sub(cam.eye(), next), sub(eye, first));
        close(cam.target, next);
    }

    #[test]
    fn eyes_start_at_the_eye_and_look_along_the_aim() {
        let mut cam = Camera::default();
        let origin = [10.0, 5.0, 2.5];
        for dir in [[0.0, 0.8, 0.6], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] {
            eyes(&mut cam, Ray { origin, dir });
            close(cam.eye(), origin);
            close(cam.forward(), dir);
        }
    }
}
