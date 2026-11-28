//! A private spectator camera for photographing the currently loaded
//! scenario: `render.spawn_view` at a player starting location, and
//! `render.view` at an explicit eye (docs/runtime/agent-api.md).
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use agent_proto::{
    CaptureCamera, CurrentView, CurrentViewParams, RpcError, SpawnViewParams, ViewParams, ViewRequest, code, method,
};
use guest_layout::{GuestMemory, points};
use engine::render_loop::views::VIEW_STRIDE;
use hooks::{Builder, Flow};
use serde_json::{Value, json};

use crate::coop_players::layout::SCENARIO_STARTS;
use crate::spectator_view::{Pose, draw_world_view, drawn_view};

/// Where a selected camera came from, for its reply.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Origin {
    /// `render.spawn_view`: a player starting location.
    Start { index: u32, count: u32 },
    /// `render.view`: an explicit eye.
    Eye,
}

struct View {
    origin: Origin,
    scenario: u32,
    pose: Pose,
    /// Horizontal degrees; `None` keeps the drawn view's own lens.
    fov: Option<f32>,
    drawn: AtomicBool,
}

impl View {
    /// The pose to draw while the scenario it was chosen in is loaded.
    fn resolve(&self, m: &dyn GuestMemory) -> Option<(Pose, Option<f32>)> {
        (GuestMemory::u32(m, points::SCENARIO.base).ok() == Some(self.scenario))
            .then_some((self.pose, self.fov))
    }

    fn reply(&self) -> Value {
        match self.origin {
            Origin::Start { index, count } => json!({
                "index": index,
                "count": count,
                "position": self.pose.position,
                "yaw": self.pose.yaw,
            }),
            Origin::Eye => json!({
                "eye": self.pose.position,
                "yaw": self.pose.yaw,
                "pitch": self.pose.pitch,
                "fov": self.fov,
            }),
        }
    }
}

fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, message)
}

fn unreadable() -> RpcError {
    RpcError::new(
        code::UNAVAILABLE,
        "no readable player starting location in the current scenario",
    )
}

fn loaded_scenario(m: &dyn GuestMemory) -> Option<u32> {
    m.u32(points::SCENARIO.base).ok().filter(|v| *v != 0)
}

/// An explicit eye in the loaded scenario.
fn eye(m: &dyn GuestMemory, request: ViewRequest) -> Result<View, RpcError> {
    let ViewRequest::Pose {
        eye,
        yaw,
        pitch,
        fov,
    } = request
    else {
        return Err(invalid("reset has no pose"));
    };
    let scenario =
        loaded_scenario(m).ok_or_else(|| RpcError::new(code::UNAVAILABLE, "no scenario loaded"))?;
    Ok(View {
        origin: Origin::Eye,
        scenario,
        pose: Pose {
            position: eye,
            yaw,
            pitch,
        },
        fov,
        drawn: AtomicBool::new(false),
    })
}

/// The view being drawn for `seat`: the private camera while it is selected
/// and its scenario is loaded, else the game's own view record. A private
/// view without a lens of its own keeps the lens the game draws.
fn current(
    m: &dyn GuestMemory,
    seat: u8,
    private: Option<&View>,
) -> Result<CurrentView, RpcError> {
    let drawn = drawn_view(m, seat);
    if let Some((pose, fov)) = private.and_then(|view| view.resolve(m)) {
        let fov = fov.or(drawn.map(|view| view.fov)).ok_or_else(|| {
            RpcError::new(code::UNAVAILABLE, "no drawn view to take the lens from")
        })?;
        let view = CurrentView::looking(pose.position, pose.axes().0, fov)
            .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "the private view is not finite"))?;
        // Keep the heading and pitch the private view was given.
        return Ok(CurrentView {
            yaw: pose.yaw,
            pitch: pose.pitch,
            private: true,
            ..view
        });
    }
    drawn.ok_or_else(|| {
        RpcError::new(
            code::UNAVAILABLE,
            format!("no view is drawn for seat {seat} (no world, or a cinematic)"),
        )
    })
}

/// The loaded scenario's player starting locations: the same block the
/// scenario editor calls StartingLocations.
pub(crate) struct Starts {
    scenario: u32,
    count: u32,
    data: u32,
}

impl Starts {
    /// The loaded scenario's starts, if it has a readable, nonempty block.
    pub(crate) fn read(m: &dyn GuestMemory) -> Option<Self> {
        let scenario = loaded_scenario(m)?;
        let block = scenario.checked_add(SCENARIO_STARTS)?;
        let count = m.u32(block).ok().filter(|&n| n > 0)?;
        let data = m.u32(block.checked_add(4)?).ok()?;
        Some(Self { scenario, count, data })
    }

