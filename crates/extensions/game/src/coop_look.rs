//! Visual prediction for synchronous Live co-op. Observe the channel's
//! outgoing 0x28 and the 0x27 record actually applied to the local player.
//! Only render_game_view receives the camera adjustment, in its own copy.
//!
//! The game thread owns the prediction ([`State`]). Every game-thread
//! hook locks it through [`Look::state`], which first replays the render
//! calls recorded since ([`Op`]) and, when the hook is done, publishes a
//! snapshot of the users. The render hook works on its own copy of that
//! snapshot ([`View`]) and posts what it did to an inbox, so it touches no
//! state the game thread writes (`context_only`). Between two game-thread hooks a view draws exactly as
//! the one state did, since the copy takes the same steps the replay then
//! takes on the owner.
mod prediction;
pub mod view;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use agent_proto::{RpcError, code, method};
use guest_layout::{GuestMemory, points};
use hooks::{Builder, Feature, Flow, Manifest, Registry};
use prediction::{Aim, Prediction};
use serde::{Deserialize, Serialize};
use settings::{Setting, Store};

use crate::coop_probe::Arm;
use crate::setting_defs::COOP_LOOK_PREDICTION;
use view::Subject;

const USERS: usize = 4;
const HISTORY: usize = 256;
const ACTION_BYTES: u32 = 0x5c;

