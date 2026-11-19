//! Visual prediction for synchronous Live co-op. Observe the channel's
//! outgoing 0x28 and the 0x27 record actually applied to the local player.
//! Only render_game_view receives the camera adjustment, in its own copy.
mod prediction;
mod view;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use agent_proto::{RpcError, code, method};
use engine::{GuestMemory, points};
use feature::{Builder, Feature, Flow, Manifest, Registry};
use prediction::{Aim, Prediction};
use serde::{Deserialize, Serialize};
use settings::{Setting, Store};

use crate::coop_probe::Arm;
use crate::setting_defs::COOP_LOOK_PREDICTION;
use view::Subject;

const USERS: usize = 4;
const HISTORY: usize = 256;
const ACTION_BYTES: u32 = 0x5c;

#[derive(Default)]
struct User {
    subject: Option<Subject>,
    prediction: Prediction,
}

impl User {
    /// Discontinuities discard sends from the old camera, including when
    /// a slot is rebound to a new salted player/unit between two frames.
    fn observe(&mut self, m: &dyn GuestMemory, user: usize) -> Result<Subject, &'static str> {
        let next = view::subject(m, user);
        if next.is_err() || self.subject.zip(next.ok()).is_some_and(|(a, b)| a.cut(b)) {
            *self = Self::default();
        }
        self.subject = next.ok();
        next
    }
}

#[derive(Default)]
struct State {
    users: [User; USERS],
    history: VecDeque<Frame>,
    serial: u64,
}

#[derive(Serialize)]
struct Frame {
    sample: u64,
    frame: u32,
    user: usize,
    status: &'static str,
    update: Option<u32>,
    matched_send: Option<u32>,
    matched_update: Option<u32>,
    last_send: Option<u32>,
    sent: Option<Aim>,
    pending: usize,
    dropped: u32,
    confirmed: Option<Aim>,
    predicted: Option<Aim>,
    camera: Option<Aim>,
    drawn: Option<Aim>,
    correction_rad: f32,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Params {
    after: Option<u64>,
}

impl State {
    fn read(&self, params: serde_json::Value) -> Result<serde_json::Value, RpcError> {
        let p: Params = if params.is_null() {
            Params::default()
        } else {
            serde_json::from_value(params)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
        };
        let frames: Vec<_> = self
            .history
            .iter()
            .filter(|f| p.after.is_none_or(|n| f.sample > n))
            .collect();
        Ok(serde_json::json!({ "latest": self.serial, "frames": frames }))
    }

    fn sent(&mut self, m: &dyn GuestMemory, packet: u32, enabled: bool) -> Option<()> {
        let seq = m.u32(packet).ok()?;
        let after = m.u32(packet.checked_add(4)?).ok()?;
        // A flagged 0x28 takes the view termination branch of 0x85f50.
        if m.u8(packet.checked_add(8)?).ok()? != 0 {
            return None;
        }
        let mask = m.u32(packet.checked_add(12)?).ok()?;
        for (u, slot) in self.users.iter_mut().enumerate() {
            if mask & (1 << u) != 0 && slot.observe(m, u).is_ok() {
                let action = packet.checked_add(16 + ACTION_BYTES * u as u32)?;
                if let Some(aim) = view::angles(m, action.checked_add(4)?) {
                    slot.prediction.sent(seq, after, aim, enabled);
                }
            }
        }
        Some(())
    }

    fn applied(&mut self, m: &dyn GuestMemory, record: u32) -> Option<()> {
        let update = m.u32(record).ok()?;
        let mask = m.u32(record.checked_add(8)?).ok()?;
        for (u, slot) in self.users.iter_mut().enumerate() {
            let Ok(subject) = slot.observe(m, u) else {
                continue;
            };
            // 0x694c0 indexes the world's action slots by the player's datum index.
            let player = u32::from(subject.player.index());
            let aim = if mask & (1 << player) != 0 {
                view::angles(m, record.checked_add(0x0c + player * ACTION_BYTES + 4)?)
            } else {
                None
            };
            slot.prediction.apply(update, aim);
        }
        Some(())
    }