    /// The standing eye at start `index`: 0.64 above it, level, along its
    /// facing. `None` for an index outside the block or an unreadable or
    /// non-finite start.
    pub(crate) fn eye(&self, m: &dyn GuestMemory, index: u32) -> Option<Pose> {
        if index >= self.count || self.data == 0 {
            return None;
        }
        let at = index.checked_mul(0x34)?.checked_add(self.data)?;
        at.checked_add(15)?;
        let mut position = m.vec3(at).ok()?;
        position[2] += 0.64; // Standing eye height, as in Theater's player camera.
        let yaw = m.f32(at + 12).ok()?;
        position.into_iter().chain([yaw]).all(f32::is_finite).then(|| Pose {
            position,
            yaw: yaw.rem_euclid(std::f32::consts::TAU),
            pitch: 0.0,
        })
    }
}

fn spawn(m: &dyn GuestMemory, index: u32, yaw_offset: f32) -> Result<View, RpcError> {
    let starts = Starts::read(m).ok_or_else(unreadable)?;
    if index >= starts.count {
        return Err(invalid(format!("spawn index {index} outside 0..{}", starts.count)));
    }
    let mut pose = starts.eye(m, index).ok_or_else(unreadable)?;
    pose.yaw += yaw_offset;
    if !pose.yaw.is_finite() {
        return Err(unreadable());
    }
    pose.yaw = pose.yaw.rem_euclid(std::f32::consts::TAU);
    Ok(View {
        origin: Origin::Start { index, count: starts.count },
        scenario: starts.scenario,
        pose,
        fov: None,
        drawn: AtomicBool::new(false),
    })
}

#[derive(Default)]
pub(crate) struct SpawnViews {
    active: AtomicBool,
    selected: Mutex<Option<Arc<View>>>,
}

impl SpawnViews {
    /// A reset, a map change or a new game releases the camera; a pending
    /// request answers `INTERRUPTED`.
    fn release(&self) {
        self.select(None);
    }

    fn select(&self, view: Option<Arc<View>>) {
        let mut selected = self.selected.lock().unwrap_or_else(|e| e.into_inner());
        self.active.store(view.is_some(), Ordering::Release);
        *selected = view;
    }

    pub(crate) fn install_hooks(
        self: &Arc<Self>,
        b: &mut Builder,
    ) -> Result<(), hooks::BuildError> {
        let scratch = b.guest_alloc(VIEW_STRIDE, runtime::guest::Permissions::RW)?;
        let views = self.clone();
        b.raw()
            .before(
                &points::RENDER_VIEWS,
                "draw agent spawn camera",
                move |call| {
                    if views.active.load(Ordering::Acquire) {
                        let selected = views
                            .selected
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .clone();
                        if let Some(view) = selected {
                            let drawn = draw_world_view(call, scratch, |m, _| view.resolve(m));
                            if drawn {
                                // Completion stamp only. The render thread reads the selected
                                // pose and changes only its private view and call context.
                                view.drawn.store(true, Ordering::Release);
                                agent::render::CAPTURE.note_drawn_camera(|| {
                                    let map =
                                        crate::health::loaded_map_name(&*call.ctx.memory()).ok()?;
                                    let (forward, up) = view.pose.axes();
                                    Some((
                                        map,
                                        CaptureCamera {
                                            position: view.pose.position,
                                            forward,
                                            up,
                                        },
                                    ))
                                });
                            }
                        }
                    }
                    Ok(Flow::Continue)
                },
            )
            .after_feature("theater-camera")
            .context_only();
        let views = self.clone();
        b.raw().observe_before(
            &points::GAME_ACTIVATE,
            "release agent spawn camera",
            move |_| {
                views.release();
                agent::render::CAPTURE.clear_drawn_camera();
            },
        );
        Ok(())
    }

