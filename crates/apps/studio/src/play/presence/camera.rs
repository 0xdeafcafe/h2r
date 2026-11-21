//! Editor camera modes over the displayed game sample and scoped player moves.

use agent_proto::{live::View, sim};
use scenario_edit::geometry::{Camera, Exact, Ray, Vec3, add, dot, normalize, scale, sub};
use serde_json::json;

use crate::App;
use crate::play::{Play, playtest::Waiting, select, surface_under_ray};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CameraMode {
    #[default]
    Free,
    Follow,
    Eyes,
    /// A game view's camera from the stream's `camera` layer, exactly.
    Game,
}

impl CameraMode {
    pub(crate) const ALL: [Self; 4] = [Self::Free, Self::Follow, Self::Eyes, Self::Game];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Follow => "follow",
            Self::Eyes => "eyes",
            Self::Game => "game",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Follow => "Follow player",
            Self::Eyes => "Player's eyes",
            Self::Game => "Game camera",
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SharedCamera {
    pub(crate) mode: CameraMode,
    /// Map revision and stream epoch whose coordinate system the camera uses.
    scope: Option<(u64, u64)>,
    target: Option<u16>,
    /// The game view Game camera takes ([`View::key`]).
    view: Option<u32>,
    /// The editor's field of view before Game camera took the game's.
    fov: Option<f32>,
    /// Establish the orbit offset on the first sample with this target's unit.
    entering: bool,
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

/// The game's view exactly. The orbit fields follow it, so leaving Game
/// camera keeps the eye where the game's was.
fn game_view(cam: &mut Camera, view: &View) {
    cam.exact = None;
    look_along(cam, view.forward);
    cam.distance = 1.0;
    cam.target = add(view.position, normalize(view.forward));
    cam.fov_y = view.fov;
    cam.exact = Some(Exact {
        eye: view.position,
        forward: view.forward,
        up: view.up,
        near: view.near,
        far: view.far,
    });
}

/// Why the paired stream carries no `camera` layer, when it does not.
pub(crate) fn camera_refusal(app: &App) -> Option<String> {
    let Some(stream) = app.pairing.mirror.as_ref() else {
        return Some("not paired with a game".into());
    };
    let status = stream.status(false);
    let Some(layers) = status["layers"].as_array() else {
        return Some("waiting for the game's stream".into());
    };
    if layers.iter().any(|l| l == "camera") {
        return None;
    }
    let why = status["refused"]["camera"]
        .as_str()
        .unwrap_or("the game's stream has no camera layer");
    Some(format!("the game has no camera: {why}"))
}

impl Play {
    pub(crate) fn camera_target(app: &App) -> Option<u16> {
        app.pairing.camera.target
    }

    pub(crate) fn set_camera_mode(app: &mut App, mode: CameraMode) -> Result<(), String> {
        Self::set_camera_target(app, mode, None)
    }

    /// The game view Game camera shows.
    pub(crate) fn camera_view(app: &App) -> Option<u32> {
        app.pairing.camera.view
    }

    /// Leave Game camera's exact pose and field of view, keeping the eye.
    fn release_game_view(app: &mut App) {
        let camera = &mut app.pairing.camera;
        camera.view = None;
        let fov = camera.fov.take();
        let cam = app.session.camera_mut();
        if cam.exact.take().is_some()
            && let Some(fov) = fov
        {
            cam.fov_y = fov;
        }
    }

    /// Take a game view's camera: `view`, the one already shown, or the
    /// first game view the stream carries.
    pub(crate) fn set_game_camera(app: &mut App, view: Option<u32>) -> Result<(), String> {
        let sample = app
            .pairing
            .shown
            .as_ref()
            .ok_or("waiting for a game sample")?;
        if !select::matching(app) {
            return Err("open the streamed map to take its camera".into());
        }
        if let Some(why) = camera_refusal(app) {
            return Err(why);
        }
        let scope = (app.session.world_rev(), sample.epoch);
        let camera = &app.pairing.camera;
        let current = camera
            .view
            .filter(|_| camera.mode == CameraMode::Game && camera.scope == Some(scope));
        let key = match view.or(current) {
            Some(key) if Some(key) == current => key,
            Some(key) => {
                if !sample.cameras.iter().any(|v| v.key == key) {
                    let name = View {
                        key,
                        ..View::default()
                    }
                    .name();
                    return Err(format!("the game drew no view {name}"));
                }
                key
            }
            None => {
                sample
                    .cameras
                    .iter()
                    .find(|v| v.is_game())
                    .or(sample.cameras.first())
                    .ok_or("the game has not drawn a view yet")?
                    .key
            }
        };
        if app.pairing.camera.mode != CameraMode::Game {
            app.pairing.camera.fov = Some(app.session.camera().fov_y);
        }
        let camera = &mut app.pairing.camera;
        camera.mode = CameraMode::Game;
        camera.target = None;
        camera.view = Some(key);
        camera.scope = Some(scope);
        camera.entering = false;
        Self::track_game_view(app, key);
        Ok(())
    }

    fn track_game_view(app: &mut App, key: u32) {
        let Some(view) = app
            .pairing
            .shown
            .as_ref()
            .and_then(|s| s.cameras.iter().find(|v| v.key == key))
            .cloned()
        else {
            // A view that is not drawn this frame holds the last pose.
            return;
        };
        game_view(app.session.camera_mut(), &view);
    }

    pub(crate) fn set_camera_target(
        app: &mut App,
        mode: CameraMode,
        target: Option<u16>,
    ) -> Result<(), String> {
        if mode == CameraMode::Game {
            if target.is_some() {
                return Err("Game camera takes a view, not a player target".into());
            }
            return Self::set_game_camera(app, None);
        }
        if mode == CameraMode::Free {
            if target.is_some() {
                return Err("Free camera has no player target".into());
            }
            Self::release_game_view(app);
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
        if !select::matching(app) {
            return Err("open the streamed map to follow its player".into());
        }
        let scope = (app.session.world_rev(), sample.epoch);
        let camera = &app.pairing.camera;
        let current = camera.target.filter(|_| camera.scope == Some(scope));
        // Keep the selected index across a switch from Follow to Eyes. A
        // split-screen player's temporary lack of a unit never selects a peer.
        let index = match target.or(current) {
            Some(index) if Some(index) == current => index,
            Some(index) => {
                if !sample
                    .players
                    .iter()
                    .any(|p| p.index == index && p.local_user.is_some())
                {
                    return Err("the shared camera needs a local player".into());
                }
                index
            }
            None => {
                app.pairing
                    .player()
                    .map(|(player, _)| player)
                    .filter(|player| player.local_user.is_some())
                    .or_else(|| {
                        sample
                            .players
                            .iter()
                            .filter(|p| p.local_user.is_some())
                            .min_by_key(|p| (p.local_user, p.index))
                    })
                    .ok_or("the shared camera needs a local player")?
                    .index
            }
        };
        let enter = camera.mode != mode || current != Some(index);
        Self::release_game_view(app);
        let camera = &mut app.pairing.camera;
        camera.mode = mode;
        camera.target = Some(index);
        camera.scope = Some(scope);
        camera.entering |= enter;
        Self::track_camera(app, index);
        Ok(())
    }

    fn track_camera(app: &mut App, index: u16) {
        let Some((pos, aim)) = select::pose_for(app, index) else {
            return;
        };
        match app.pairing.camera.mode {
            CameraMode::Follow if app.pairing.camera.entering => {
                enter_follow(app.session.camera_mut(), pos)
            }
            CameraMode::Follow => follow(app.session.camera_mut(), pos),
            CameraMode::Eyes => {
                let eye = add(pos, [0.0, 0.0, select::EYE_HEIGHT]);
                let Some(ray) = select::unit_ray(eye, aim) else {
                    return;
                };
                eyes(app.session.camera_mut(), ray);
            }
            CameraMode::Free | CameraMode::Game => return,
        }
        app.pairing.camera.entering = false;
    }

    pub(in crate::play) fn update_shared_camera(app: &mut App) {
        app.pairing.camera.cursor = None;
        let Some((world, epoch)) = app.pairing.camera.scope else {
            return;
        };
        if world != app.session.world_rev()
            || app
                .pairing
                .shown
                .as_ref()
                .is_some_and(|s| s.epoch != epoch || !select::matching(app))
            || app.pairing.stream_map["epoch"]
                .as_u64()
                .is_some_and(|e| e != epoch)
        {
            Self::release_game_view(app);
            app.pairing.camera = SharedCamera::default();
            return;
        }
        if let Some(key) = app.pairing.camera.view {
            Self::track_game_view(app, key);
        } else if let Some(index) = app.pairing.camera.target {
            Self::track_camera(app, index);
        }
    }

    pub(crate) fn cursor_destination(app: &App) -> Option<Vec3> {
        let (ray, world) = app.pairing.camera.cursor?;
        if world != app.session.world_rev() {
            return None;
        }
        surface_under_ray(app, &ray)
    }

    /// Shared by publication, immediate UI moves and queued socket dispatch.
    pub(in crate::play) fn player_move_scope<'a>(
        &self,
        app: &App,
        edits_blocked: Option<&'a str>,
    ) -> Result<(u64, u64, u64), &'a str> {
        if !self.is_paired() {
            return Err("not paired with a game");
        }
        if !self.has_method(sim::method::STATE_SET) {
            return Err("moving the player requires sim.state.set");
        }
        if self.reload_copy.is_some() || self.waiting != Waiting::Idle {
            return Err("wait for the playtest or map reload to finish before moving the player");
        }
        if let Some(why) = edits_blocked {
            return Err(why);
        }
        self.session.ok_or("waiting for the session")?.writes()?;
        let epoch = self.edits.epoch.ok_or("waiting for the loaded map")?;
        Ok((self.pairing_id, app.session.world_rev(), epoch))
    }

    pub(crate) fn send_player_move(&mut self, app: &App, request: PlayerMove) -> Result<(), String> {
        if self.player_move_scope(app, self.edits_blocked(app).as_deref())? != request.scope {
            return Err("the game or map changed before the move".into());
        }
        let shown = app
            .pairing
            .shown
            .as_ref()
            .ok_or("waiting for a game sample")?;
        request.check_unit(app, shown)?;
        // The UI can still be drawing an older tick when a respawn has
        // already arrived on the binary connection.
        let stream = app
            .pairing
            .mirror
            .as_ref()
            .ok_or("waiting for the game's latest exact sample")?;
        request.check_latest(stream)?;
        let scope = self.move_context(app);
        let ticket = self.feedback.sent_move(scope, &request);
        self.link
            .as_ref()
            .ok_or("the game link closed before the move")?
            .move_player(request, stream, ticket)
    }

    pub(crate) fn player_here(&mut self, app: &mut App, pos: Vec3, facing: bool) {
        let result = self
            .player_move_scope(app, self.edits_blocked(app).as_deref())
            .map_err(str::to_owned)
            .and_then(|scope| PlayerMove::at(app, scope, pos, facing))
            .and_then(|request| self.send_player_move(app, request));
        if let Err(why) = result {
            self.game_refusal(app, why);
        }
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

impl PlayerMove {
    pub(crate) fn capture(app: &App, pos: Vec3, facing: bool) -> Result<Self, String> {
        let scope = app.pairing.player_scope.clone()?;
        Self::at(app, scope, pos, facing)
    }

    fn check_unit(&self, app: &App, sample: &live_mirror::Sample) -> Result<(), &'static str> {
        if app.session.world_rev() != self.scope.1 || !app.pairing.matches_sample(app, sample) {
            return Err("the game or map changed before the move");
        }
        self.check_target(sample)
    }

    pub(in crate::play) fn check_latest(
        &self,
        stream: &crate::play::stream::Stream,
    ) -> Result<(), &'static str> {
        let exact = stream
            .sample(true)
            .ok_or("waiting for the game's latest exact sample")?;
        self.check_target(&exact)
    }

    fn check_target(&self, sample: &live_mirror::Sample) -> Result<(), &'static str> {
        if sample.stale {
            return Err("the player sample is stale");
        }
        if sample.epoch != self.scope.2 {
            return Err("the game or map changed before the move");
        }
        if !sample
            .players
            .iter()
            .any(|p| p.index == self.player && p.local_user.is_some() && p.unit == Some(self.unit))
            || !sample.objects.iter().any(|o| o.id == self.unit)
        {
            return Err("the local player's unit changed before the move");
        }
        Ok(())
    }

    /// sim.state.set identifies the player by index. This is the wire boundary
    /// for an expected salted unit and epoch when the game supports those guards.
    pub(in crate::play) fn state_set_params(&self) -> serde_json::Value {
        let mut params = json!({"player":format!("player.{}", self.player), "pos":self.pos});
        if let Some((yaw, pitch)) = self.facing {
            params["yaw"] = json!(yaw);
            params["pitch"] = json!(pitch);
        }
        params
    }

    fn at(app: &App, scope: (u64, u64, u64), pos: Vec3, facing: bool) -> Result<Self, String> {
        let sample = app
            .pairing
            .shown
            .as_ref()
            .ok_or("waiting for a game sample")?;
        let (player, unit) = app
            .pairing
            .player()
            .filter(|(p, _)| p.local_user.is_some())
            .ok_or("moving the player needs a local unit on the open map")?;
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
        let request = Self {
            scope,
            player: player.index,
            unit: unit.id,
            pos,
            facing,
        };
        request.check_unit(app, sample)?;
        Ok(request)
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

#[cfg(all(test, unix))]
mod move_and_target_tests;

#[cfg(all(test, unix))]
mod game_camera_tests;