    fn render(
        &mut self,
        m: &mut dyn GuestMemory,
        arg: u32,
        scratch: u32,
        enabled: bool,
    ) -> Option<()> {
        let original = view::ptr(m, arg)?;
        if m.u32(original).ok()? != 0 {
            return None;
        }
        let u = m.u32(original.checked_add(8)?).ok()? as usize;
        let slot = self.users.get_mut(u)?;
        let frame = m.u32(engine::render_loop::occlusion::guest::FRAME).ok()?;
        let camera = view::facing(m.vec3(original.checked_add(0x18)?).ok()?);
        let mut drawn = camera;
        let mut correction = 0.0;
        let status = match view::mode(m) {
            Some(3) => match slot.observe(m, u) {
                Err(reason) => reason,
                Ok(_) if !enabled => {
                    slot.prediction.disable();
                    "setting off"
                }
                Ok(_) => match camera.and_then(|a| slot.prediction.draw(a)) {
                    None => "waiting",
                    Some((offset, error)) => {
                        correction = error;
                        if offset == Aim::default() {
                            "active"
                        } else if view::draw_copy(
                            m,
                            arg,
                            original,
                            scratch + u as u32 * view::STRIDE,
                            offset,
                        )
                        .is_some()
                        {
                            drawn = view::facing(
                                m.vec3(scratch + u as u32 * view::STRIDE + 0x18).ok()?,
                            );
                            "active"
                        } else {
                            "invalid camera"
                        }
                    }
                },
            },
            mode => {
                *slot = User::default();
                if mode == Some(2) {
                    "host"
                } else {
                    "outside co-op"
                }
            }
        };
        let p = &slot.prediction;
        self.serial += 1;
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(Frame {
            sample: self.serial,
            frame,
            user: u,
            status,
            update: p.update,
            matched_send: p.ack_seq,
            matched_update: p.ack_update,
            last_send: p.last_send().map(|(seq, _)| seq),
            sent: p.last_send().map(|(_, aim)| aim),
            pending: p.pending(),
            dropped: p.dropped,
            confirmed: p.confirmed,
            predicted: if enabled { p.target() } else { None },
            camera,
            drawn,
            correction_rad: correction,
        });
        Some(())
    }
}

#[derive(Clone)]
struct Look {
    state: Arc<Mutex<State>>,
    arm: Arc<Arm>,
    setting: Option<Setting<bool>>,
    default: bool,
}

impl Look {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn enabled(&self) -> bool {
        self.setting.as_ref().map_or(self.default, Setting::get)
    }
}

impl Feature for Look {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "coop-look-prediction",
            "Draw pending synchronous co-op guest look",
        )
        .doc("docs/live/live-coop.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), feature::BuildError> {
        let scratch =
            b.guest_alloc(view::STRIDE * USERS as u32, runtime::guest::Permissions::RW)?;
        let look = self.clone();
        b.raw().observe_before(
            &points::SIM_ACTIONS_ENCODE,
            "outgoing guest look",
            move |o| {
                if !look.arm.active() {
                    return;
                }
                let m = o.guest();
                let esp = o.cpu.esp;
                if m.u32(esp.wrapping_add(8)).ok() == Some(0x180)
                    && view::mode(&*m) == Some(3)
                    && let Some(packet) = view::ptr(&*m, esp.wrapping_add(12))
                {
                    let _ = look.state().sent(&*m, packet, look.enabled());
                }
            },
        );
        let look = self.clone();
        b.raw().observe_before(
            &points::SIM_UPDATE_APPLY,
            "confirmed guest look",
            move |o| {
                if !look.arm.active() {
                    return;
                }
                let m = o.guest();
                if view::mode(&*m) == Some(3)
                    && let Some(record) = view::ptr(&*m, o.cpu.esp.wrapping_add(4))
                {
                    let _ = look.state().applied(&*m, record);
                }
            },
        );
        let look = self.clone();
        b.raw()
            .before(&points::RENDER_GAME_VIEW, "draw guest look", move |c| {
                if look.arm.active() {
                    let mut m = c.ctx.memory();
                    let _ = look.state().render(
                        &mut *m,
                        c.cpu.esp.wrapping_add(4),
                        scratch,
                        look.enabled(),
                    );
                }
                Ok(Flow::Continue)
            });
        let look = self.clone();
        b.raw()
            .observe_before(
                &points::SCRIPT_TELEPORT,
                "discard teleported camera",
                move |o| {
                    if !look.arm.active() {
                        return;
                    }
                    let m = o.guest();
                    if let Ok(unit) = m.u32(o.cpu.esp.wrapping_add(4)) {
                        for slot in &mut look.state().users {
                            if slot.subject.is_some_and(|s| s.unit.raw() == unit) {
                                *slot = User::default();
                            }
                        }
                    }
                },
            )
            .optional();
        for point in [
            &points::GAME_ACTIVATE,
            &points::GAME_STATE_REVERT,
            &points::SIM_STATE_RECEIVED,
            &points::BSP_SWITCH,
            &points::CINEMATIC_START,
        ] {
            let look = self.clone();
            b.raw()
                .observe_before(point, "discard old camera", move |_| {
                    look.state().users = Default::default();
                })
                .optional();
        }
        let look = self.clone();
        b.snapshot(feature::SnapshotPolicy::section(Vec::new, move |_, _| {
            look.state().users = Default::default();
            Ok(())
        }));
        Ok(())
    }
}

pub fn install(
    registry: &mut Registry,
    arm: Arc<Arm>,
    store: Option<&Store>,
    agent: Option<&agent::Agent>,
) -> anyhow::Result<()> {
    let look = Look {
        arm,
        state: Arc::default(),
        setting: store.map(|s| s.handle(COOP_LOOK_PREDICTION.key)),
        default: std::env::var("H2_COOP_LOOK_PREDICTION")
            .ok()
            .and_then(|s| COOP_LOOK_PREDICTION.parse(&s).ok())
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
    };
    if registry
        .add(&look)
        .map_err(|e| anyhow::anyhow!("co-op look prediction: {e}"))?
        && let Some(a) = agent
    {
        a.add_method(method::COOP_LOOK, agent::Access::Read, move |p| {
            look.state().read(p)
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