    pub(crate) fn install_method(self: &Arc<Self>, agent: &agent::Agent) {
        self.install_current(agent);
        let views = self.clone();
        agent.add_game_method(
            method::RENDER_SPAWN_VIEW,
            agent::Access::Act,
            move |params, ctx| {
                let request: SpawnViewParams =
                    serde_json::from_value(params).map_err(|err| invalid(err.to_string()))?;
                if request.reset && request.index.is_none() && request.yaw_offset.is_none() {
                    views.select(None);
                    return Ok(json!({ "reset": true }));
                }
                let index = request.index.filter(|_| !request.reset).ok_or_else(|| {
                    invalid("supply index and optional yaw_offset, or reset:true")
                })?;
                let offset = request.yaw_offset.unwrap_or_default();
                if !offset.is_finite() {
                    return Err(invalid("yaw_offset must be finite"));
                }
                let view = ctx.read_guest(|m| spawn(m, index, offset))??;
                views.answer_when_drawn(view, ctx);
                Ok(Value::Null)
            },
        );
        let views = self.clone();
        agent.add_game_method(
            method::RENDER_VIEW,
            agent::Access::Act,
            move |params, ctx| {
                let request = serde_json::from_value::<ViewParams>(params)
                    .map_err(|err| invalid(err.to_string()))?
                    .check()
                    .map_err(invalid)?;
                if request == ViewRequest::Reset {
                    views.release();
                    return Ok(json!({ "reset": true }));
                }
                let view = ctx.read_guest(|m| eye(m, request))??;
                views.answer_when_drawn(view, ctx);
                Ok(Value::Null)
            },
        );
    }

    /// `render.view.current`: read-only, at the tick boundary.
    pub(crate) fn install_current(self: &Arc<Self>, agent: &agent::Agent) {
        let views = self.clone();
        agent.add_game_method(
            method::RENDER_VIEW_CURRENT,
            agent::Access::Read,
            move |params, ctx| {
                let request = if params.is_null() {
                    CurrentViewParams::default()
                } else {
                    serde_json::from_value::<CurrentViewParams>(params)
                        .map_err(|err| invalid(err.to_string()))?
                };
                let seat = request.check().map_err(invalid)?;
                let private = views
                    .selected
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                let view = ctx.read_guest(|m| current(m, seat, private.as_deref()))??;
                serde_json::to_value(view).map_err(|err| RpcError::new(code::UNAVAILABLE, err.to_string()))
            },
        );
    }

    /// Selects `view` and answers once the world renderer has drawn it.
    fn answer_when_drawn(self: &Arc<Self>, view: View, ctx: &mut agent::GameCtx<'_>) {
        let mut pending = self.pending(view);
        ctx.defer(agent::DEFER_TICKS_MAX, Box::new(move |_| pending.reply()));
    }

    fn pending(self: &Arc<Self>, view: View) -> Pending {
        let view = Arc::new(view);
        self.select(Some(view.clone()));
        Pending {
            views: self.clone(),
            view,
            accepted: false,
        }
    }
}

/// A timed-out request releases its camera. An interrupted request leaves
/// the replacement camera alone.
struct Pending {
    views: Arc<SpawnViews>,
    view: Arc<View>,
    accepted: bool,
}

