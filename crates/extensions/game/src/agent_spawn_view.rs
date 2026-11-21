//! A private spectator camera for photographing the currently loaded scenario.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use agent_proto::render::CaptureCamera;
use agent_proto::{RpcError, SpawnViewParams, code, method};
use engine::{GuestMemory, points, render_loop::views::VIEW_STRIDE};
use hooks::{Builder, Flow};
use serde_json::{Value, json};

use crate::coop_players::layout::SCENARIO_STARTS;
use crate::spectator_view::{Pose, draw_world_view};

struct View {
    index: u32,
    count: u32,
    scenario: u32,
    pose: Pose,
    drawn: AtomicBool,
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

fn spawn(m: &dyn GuestMemory, index: u32, yaw_offset: f32) -> Result<View, RpcError> {
    // The same block the scenario editor calls StartingLocations.
    let scenario = m
        .u32(points::SCENARIO.base)
        .ok()
        .filter(|v| *v != 0)
        .ok_or_else(unreadable)?;
    let block = scenario
        .checked_add(SCENARIO_STARTS)
        .ok_or_else(unreadable)?;
    let count = m.u32(block).map_err(|_| unreadable())?;
    if count == 0 {
        return Err(unreadable());
    }
    if index >= count {
        return Err(invalid(format!("spawn index {index} outside 0..{count}")));
    }
    let data = m
        .u32(block.checked_add(4).ok_or_else(unreadable)?)
        .map_err(|_| unreadable())?;
    let at = index
        .checked_mul(0x34)
        .and_then(|off| data.checked_add(off))
        .ok_or_else(unreadable)?;
    if data == 0 || at.checked_add(15).is_none() {
        return Err(unreadable());
    }
    let mut position = m.vec3(at).map_err(|_| unreadable())?;
    position[2] += 0.64; // Standing eye height, as in Theater's player camera.
    let yaw = m.f32(at + 12).map_err(|_| unreadable())? + yaw_offset;
    if !position.into_iter().chain([yaw]).all(f32::is_finite) {
        return Err(unreadable());
    }
    Ok(View {
        index,
        count,
        scenario,
        pose: Pose {
            position,
            yaw: yaw.rem_euclid(std::f32::consts::TAU),
            pitch: 0.0,
        },
        drawn: AtomicBool::new(false),
    })
}

#[derive(Default)]
pub(crate) struct SpawnViews {
    active: AtomicBool,
    selected: Mutex<Option<Arc<View>>>,
}

impl SpawnViews {
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
                            let drawn = draw_world_view(call, scratch, |m, _| {
                                (GuestMemory::u32(m, points::SCENARIO.base).ok()
                                    == Some(view.scenario))
                                .then_some(view.pose)
                            });
                            if drawn {
                                // Completion stamp only. The render thread reads the selected
                                // pose and changes only its private view and call context.
                                view.drawn.store(true, Ordering::Release);
                                agent::render::CAPTURE.note_drawn_camera(|| {
                                    let map = crate::health::loaded_map_name(&*call.ctx.memory()).ok()?;
                                    let (forward, up) = view.pose.axes();
                                    Some((map, CaptureCamera {
                                        position: view.pose.position,
                                        forward,
                                        up,
                                    }))
                                });
                            }
                        }
                    }
                    Ok(Flow::Continue)
                },
            )
            .after_feature("theater-film")
            .context_only();
        let views = self.clone();
        b.raw().observe_before(
            &points::GAME_ACTIVATE,
            "release agent spawn camera",
            move |_| {
                views.select(None);
                agent::render::CAPTURE.clear_drawn_camera();
            },
        );
        Ok(())
    }

    pub(crate) fn install_method(self: &Arc<Self>, agent: &agent::Agent) {
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
                let view = Arc::new(ctx.read_guest(|m| spawn(m, index, offset))??);
                views.select(Some(view.clone()));
                let mut pending = Pending {
                    views: views.clone(),
                    view,
                    accepted: false,
                };
                ctx.defer(agent::DEFER_TICKS_MAX, Box::new(move |_| pending.reply()));
                Ok(Value::Null)
            },
        );
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
                "spawn camera replaced or map changed",
            )));
        }
        if !self.view.drawn.load(Ordering::Acquire) {
            return None;
        }
        self.accepted = true;
        Some(Ok(json!({
            "index": self.view.index,
            "count": self.view.count,
            "position": self.view.pose.position,
            "yaw": self.view.pose.yaw,
        })))
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
    use engine::testing::FakeGuest;

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
        assert_eq!((v.index, v.count), (1, 2));
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
        let select = |index| {
            let view = Arc::new(spawn(&scenario(), index, 0.0).unwrap());
            views.select(Some(view.clone()));
            Pending {
                views: views.clone(),
                view,
                accepted: false,
            }
        };
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
}