/// A film's update: write each local user's recorded facing to its
/// player-control look (`PC_YAW`, `PC_PITCH`), so a first-person view
/// looks where the recording looked (docs/game/theater-design.md,
/// "What the guest is given while a film plays"). `update` is the
/// 0x4048-byte record, its action mask at `+8` and the actions from `+0x0c`
/// by player datum index. A user with no subject (dead, in a vehicle, a
/// cinematic, a director that is not first person), no action in the mask
/// or an angle out of range keeps its look. Returns the users written.
pub fn follow_film(m: &mut dyn GuestMemory, update: u32) -> usize {
    let Some(mask) = update.checked_add(8).and_then(|at| m.u32(at).ok()) else {
        return 0;
    };
    let mut written = 0;
    for user in 0..USERS {
        let Ok(subject) = view::subject(&*m, user) else {
            continue;
        };
        let player = u32::from(subject.player.index());
        if player >= 16 || mask & (1 << player) == 0 {
            continue;
        }
        let Some(aim) = update
            .checked_add(0x0c + player * ACTION_BYTES + 4)
            .and_then(|at| view::angles(&*m, at))
        else {
            continue;
        };
        let Some(control) = view::control(&*m, user) else {
            continue;
        };
        let yaw = aim.yaw.rem_euclid(std::f32::consts::TAU);
        if m.write(control + crate::touch_glue::PC_YAW, &yaw.to_le_bytes())
            .is_ok()
            && m.write(control + crate::touch_glue::PC_PITCH, &aim.pitch.to_le_bytes())
                .is_ok()
        {
            written += 1;
        }
    }
    written
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct User {
    subject: Option<Subject>,
    prediction: Prediction,
}

impl User {
    /// Discontinuities discard sends from the old camera, including when
    /// a slot is rebound to a new salted player/unit between two frames.
    fn observe(&mut self, m: &dyn GuestMemory, user: usize) -> Result<Subject, &'static str> {
        self.rebind(view::subject(m, user))
    }

    fn rebind(&mut self, next: Result<Subject, &'static str>) -> Result<Subject, &'static str> {
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

    /// Draws `user`'s look as [`render_users`] does, on this state, and logs it.
    /// The tests' reference for the published-copy path.
    #[cfg(test)]
    fn render(
        &mut self,
        m: &mut dyn GuestMemory,
        arg: u32,
        scratch: u32,
        enabled: bool,
    ) -> Option<()> {
        let op = render_users(&mut self.users, m, arg, scratch, enabled)?;
        self.log(&op);
        Some(())
    }

    /// Takes the steps a render call took on a view's copy.
    fn replay(&mut self, op: &Op) {
        if let Some(slot) = self.users.get_mut(op.user) {
            step(slot, op.subject, op.mode, op.enabled, op.camera);
            self.log(op);
        }
    }

    fn log(&mut self, op: &Op) {
        if !op.logged {
            return;
        }
        let p = &self.users[op.user].prediction;
        self.serial += 1;
        if self.history.len() == HISTORY {
            self.history.pop_front();
        }
        self.history.push_back(Frame {
            sample: self.serial,
            frame: op.frame,
            user: op.user,
            status: op.status,
            update: p.update,
            matched_send: p.ack_seq,
            matched_update: p.ack_update,
            last_send: p.last_send().map(|(seq, _)| seq),
            sent: p.last_send().map(|(_, aim)| aim),
            pending: p.pending(),
            dropped: p.dropped,
            confirmed: p.confirmed,
            predicted: if op.enabled { p.target() } else { None },
            camera: op.camera,
            drawn: op.drawn,
            correction_rad: op.correction,
        });
    }
}

/// One render call: what it read from the guest and what it decided, enough
/// to take the same steps on the owner's state.
#[derive(Clone, Copy)]
struct Op {
    user: usize,
    frame: u32,
    /// The user's subject, read only in co-op mode.
    subject: Option<Result<Subject, &'static str>>,
    mode: Option<u32>,
    enabled: bool,
    camera: Option<Aim>,
    status: &'static str,
    drawn: Option<Aim>,
    correction: f32,
    /// Whether the call reached its end and shows in the agent's frames.
    logged: bool,
}

struct Step {
    status: &'static str,
    /// The look to add to the camera, when there is one to draw.
    offset: Option<Aim>,
    correction: f32,
}

/// The prediction's part of a render call: everything that changes `slot`.
fn step(
    slot: &mut User,
    subject: Option<Result<Subject, &'static str>>,
    mode: Option<u32>,
    enabled: bool,
    camera: Option<Aim>,
) -> Step {
    let mut out = Step { status: "", offset: None, correction: 0.0 };
    out.status = match mode {
        Some(3) => match slot.rebind(subject.unwrap_or(Err("no subject"))) {
            Err(reason) => reason,
            Ok(_) if !enabled => {
                slot.prediction.disable();
                "setting off"
            }
            Ok(_) => match camera.and_then(|a| slot.prediction.draw(a)) {
                None => "waiting",
                Some((offset, error)) => {
                    out.correction = error;
                    if offset != Aim::default() {
                        out.offset = Some(offset);
                    }
                    "active"
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
    out
}

/// A render call on `users`: reads the guest, steps the user's prediction
/// and draws its private camera. `None`: nothing to draw, nothing stepped.
fn render_users(
    users: &mut [User; USERS],
    m: &mut dyn GuestMemory,
    arg: u32,
    scratch: u32,
    enabled: bool,
) -> Option<Op> {
    let original = view::ptr(m, arg)?;
    if m.u32(original).ok()? != 0 {
        return None;
    }
    let u = m.u32(original.checked_add(8)?).ok()? as usize;
    let slot = users.get_mut(u)?;
    let frame = m.u32(engine::render_loop::occlusion::guest::FRAME).ok()?;
    let camera = view::facing(m.vec3(original.checked_add(0x18)?).ok()?);
    let mode = view::mode(m);
    let subject = (mode == Some(3)).then(|| view::subject(m, u));
    let stepped = step(slot, subject, mode, enabled, camera);
    let mut op = Op {
        user: u,
        frame,
        subject,
        mode,
        enabled,
        camera,
        status: stepped.status,
        drawn: camera,
        correction: stepped.correction,
        logged: true,
    };
    if let Some(offset) = stepped.offset {
        let private = scratch + u as u32 * view::STRIDE;
        if view::draw_copy(m, arg, original, private, offset).is_some() {
            match m.vec3(private + 0x18) {
                Ok(forward) => op.drawn = view::facing(forward),
                Err(_) => op.logged = false,
            }
        } else {
            op.status = "invalid camera";
        }
    }
    Some(op)
}

/// What the game thread last published, and when.
#[derive(Default)]
struct Published {
    epoch: u64,
    users: [User; USERS],
}

/// The render hook's copy of the published users.
#[derive(Default)]
struct View {
    epoch: Option<u64>,
    users: [User; USERS],
}

#[derive(Default)]
struct Shared {
    published: Mutex<Published>,
    view: Mutex<View>,
    /// Render calls the game thread has yet to replay, oldest first.
    inbox: Mutex<Vec<Op>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The game thread's lock on the state: renders replayed on the way in, the
/// users published on the way out.
struct Game<'a> {
    guard: MutexGuard<'a, State>,
    shared: &'a Shared,
}

impl std::ops::Deref for Game<'_> {
    type Target = State;
    fn deref(&self) -> &State {
        &self.guard
    }
}

impl std::ops::DerefMut for Game<'_> {
    fn deref_mut(&mut self) -> &mut State {
        &mut self.guard
    }
}

impl Drop for Game<'_> {
    fn drop(&mut self) {
        let mut published = lock(&self.shared.published);
        published.epoch += 1;
        published.users = self.guard.users.clone();
    }
}

#[derive(Clone)]
struct Look {
    state: Arc<Mutex<State>>,
    shared: Arc<Shared>,
    arm: Arc<Arm>,
    setting: Option<Setting<bool>>,
    default: bool,
}

impl Look {
    /// The game thread's state, with the renders since replayed.
    fn state(&self) -> Game<'_> {
        let mut guard = lock(&self.state);
        let ops = std::mem::take(&mut *lock(&self.shared.inbox));
        for op in &ops {
            guard.replay(op);
        }
        Game { guard, shared: &self.shared }
    }

    /// The render hook: draws from this view's copy of the published users
    /// and posts the call for the game thread. Takes no lock the game
    /// thread holds while it works.
    fn render(&self, m: &mut dyn GuestMemory, arg: u32, scratch: u32) {
        let mut view = lock(&self.shared.view);
        {
            let published = lock(&self.shared.published);
            if view.epoch != Some(published.epoch) {
                view.users = published.users.clone();
                view.epoch = Some(published.epoch);
            }
        }
        if let Some(op) = render_users(&mut view.users, m, arg, scratch, self.enabled()) {
            lock(&self.shared.inbox).push(op);
        }
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

    fn build(&self, b: &mut Builder) -> Result<(), hooks::BuildError> {
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
                    look.render(&mut *m, c.cpu.esp.wrapping_add(4), scratch);
                }
                Ok(Flow::Continue)
            })
            // Reads and writes this call's guest memory, and the view's own
            // copy of the users; the game thread's state is reached only
            // through the inbox.
            .context_only();
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
        b.snapshot(hooks::SnapshotPolicy::section(Vec::new, move |_, _| {
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
        shared: Arc::default(),
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