impl Pending {
    fn reply(&mut self) -> Option<Result<Value, RpcError>> {
        let current = self
            .views
            .selected
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if !current.as_ref().is_some_and(|v| Arc::ptr_eq(v, &self.view)) {
            return Some(Err(RpcError::new(
                code::INTERRUPTED,
                "agent camera replaced, reset or map changed",
            )));
        }
        if !self.view.drawn.load(Ordering::Acquire) {
            return None;
        }
        self.accepted = true;
        Some(Ok(self.view.reply()))
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if self.accepted {
            return;
        }
        let mut current = self
            .views
            .selected
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if current.as_ref().is_some_and(|v| Arc::ptr_eq(v, &self.view)) {
            *current = None;
            self.views.active.store(false, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    fn scenario() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_u32(points::SCENARIO.base, 0x1000)
            .put_u32(0x1000 + SCENARIO_STARTS, 2)
            .put_u32(0x1004 + SCENARIO_STARTS, 0x2000)
            .put_f32(0x2034, 8.0)
            .put_f32(0x2038, -2.0)
            .put_f32(0x203c, 3.0)
            .put_f32(0x2040, 1.25);
        m
    }

    #[test]
    fn uses_the_selected_scenario_start_and_facing() {
        let mut m = scenario();
        let v = spawn(&m, 1, -0.25).unwrap();
        assert_eq!(v.origin, Origin::Start { index: 1, count: 2 });
        assert_eq!(v.pose.position, [8.0, -2.0, 3.0_f32 + 0.64]);
        assert_eq!(v.pose.yaw, 1.0);
        assert_eq!(spawn(&m, 2, 0.0).err().unwrap().code, code::INVALID_PARAMS);
        m.put_f32(0x2040, f32::NAN);
        assert_eq!(spawn(&m, 1, 0.0).err().unwrap().code, code::UNAVAILABLE);
        m.put_u32(0x1004 + SCENARIO_STARTS, u32::MAX);
        assert!(spawn(&m, 0, 0.0).is_err());
        assert!(spawn(&FakeGuest::new(), 0, 0.0).is_err());
    }

    #[test]
    fn only_acknowledges_a_drawn_view_and_keeps_the_replacement() {
        let views = Arc::new(SpawnViews::default());
        let select = |index| views.pending(spawn(&scenario(), index, 0.0).unwrap());
        let mut old = select(0);
        assert!(old.reply().is_none());
        let mut current = select(1);
        assert_eq!(old.reply().unwrap().unwrap_err().code, code::INTERRUPTED);
        drop(old);
        current.view.drawn.store(true, Ordering::Release);
        assert_eq!(current.reply().unwrap().unwrap()["index"], 1);
        drop(current);
        assert!(views.active.load(Ordering::Acquire));
        drop(select(0));
        assert!(!views.active.load(Ordering::Acquire));
    }

    fn pose_request(fov: Option<f32>) -> ViewRequest {
        ViewParams {
            eye: Some([4.0, -6.0, 2.5]),
            yaw: Some(std::f32::consts::FRAC_PI_2),
            pitch: Some(-0.3),
            fov,
            reset: false,
        }
        .check()
        .unwrap()
    }

    #[test]
    fn an_explicit_eye_needs_a_loaded_scenario() {
        assert_eq!(
            eye(&FakeGuest::new(), pose_request(None))
                .err()
                .unwrap()
                .code,
            code::UNAVAILABLE
        );
        assert_eq!(
            eye(&scenario(), ViewRequest::Reset).err().unwrap().code,
            code::INVALID_PARAMS
        );
        let v = eye(&scenario(), pose_request(Some(90.0))).unwrap();
        assert_eq!(
            (v.origin, v.scenario, v.fov),
            (Origin::Eye, 0x1000, Some(90.0))
        );
        assert_eq!(v.pose.position, [4.0, -6.0, 2.5]);
        assert_eq!(
            (v.pose.yaw, v.pose.pitch),
            (std::f32::consts::FRAC_PI_2, -0.3)
        );
        let reply = v.reply();
        assert_eq!(reply["eye"], json!([4.0, -6.0, 2.5]));
        assert_eq!(reply["fov"], json!(90.0));
        assert!(reply.get("index").is_none());
    }

    #[test]
    fn the_pose_reaches_the_private_view() {
        use crate::spectator_view::{
            WORLD_VIEW_RETURN, blank_view, replace_world_view, vertical_fov,
        };
        const STACK: u32 = 0x5000;
        const ORIGINAL: u32 = 0x6000;
        const SCRATCH: u32 = 0x7000;
        let mut original = blank_view();
        original[8..12].copy_from_slice(&0u32.to_le_bytes());
        let mut guest = scenario();
        guest
            .put_u32(STACK, WORLD_VIEW_RETURN)
            .put_u32(STACK + 12, ORIGINAL)
            .put(ORIGINAL, &original)
            .map(SCRATCH, VIEW_STRIDE);
        let view = eye(&guest, pose_request(Some(90.0))).unwrap();
        assert!(replace_world_view(&mut guest, STACK, 1, SCRATCH, |m, _| {
            view.resolve(m)
        }));
        assert_eq!(guest.u32(STACK + 12).unwrap(), SCRATCH);
        let (forward, up) = view.pose.axes();
        for camera in [0x0c, 0x80] {
            assert_eq!(guest.vec3(SCRATCH + camera).unwrap(), [4.0, -6.0, 2.5]);
            assert_eq!(guest.vec3(SCRATCH + camera + 0x0c).unwrap(), forward);
            assert_eq!(guest.vec3(SCRATCH + camera + 0x18).unwrap(), up);
            assert_eq!(
                guest.f32(SCRATCH + camera + 0x28).unwrap(),
                vertical_fov(90.0)
            );
        }
        // Facing +y and looking down.
        assert!(forward[0].abs() < 1e-6 && forward[1] > 0.9 && forward[2] < 0.0);

        // Another scenario loaded: the view is not drawn.
        guest
            .put_u32(STACK + 12, ORIGINAL)
            .put_u32(points::SCENARIO.base, 0x1100);
        assert!(!replace_world_view(
            &mut guest,
            STACK,
            1,
            SCRATCH,
            |m, _| view.resolve(m)
        ));
        assert_eq!(guest.u32(STACK + 12).unwrap(), ORIGINAL);
    }

    #[test]
    fn a_drawn_eye_answers_with_its_pose() {
        let views = Arc::new(SpawnViews::default());
        let mut pending = views.pending(eye(&scenario(), pose_request(None)).unwrap());
        assert!(views.active.load(Ordering::Acquire));
        assert!(pending.reply().is_none());
        pending.view.drawn.store(true, Ordering::Release);
        let reply = pending.reply().unwrap().unwrap();
        assert_eq!(reply["eye"], json!([4.0, -6.0, 2.5]));
        assert_eq!(reply["fov"], Value::Null);
        drop(pending);
        assert!(
            views.active.load(Ordering::Acquire),
            "an answered view stays"
        );
    }

    #[test]
    fn reset_releases_the_camera_and_interrupts_a_pending_view() {
        let views = Arc::new(SpawnViews::default());
        let mut pending = views.pending(eye(&scenario(), pose_request(None)).unwrap());
        views.release();
        assert!(!views.active.load(Ordering::Acquire));
        assert_eq!(
            pending.reply().unwrap().unwrap_err().code,
            code::INTERRUPTED
        );
    }

    #[test]
    fn a_map_change_interrupts_a_pending_view() {
        // GAME_ACTIVATE's observer calls `release`; the next pending poll
        // answers INTERRUPTED even if the old view was drawn first.
        let views = Arc::new(SpawnViews::default());
        let mut pending = views.pending(eye(&scenario(), pose_request(Some(70.0))).unwrap());
        pending.view.drawn.store(true, Ordering::Release);
        views.release();
        assert_eq!(
            pending.reply().unwrap().unwrap_err().code,
            code::INTERRUPTED
        );
        drop(pending);
        assert!(!views.active.load(Ordering::Acquire));
        assert!(views.selected.lock().unwrap().is_none());
    }

    /// The game draws seat 0 from a record at (1, 2, 3) facing +y with a
    /// 90 degree lens, in scenario `0x1000`.
    fn drawn() -> FakeGuest {
        const RECORD: u32 = 0x0047_f210;
        let mut m = scenario();
        let camera = RECORD + 0x0c;
        m.put_u32(RECORD, 0)
            .put_u32(RECORD + 8, 0)
            .put_f32(camera, 1.0)
            .put_f32(camera + 4, 2.0)
            .put_f32(camera + 8, 3.0)
            .put_f32(camera + 0x0c, 0.0)
            .put_f32(camera + 0x10, 1.0)
            .put_f32(camera + 0x14, 0.0)
            .put_f32(camera + 0x28, crate::spectator_view::vertical_fov(90.0));
        m
    }

    #[test]
    fn the_current_view_is_the_game_record_without_a_private_camera() {
        let m = drawn();
        let view = current(&m, 0, None).unwrap();
        assert_eq!(view.eye, [1.0, 2.0, 3.0]);
        assert!(!view.private);
        assert!((view.fov - 90.0).abs() < 1e-3);
        assert_eq!(current(&m, 1, None).unwrap_err().code, code::UNAVAILABLE);
        assert_eq!(
            current(&FakeGuest::new(), 0, None).unwrap_err().code,
            code::UNAVAILABLE
        );
        // It replays: the pose passes `render.view`'s check.
        assert!(matches!(
            view.view_params().check(),
            Ok(ViewRequest::Pose { fov: Some(_), .. })
        ));
    }

    #[test]
    fn the_current_view_reports_the_private_camera_while_it_is_loaded() {
        let m = drawn();
        let private = eye(&m, pose_request(Some(60.0))).unwrap();
        let view = current(&m, 0, Some(&private)).unwrap();
        assert!(view.private);
        assert_eq!(view.eye, [4.0, -6.0, 2.5]);
        assert_eq!((view.yaw, view.pitch), (std::f32::consts::FRAC_PI_2, -0.3));
        assert_eq!(view.fov, 60.0);
        // `render.view(render.view.current())` selects the same pose.
        let Ok(ViewRequest::Pose { eye, yaw, pitch, fov }) = view.view_params().check() else {
            panic!("not a pose");
        };
        assert_eq!((eye, yaw, pitch, fov), (view.eye, view.yaw, view.pitch, Some(60.0)));
        // Without a lens of its own it keeps the drawn view's.
        let keep = eye_view(&m, pose_request(None));
        let view = current(&m, 0, Some(&keep)).unwrap();
        assert!(view.private && (view.fov - 90.0).abs() < 1e-3);
        // Another scenario loaded: the private camera is not drawn.
        let mut other = drawn();
        other.put_u32(points::SCENARIO.base, 0x1100);
        let view = current(&other, 0, Some(&private)).unwrap();
        assert!(!view.private);
        assert_eq!(view.eye, [1.0, 2.0, 3.0]);
    }

    fn eye_view(m: &FakeGuest, request: ViewRequest) -> View {
        eye(m, request).unwrap()
    }
}
