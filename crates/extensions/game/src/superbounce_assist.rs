//! The superbounce fall assist: hold X while falling and the stick is
//! steered so the player lands on a bounce strip (docs/game/superbounce.md,
//! "Hold X: the fall assist").
//!
//! It works on input, not on physics. An input tap ([`AssistTap`]) sits at
//! `xapi`'s `XInputGetState`, under the agent's tap and over a recording's or
//! replay's. Each read for the first controller it checks one atomic
//! ([`superbounce_mode::active`]); only with the mode on, X held and the
//! player in the air does it read the player's body, pick a strip, and
//! replace the left stick in the packet the guest is about to read, and
//! take X out of it. It changes the packet before the tap under it sees it,
//! so a film or input log records the pad the guest got, and a replay (which
//! overwrites the packet) never runs the assist. The [`SuperbounceAssist`]
//! feature has no guest hooks: nothing about it enters a film's hook digest.
//!
//! The rules are pure functions over a [`Body`] and the scan's spots:
//! [`fall_time`], [`predicted_landing`], [`reach`] (the gate), [`pick`],
//! [`steer`] (the PD controller), [`world_to_stick`] and [`apply`] (the
//! stick and the button).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use engine::memory::{GuestMemory, MemFault};
use input::gamepad::{AnalogButton, Gamepad};
use superbounce::{Kind, Spot, Vec3};
use xapi::tap::{InputTap, PACKET};

use crate::superbounce_mode;

/// The feature's name (`H2_FEATURES` switches it).
pub const FEATURE: &str = "superbounce-assist";

// ---- what the player holds ----

/// The button that asks for the assist. Pad X; on the keyboard that is E
/// (`crates/platform/input/src/bindings.rs`, a rebinding follows), which
/// the keyboard backend merges into the same pad packet.
pub const BUTTON: AnalogButton = AnalogButton::X;
/// A face button's pressure at which it counts as held (XInput's trigger
/// threshold).
pub const PRESSED: u8 = 30;
/// The controller port the assist works for: the first local player.
pub const PORT: u32 = 0;

// ---- the numbers ----

/// The strip's stand point must be within this horizontal distance (world
/// units).
pub const REACH_RADIUS: f32 = 3.0;
/// Aim this far before the stand point along the drift (world units).
pub const AIM_BACK: f32 = 0.1;
/// Cross the strip at this speed along the drift (units/s).
pub const ARRIVE_SPEED: f32 = 0.8;
/// Gravity (units/s²): the 4.15 that fits the traced jump
/// (docs/game/superbounce.md, "Spots in the geometry").
pub const GRAVITY: f32 = 4.15;
/// What a full stick pushes a biped sideways in the air (units/s²). An
/// estimate: the stick is this scaled, and the lab is where it is measured.
pub const AIR_ACCEL: f32 = 3.0;
/// Slack on the reach gate (units): a good approach whose stick is weaker
/// or stronger than [`AIR_ACCEL`] says wanders from the coasting line and
/// must not drop out.
pub const HOLD_SLACK: f32 = 0.4;
/// The assist only works on a player falling, not one still rising
/// (units/s of vertical speed).
pub const FALLING_VZ: f32 = 0.5;
/// A strip this far below the player (units) or less is landed on.
pub const MIN_DROP: f32 = 0.02;
/// Falls longer than this (seconds) are not steered.
pub const MAX_TIME: f32 = 3.0;
/// The PD controller's gains: 5 rad/s, damping 0.8. Higher ones overshoot
/// into the gate's slack when the stick is stronger than [`AIR_ACCEL`].
pub const KP: f32 = 25.0;
pub const KD: f32 = 8.0;
/// The aim vector's horizontal length below which a yaw means nothing
/// (looking straight up or down).
const MIN_AIM: f32 = 0.05;
/// Where an object's position is, in its body.
const POSITION: u32 = crate::telemetry::layout::BODY_POSITION;
/// A biped's unit kind in [`crate::skulls_custom::LocalUnit`].
const BIPED_KIND: u8 = 0;

// ---- the pure rules ----

/// What the assist needs of the player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    pub position: Vec3,
    /// World units a second.
    pub velocity: Vec3,
    /// The aim's yaw (radians, counter-clockwise from +X): the stick's
    /// forward.
    pub yaw: f32,
    pub airborne: bool,
}

/// Seconds until a body `height` above a floor, moving `vz` up, reaches it
/// under [`GRAVITY`]; `None` when it never does (it stays above or is
/// below).
pub fn fall_time(height: f32, vz: f32) -> Option<f32> {
    let d = vz * vz + 2.0 * GRAVITY * height;
    if !d.is_finite() || d < 0.0 {
        return None;
    }
    let t = (vz + d.sqrt()) / GRAVITY;
    (t.is_finite() && t >= 0.0).then_some(t)
}

/// Where a body at `p` moving `v` is after `t` seconds, coasting.
pub fn predicted_landing(p: [f32; 2], v: [f32; 2], t: f32) -> [f32; 2] {
    [p[0] + v[0] * t, p[1] + v[1] * t]
}

/// A drift as a unit vector; zero stays zero.
pub fn drift_unit(drift: [f32; 2]) -> [f32; 2] {
    let n = drift[0].hypot(drift[1]);
    if n > 1e-6 && n.is_finite() {
        [drift[0] / n, drift[1] / n]
    } else {
        [0.0; 2]
    }
}

/// Why a strip can be steered onto, and where to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    /// Seconds to touchdown on the strip's height.
    pub time: f32,
    /// The point to cross: the stand point, [`AIM_BACK`] before it along
    /// the drift.
    pub aim: [f32; 2],
    /// The velocity to cross it at: [`ARRIVE_SPEED`] along the drift.
    pub arrive: [f32; 2],
    /// Where the player lands if the stick is let go.
    pub landing: [f32; 2],
    /// How far that is from [`Reach::aim`].
    pub miss: f32,
}

/// The reach cone: whether `body` can be steered onto `spot`'s strip before
/// it lands.
///
/// - the spot is a bounce spot, the player is airborne and falling
///   ([`FALLING_VZ`]);
/// - the stand point is below the player and within [`REACH_RADIUS`]
///   sideways;
/// - the coasting landing is within what the stick can move in the time left
///   (half [`AIR_ACCEL`] times the fall time squared, plus
///   [`HOLD_SLACK`]), which makes the cone wider the longer the fall.
pub fn reach(body: &Body, spot: &Spot) -> Option<Reach> {
    if spot.kind != Kind::Bounce || !body.airborne || body.velocity[2] > FALLING_VZ {
        return None;
    }
    let drop = body.position[2] - spot.stand[2];
    if drop < MIN_DROP {
        return None;
    }
    let side = (spot.stand[0] - body.position[0]).hypot(spot.stand[1] - body.position[1]);
    if side > REACH_RADIUS {
        return None;
    }
    let time = fall_time(drop, body.velocity[2]).filter(|&t| t <= MAX_TIME)?;
    let dir = drift_unit(spot.drift);
    let aim = [
        spot.stand[0] - AIM_BACK * dir[0],
        spot.stand[1] - AIM_BACK * dir[1],
    ];
    let arrive = [ARRIVE_SPEED * dir[0], ARRIVE_SPEED * dir[1]];
    let landing = predicted_landing(
        [body.position[0], body.position[1]],
        [body.velocity[0], body.velocity[1]],
        time,
    );
    let miss = (aim[0] - landing[0]).hypot(aim[1] - landing[1]);
    (miss <= 0.5 * AIR_ACCEL * time * time + HOLD_SLACK).then_some(Reach {
        time,
        aim,
        arrive,
        landing,
        miss,
    })
}

/// The strip to steer onto: of the reachable bounce spots, the one the
/// coasting landing misses by least.
pub fn pick<'a>(spots: &'a [Spot], body: &Body) -> Option<(&'a Spot, Reach)> {
    spots
        .iter()
        .filter_map(|s| reach(body, s).map(|r| (s, r)))
        .min_by(|a, b| a.1.miss.total_cmp(&b.1.miss))
}

/// The horizontal acceleration to ask for (units/s², world axes): a PD
/// controller on the track that lands on [`Reach::aim`] at [`Reach::arrive`]
/// (the point the player would be at, `time` before touchdown, moving at the
/// arrival velocity all the way). Its error is the predicted landing's miss
/// when the velocities match.
pub fn steer(body: &Body, r: &Reach) -> [f32; 2] {
    let mut a = [0.0; 2];
    for i in 0..2 {
        let track = r.aim[i] - r.arrive[i] * r.time;
        a[i] = KP * (track - body.position[i]) + KD * (r.arrive[i] - body.velocity[i]);
    }
    a
}

/// A world acceleration as the left stick `[x (right), y (forward)]`, in
/// -1..1: scaled by [`AIR_ACCEL`], turned into the aim's frame, and cut
/// back to the unit disc keeping its direction.
pub fn world_to_stick(accel: [f32; 2], yaw: f32) -> [f32; 2] {
    if !accel.iter().chain(&[yaw]).all(|v| v.is_finite()) {
        return [0.0; 2];
    }
    let (s, c) = yaw.sin_cos();
    // Forward is (cos, sin); right, with Z up, is (sin, -cos).
    let x = (accel[0] * s - accel[1] * c) / AIR_ACCEL;
    let y = (accel[0] * c + accel[1] * s) / AIR_ACCEL;
    let n = x.hypot(y);
    if n > 1.0 { [x / n, y / n] } else { [x, y] }
}

/// The reverse of [`world_to_stick`] below saturation: the world
/// acceleration a stick asks for.
pub fn stick_to_world(stick: [f32; 2], yaw: f32) -> [f32; 2] {
    let (s, c) = yaw.sin_cos();
    [
        (stick[0] * s + stick[1] * c) * AIR_ACCEL,
        (-stick[0] * c + stick[1] * s) * AIR_ACCEL,
    ]
}

/// A stick as the pad's signed 16-bit axes.
pub fn stick_raw(stick: [f32; 2]) -> (i16, i16) {
    let q = |v: f32| (v.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
    (q(stick[0]), q(stick[1]))
}

/// What the assist does this read.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    /// The strip's stand point.
    pub target: Vec3,
    pub reach: Reach,
    /// The left stick to send.
    pub stick: [f32; 2],
}

/// The plan for `body` over `spots`, if a strip is in reach.
pub fn plan(spots: &[Spot], body: &Body) -> Option<Plan> {
    let (spot, reach) = pick(spots, body)?;
    Some(Plan {
        target: spot.stand,
        stick: world_to_stick(steer(body, &reach), body.yaw),
        reach,
    })
}

/// The landing a falling `body` is headed for, for the HUD and the world
/// ring (docs/game/superbounce.md, "Landings"): of the bounce strips below
/// it whose stand point the coasting landing ([`predicted_landing`] at
/// [`fall_time`]) is within [`REACH_RADIUS`] of, the one it lands nearest
/// the slot of. `on_slot` when it lands inside the slot, `fixable` when it
/// does not and [`reach`] says the assist can still bring it in, else
/// `miss`. `None` on the ground, rising, or with no such strip.
pub fn predict(spots: &[Spot], body: &Body) -> Option<superbounce_mode::Predicted> {
    use crate::superbounce_charge::{edge_distance_xy, need_of, slot_miss, width_of};
    use superbounce_mode::{Course, Predicted};
    let vz = body.velocity[2];
    if !body.airborne || !vz.is_finite() || vz >= 0.0 {
        return None;
    }
    let p = [body.position[0], body.position[1]];
    let v = [body.velocity[0], body.velocity[1]];
    let (spot, landing, height, _) = spots
        .iter()
        .filter(|s| s.kind == Kind::Bounce)
        .filter_map(|s| {
            let height = body.position[2] - s.stand[2];
            if height < MIN_DROP {
                return None;
            }
            let time = fall_time(height, vz).filter(|&t| t <= MAX_TIME)?;
            let landing = predicted_landing(p, v, time);
            let side = (s.stand[0] - landing[0]).hypot(s.stand[1] - landing[1]);
            (side <= REACH_RADIUS).then(|| (s, landing, height, edge_distance_xy(s, landing)))
        })
        .min_by(|a, b| a.3.total_cmp(&b.3))?;
    let verdict = if slot_miss(spot, landing) <= 0.0 {
        Course::OnSlot
    } else if reach(body, spot).is_some() {
        Course::Fixable
    } else {
        Course::Miss
    };
    Some(Predicted {
        at: [landing[0], landing[1], spot.stand[2]],
        verdict,
        speed: (vz * vz + 2.0 * GRAVITY * height).sqrt(),
        need: need_of(spot),
        edge: spot.edge,
        stand: spot.stand,
        width: width_of(spot),
        fall: spot.fall,
    })
}

/// Whether the button is being swallowed: from the first read the assist
/// engaged on, until the player lets go. Without it the press that began in
/// the air would reach the game the moment the strip went out of reach (a
/// landing, a miss), as a fresh press.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Latch(pub bool);

/// Puts the assist's decision into `pad`: the stick, and X out while the
/// button is swallowed. `held` is whether the player holds the button;
/// `plan` is what the assist would do (ignored when not held).
/// Returns whether the assist is engaged.
pub fn apply(latch: &mut Latch, pad: &mut Gamepad, held: bool, plan: Option<&Plan>) -> bool {
    let plan = plan.filter(|_| held);
    latch.0 = held && (latch.0 || plan.is_some());
    if let Some(p) = plan {
        let (lx, ly) = stick_raw(p.stick);
        pad.thumb_lx = lx;
        pad.thumb_ly = ly;
    }
    if latch.0 {
        pad.analog[BUTTON.index()] = 0;
    }
    plan.is_some()
}

// ---- what the agent sees ----

/// The assist's last read, for `superbounce.assist`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Report {
    pub engaged: bool,
    /// The strip's stand point.
    pub target: Option<Vec3>,
    /// The point being aimed at.
    pub aim: Option<[f32; 2]>,
    /// Where the player lands if the stick is let go.
    pub landing: Option<[f32; 2]>,
    /// Seconds to touchdown.
    pub time: Option<f32>,
    /// The left stick sent, `[x (right), y (forward)]`.
    pub stick: Option<[f32; 2]>,
}

impl Report {
    const EMPTY: Report = Report {
        engaged: false,
        target: None,
        aim: None,
        landing: None,
        time: None,
        stick: None,
    };

    fn of(p: &Plan) -> Report {
        Report {
            engaged: true,
            target: Some(p.target),
            aim: Some(p.reach.aim),
            landing: Some(p.reach.landing),
            time: Some(p.reach.time),
            stick: Some(p.stick),
        }
    }
}

// ---- the crouch button ----

/// The player's input settings records (`0x51ea18 + 0x1c × player`), whose
/// bytes 8 to 0x17 map each action slot to the physical button that drives
/// it (docs/input/controller-layouts.md, "The action table").
const ACTION_TABLE: u32 = 0x0051_ea18;
const ACTION_TABLE_STRIDE: u32 = 0x1c;
const ACTION_SLOTS: u32 = 8;
/// Crouch is action slot 10 (docs/game/bots.md, "The action from a pad").
const CROUCH_SLOT: u32 = 10;
/// The Default layout's crouch button: the left stick's click.
const DEFAULT_CROUCH_BUTTON: u8 = 14;

/// Whether physical button `index` of the action table (0 to 7 the analog
/// buttons A, B, X, Y, Black, White, LT, RT; 8 to 15 the digital bits
/// d-pad up, down, left, right, Start, Back, left and right stick) is down
/// in `pad`. `None` for an index past 15.
pub fn button_down(pad: &Gamepad, index: u8) -> Option<bool> {
    match index {
        0..=7 => Some(pad.analog[usize::from(index)] >= PRESSED),
        8..=15 => Some(pad.buttons.0 & (1 << (index - 8)) != 0),
        _ => None,
    }
}

/// The button the player's layout crouches with, from the action table;
/// the Default layout's when it cannot be read.
fn crouch_button(memory: Option<&dyn xapi::GuestMemory>) -> u8 {
    let at = ACTION_TABLE + ACTION_TABLE_STRIDE * PORT + ACTION_SLOTS + CROUCH_SLOT;
    let mut b = [0u8; 1];
    match memory.map(|m| m.read(at, &mut b)) {
        Some(Ok(())) if b[0] <= 15 => b[0],
        _ => DEFAULT_CROUCH_BUTTON,
    }
}

/// The crouch button as the guest last read it on [`PORT`]:
/// [`CROUCH_UNKNOWN`], [`CROUCH_UP`] or [`CROUCH_DOWN`].
static CROUCH: AtomicU8 = AtomicU8::new(CROUCH_UNKNOWN);
const CROUCH_UNKNOWN: u8 = 0;
const CROUCH_UP: u8 = 1;
const CROUCH_DOWN: u8 = 2;

/// Whether the first player holds crouch in the pad the guest last read,
/// with the agent's steps and before a replay's; `None` when the tap is
/// not installed or the mode is off. The charge's `held` phase reads it.
pub fn crouch_held() -> Option<bool> {
    match CROUCH.load(Ordering::Relaxed) {
        CROUCH_UP => Some(false),
        CROUCH_DOWN => Some(true),
        _ => None,
    }
}

fn publish_crouch(held: Option<bool>) {
    let v = match held {
        Some(true) => CROUCH_DOWN,
        Some(false) => CROUCH_UP,
        None => CROUCH_UNKNOWN,
    };
    CROUCH.store(v, Ordering::Relaxed);
}

static ENABLED: AtomicBool = AtomicBool::new(false);
/// The player has turned the assist on (`superbounce.fall_assist`).
static OPTED_IN: AtomicBool = AtomicBool::new(false);
/// The tap is in place ([`install_tap`] wrapped the controller reads).
static TAPPED: AtomicBool = AtomicBool::new(false);
static ENGAGED: AtomicBool = AtomicBool::new(false);
static TEST_HOLD: AtomicBool = AtomicBool::new(false);
static REPORT: Mutex<Report> = Mutex::new(Report::EMPTY);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn publish(r: Report) {
    ENGAGED.store(r.engaged, Ordering::Relaxed);
    *lock(&REPORT) = r;
}

/// Whether the player has turned the assist on: the
/// `superbounce.fall_assist` setting, off by default. Off, X is never
/// read, nothing steers, and nothing on the map or the HUD asks for X.
pub fn opted_in() -> bool {
    OPTED_IN.load(Ordering::Relaxed)
}

/// Sets what [`opted_in`] answers (the setting's live value).
pub fn set_opted_in(on: bool) {
    OPTED_IN.store(on, Ordering::Relaxed);
}

// ---- the verifier's pad ----

/// A whole pad for the first controller, set by `superbounce.verify`'s
/// trials on `player.0` (crates/extensions/game/src/superbounce_verify.rs):
/// the left stick, the crouch button of the player's layout, and A.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VerifyPad {
    /// `[x (right), y (forward)]`, -1..1.
    pub stick: [f32; 2],
    pub crouch: bool,
    pub jump: bool,
}

static VERIFY_PAD: Mutex<Option<VerifyPad>> = Mutex::new(None);

/// Replaces the first controller's pad with `pad` at every read until
/// called with `None`, whether the mode is on or not. `false` when there is
/// no tap to do it (the `superbounce-assist` feature is off).
pub fn set_verify_pad(pad: Option<VerifyPad>) -> bool {
    *lock(&VERIFY_PAD) = pad;
    TAPPED.load(Ordering::Relaxed)
}

/// Whether the tap is in place, so [`set_verify_pad`] reaches the game.
pub fn tapped() -> bool {
    TAPPED.load(Ordering::Relaxed)
}

/// `pad` with the verifier's pad in place of what the player pressed.
pub fn verify_pad_into(pad: &mut Gamepad, v: &VerifyPad, crouch_button: u8) {
    *pad = Gamepad::default();
    let (x, y) = stick_raw(v.stick);
    pad.thumb_lx = x;
    pad.thumb_ly = y;
    if v.jump {
        pad.analog[AnalogButton::A.index()] = 255;
    }
    if v.crouch {
        match crouch_button {
            0..=7 => pad.analog[usize::from(crouch_button)] = 255,
            8..=15 => pad.buttons.0 |= 1 << (crouch_button - 8),
            _ => pad.buttons.0 |= 1 << (DEFAULT_CROUCH_BUTTON - 8),
        }
    }
}

/// Whether the assist is steering the player now: the HUD can show it.
pub fn assisting() -> bool {
    ENGAGED.load(Ordering::Relaxed)
}

/// The assist's last read.
pub fn report() -> Report {
    *lock(&REPORT)
}

/// `superbounce.assist_test`: acts as if the button is held (`true`) or
/// stops (`false`), so an agent can try the assist with no pad. Returns
/// the setting.
pub fn set_test_hold(hold: bool) -> bool {
    TEST_HOLD.store(hold, Ordering::Relaxed);
    hold
}

/// `superbounce.assist` takes no parameters: null or `{}`.
pub fn no_params(params: &serde_json::Value) -> Result<(), &'static str> {
    if params.is_null() || params.as_object().is_some_and(|f| f.is_empty()) {
        Ok(())
    } else {
        Err("superbounce.assist takes no parameters")
    }
}

/// `superbounce.assist` as the agent answers it.
pub fn assist_reply() -> agent_proto::superbounce::Assist {
    let r = report();
    let wide = |v: Vec3| v.map(f64::from);
    let wide2 = |v: [f32; 2]| v.map(f64::from);
    agent_proto::superbounce::Assist {
        active: superbounce_mode::active(),
        enabled: opted_in(),
        test_hold: TEST_HOLD.load(Ordering::Relaxed),
        engaged: r.engaged,
        target: r.target.map(wide),
        aim: r.aim.map(wide2),
        landing: r.landing.map(wide2),
        fall_time: r.time.map(f64::from),
        stick: r.stick.map(wide2),
    }
}

// ---- reading the game ----

struct Mem<'a>(&'a dyn xapi::GuestMemory);

impl GuestMemory for Mem<'_> {
    fn read(&self, addr: u32, buf: &mut [u8]) -> Result<(), MemFault> {
        self.0.read(addr, buf).map_err(|_| MemFault {
            addr,
            len: buf.len() as u32,
            write: false,
        })
    }

    fn write(&mut self, addr: u32, data: &[u8]) -> Result<(), MemFault> {
        Err(MemFault {
            addr,
            len: data.len() as u32,
            write: true,
        })
    }
}

/// The body of the local player on controller `port`, for the prompts too.
pub(crate) fn read_body(m: &dyn GuestMemory, port: u32) -> Option<Body> {
    use crate::telemetry::layout;
    let unit = crate::skulls_custom::local_units(m)
        .ok()?
        .into_iter()
        .find(|u| u.kind == BIPED_KIND && i64::from(u.user) == i64::from(port))?;
    let base = unit.body;
    let position = m.vec3(base.wrapping_add(POSITION)).ok()?;
    let velocity = m.vec3(base.wrapping_add(layout::BODY_VELOCITY)).ok()?;
    let aim = m.vec3(base.wrapping_add(layout::UNIT_AIM)).ok()?;
    let flags = m.u16(base.wrapping_add(layout::BIPED_FLAGS)).ok()?;
    if !position
        .iter()
        .chain(&velocity)
        .chain(&aim)
        .all(|v| v.is_finite())
        || aim[0].hypot(aim[1]) < MIN_AIM
    {
        return None;
    }
    Some(Body {
        position,
        velocity,
        yaw: aim[1].atan2(aim[0]),
        airborne: flags & 1 != 0,
    })
}

// ---- the tap ----

/// The assist at `XInputGetState`.
pub struct AssistTap {
    inner: Option<Box<dyn InputTap>>,
    /// Controller port each open handle was opened on.
    ports: HashMap<u32, u32>,
    latch: Latch,
    /// The report holds an engaged read.
    live: bool,
}

impl AssistTap {
    pub fn new(inner: Option<Box<dyn InputTap>>) -> Self {
        AssistTap {
            inner,
            ports: HashMap::new(),
            latch: Latch::default(),
            live: false,
        }
    }

    fn release(&mut self) {
        if self.live {
            self.live = false;
            publish(Report::EMPTY);
        }
        self.latch = Latch::default();
    }

    fn assist(
        &mut self,
        handle: u32,
        status: u32,
        packet: &mut [u8; PACKET],
        memory: Option<&dyn xapi::GuestMemory>,
    ) {
        // A verifier's trial owns the first pad whatever the mode.
        if status == 0
            && self.ports.get(&handle) == Some(&PORT)
            && let Some(v) = *lock(&VERIFY_PAD)
        {
            let raw: &[u8; Gamepad::SIZE] = (&packet[4..])
                .try_into()
                .expect("a packet is a number and a gamepad");
            let mut pad = Gamepad::from_bytes(raw);
            let button = crouch_button(memory);
            verify_pad_into(&mut pad, &v, button);
            packet[4..].copy_from_slice(&pad.to_bytes());
            publish_crouch(Some(v.crouch));
            if self.live || self.latch.0 {
                self.release();
            }
            return;
        }
        // The cost of a game that is not a superbounce one.
        if !superbounce_mode::active() {
            if self.live || self.latch.0 {
                self.release();
            }
            publish_crouch(None);
            return;
        }
        if status != 0 || self.ports.get(&handle) != Some(&PORT) {
            return;
        }
        let raw: &[u8; Gamepad::SIZE] = (&packet[4..])
            .try_into()
            .expect("a packet is a number and a gamepad");
        let mut pad = Gamepad::from_bytes(raw);
        // The charge's `held` phase: the crouch button of the player's own
        // layout, in the pad as the guest gets it.
        publish_crouch(button_down(&pad, crouch_button(memory)));
        // The ghost run's button (superbounce_ghost.rs).
        crate::superbounce_ghost::publish_button(button_down(
            &pad,
            crate::superbounce_ghost::BUTTON,
        ));
        // Opt in: with the setting off the button is the game's.
        let held =
            opted_in() && (pad.analog(BUTTON) >= PRESSED || TEST_HOLD.load(Ordering::Relaxed));
        if !held && !self.latch.0 {
            self.release();
            return;
        }
        let plan = if held {
            memory
                .and_then(|m| read_body(&Mem(m), PORT))
                .and_then(|b| plan(&superbounce_mode::current_spots(), &b))
        } else {
            None
        };
        let engaged = apply(&mut self.latch, &mut pad, held, plan.as_ref());
        packet[4..].copy_from_slice(&pad.to_bytes());
        match plan.filter(|_| engaged) {
            Some(p) => {
                self.live = true;
                publish(Report::of(&p));
            }
            None if self.live => {
                self.live = false;
                publish(Report::EMPTY);
            }
            None => {}
        }
    }
}

impl InputTap for AssistTap {
    fn read(&mut self, handle: u32, status: &mut u32, packet: &mut [u8; PACKET]) {
        self.read_with_memory(handle, status, packet, None);
    }

    fn read_with_memory(
        &mut self,
        handle: u32,
        status: &mut u32,
        packet: &mut [u8; PACKET],
        memory: Option<&dyn xapi::GuestMemory>,
    ) {
        // Before the tap under this one: a recording logs the packet the
        // guest gets, and a replay overwrites it.
        self.assist(handle, *status, packet, memory);
        if let Some(t) = &mut self.inner {
            t.read_with_memory(handle, status, packet, memory);
        }
    }

    fn sync(&mut self, input: &mut input::InputManager) {
        match &mut self.inner {
            Some(t) => t.sync(input),
            None => input.sync(),
        }
    }

    fn restore_input(
        &mut self,
        cursor: u64,
        input: &mut input::InputManager,
    ) -> Result<(), String> {
        match &mut self.inner {
            Some(t) => t.restore_input(cursor, input),
            None => Ok(()),
        }
    }

    fn opened(&mut self, handle: u32, port: u32) {
        self.ports.insert(handle, port);
        if let Some(t) = &mut self.inner {
            t.opened(handle, port);
        }
    }

    fn closed(&mut self, handle: u32) {
        self.ports.remove(&handle);
        if let Some(t) = &mut self.inner {
            t.closed(handle);
        }
    }

    fn cursor(&self) -> Option<u64> {
        self.inner.as_ref().and_then(|t| t.cursor())
    }

    fn seek(&mut self, cursor: u64) -> Result<(), String> {
        match &mut self.inner {
            Some(t) => t.seek(cursor),
            None => Ok(()),
        }
    }
}

/// Wraps `xapi`'s input tap (a recording's, a replay's, or none) in the
/// assist, when the feature is on. Call once, before the agent installs its
/// own tap over the result.
pub fn install_tap<S: xapi::Storage>(xapi: Option<&Mutex<xapi::Xapi<S>>>) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(x) = xapi else { return };
    let mut x = lock(x);
    let inner = x.take_input_tap();
    x.set_input_tap(Some(Box::new(AssistTap::new(inner))));
    TAPPED.store(true, Ordering::Relaxed);
}

// ---- the feature ----

/// The feature: its manifest and its snapshot policy. The work is in the
/// tap ([`install_tap`]); it has no guest hooks.
pub struct SuperbounceAssist;

impl hooks::Feature for SuperbounceAssist {
    fn manifest(&self) -> hooks::Manifest {
        // Not presentation-only: it changes the pad the guest reads. It
        // adds no hook, so a film's hook digest is unchanged by it, and a
        // film records the pad as sent.
        hooks::Manifest::new(
            FEATURE,
            "Superbounce fall assist: hold X to steer onto a strip",
        )
        .doc("docs/game/superbounce.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // Everything is re-read from guest RAM at each pad read; what the
        // tap keeps (the swallowed-button latch, the test hold, the last
        // report) is momentary, and losing it in a load costs one press.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds the feature to `registry`. Whether the tap is installed follows
/// `H2_FEATURES` (the kill switch); whether it steers follows the
/// `superbounce.fall_assist` setting in `settings`, read live (off without
/// a store).
pub fn install(
    registry: &mut hooks::Registry,
    settings: Option<Arc<settings::Store>>,
) -> anyhow::Result<()> {
    let on = registry
        .add(&SuperbounceAssist)
        .map_err(|e| anyhow::anyhow!("superbounce assist: {e}"))?;
    ENABLED.store(on, Ordering::Relaxed);
    set_opted_in(false);
    if let Some(s) = &settings {
        follow_setting(s);
    }
    Ok(())
}

/// Makes [`opted_in`] follow the `superbounce.fall_assist` setting in
/// `store`: its value now, then each change.
pub fn follow_setting(store: &settings::Store) {
    let key = crate::setting_defs::SUPERBOUNCE_FALL_ASSIST.key;
    set_opted_in(store.handle::<bool>(key).get());
    store.on_change(key, |v| {
        set_opted_in(matches!(v, settings::Value::Bool(true)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spot_at(stand: Vec3, drift: [f32; 2]) -> Spot {
        Spot {
            kind: Kind::Bounce,
            edge: [stand, stand],
            stand,
            fall: 1.0,
            launch: 12.0,
            drift,
            score: 1.0,
            ..Spot::default()
        }
    }

    fn falling(at: Vec3, v: Vec3) -> Body {
        Body {
            position: at,
            velocity: v,
            yaw: 0.0,
            airborne: true,
        }
    }

    fn near(a: f32, b: f32, eps: f32) {
        assert!((a - b).abs() <= eps, "{a} vs {b} (±{eps})");
    }

    // ---- the landing prediction ----

    #[test]
    fn a_drop_from_rest_takes_the_free_fall_time() {
        // 2.43 units at 4.15 units/s² lands at sqrt(2gh) = 4.49 units/s.
        // (The lab's drop from 3.5 above the strip landed at 5.24 after a
        // 0.23 start, which fits the same gravity.)
        let t = fall_time(2.43, 0.0).unwrap();
        near(t, (2.0 * 2.43 / GRAVITY).sqrt(), 1e-5);
        near(GRAVITY * t, (2.0 * GRAVITY * 2.43).sqrt(), 1e-4);
        near(fall_time(0.0, -5.4).unwrap(), 0.0, 1e-5);
    }

    #[test]
    fn a_rising_body_lands_later_and_one_below_the_floor_never_does() {
        near(fall_time(0.0, 3.0).unwrap(), 2.0 * 3.0 / GRAVITY, 1e-5);
        assert_eq!(fall_time(-1.0, 0.0), None);
        assert_eq!(fall_time(f32::NAN, 0.0), None);
    }

    #[test]
    fn the_landing_is_where_coasting_leads() {
        assert_eq!(predicted_landing([1.0, 2.0], [0.5, -1.0], 2.0), [2.0, 0.0]);
        assert_eq!(predicted_landing([1.0, 2.0], [0.0, 0.0], 5.0), [1.0, 2.0]);
    }

    // ---- the reach cone ----

    fn gate(body: Body, spot: Spot) -> bool {
        reach(&body, &spot).is_some()
    }

    #[test]
    fn a_strip_below_and_close_is_in_reach() {
        let spot = spot_at([0.0, 0.0, 0.0], [1.0, 0.0]);
        let r = reach(&falling([0.3, 0.2, 2.4], [0.0; 3]), &spot).unwrap();
        near(r.aim[0], -AIM_BACK, 1e-6);
        near(r.arrive[0], ARRIVE_SPEED, 1e-6);
        near(r.time, fall_time(2.4, 0.0).unwrap(), 1e-6);
        assert_eq!(r.landing, [0.3, 0.2]);
    }

    #[test]
    fn the_cone_refuses_what_is_above_far_or_not_a_fall() {
        let spot = spot_at([0.0, 0.0, 0.0], [1.0, 0.0]);
        let ok = falling([0.0, 0.0, 2.4], [0.0; 3]);
        assert!(gate(ok, spot));
        // Not below the player, or level with them.
        assert!(!gate(falling([0.0, 0.0, -1.0], [0.0; 3]), spot));
        assert!(!gate(falling([0.0, 0.0, 0.01], [0.0; 3]), spot));
        // Too far sideways, however long the fall.
        assert!(!gate(falling([3.5, 0.0, 30.0], [0.0; 3]), spot));
        // On the ground, or rising (a bounce's flight, a jump's start).
        assert!(!gate(
            Body {
                airborne: false,
                ..ok
            },
            spot
        ));
        assert!(!gate(falling([0.0, 0.0, 2.4], [0.0, 0.0, 12.0]), spot));
        // A crouch spot is not a strip.
        assert!(!gate(
            ok,
            Spot {
                kind: Kind::Crouch,
                ..spot
            }
        ));
    }

    #[test]
    fn the_cone_widens_with_the_time_left_to_steer() {
        let spot = spot_at([2.5, 0.0, 0.0], [1.0, 0.0]);
        // 2.5 sideways: out of reach from a short fall, within it from a
        // long one.
        assert!(!gate(falling([0.0, 0.0, 0.3], [0.0; 3]), spot));
        assert!(gate(falling([0.0, 0.0, 4.0], [0.0; 3]), spot));
        // Coasting the wrong way takes it out of reach again.
        assert!(!gate(falling([0.0, 0.0, 1.0], [-3.0, 0.0, 0.0]), spot));
    }

    #[test]
    fn a_drift_less_spot_aims_at_the_stand_point_at_rest() {
        let spot = spot_at([1.0, 1.0, 0.0], [0.0, 0.0]);
        let r = reach(&falling([1.0, 1.0, 2.0], [0.0; 3]), &spot).unwrap();
        assert_eq!(r.aim, [1.0, 1.0]);
        assert_eq!(r.arrive, [0.0, 0.0]);
    }

    #[test]
    fn the_spot_that_coasting_misses_by_least_is_picked() {
        let a = spot_at([1.5, 0.0, 0.0], [1.0, 0.0]);
        let b = spot_at([-0.4, 0.2, 0.0], [1.0, 0.0]);
        let body = falling([0.0, 0.0, 2.0], [0.0; 3]);
        let spots = [a, b];
        let (picked, _) = pick(&spots, &body).unwrap();
        assert_eq!(picked.stand, b.stand);
        assert!(pick(&[], &body).is_none());
    }

    // ---- the stick ----

    #[test]
    fn the_stick_is_in_the_aims_frame() {
        let a = AIR_ACCEL;
        let at = |accel: [f32; 2], yaw: f32| world_to_stick(accel, yaw);
        let same = |s: [f32; 2], x: f32, y: f32| {
            near(s[0], x, 1e-5);
            near(s[1], y, 1e-5);
        };
        use std::f32::consts::{FRAC_PI_2, PI};
        // Facing +X: forward is +X, left (+Y) is stick left.
        same(at([a, 0.0], 0.0), 0.0, 1.0);
        same(at([0.0, a], 0.0), -1.0, 0.0);
        // Facing +Y: +X is on the right.
        same(at([a, 0.0], FRAC_PI_2), 1.0, 0.0);
        same(at([0.0, a], FRAC_PI_2), 0.0, 1.0);
        // Facing -X: the world turns around.
        same(at([a, 0.0], PI), 0.0, -1.0);
        same(at([0.0, a], PI), 1.0, 0.0);
        // Facing -Y.
        same(at([a, 0.0], -FRAC_PI_2), -1.0, 0.0);
        same(at([0.0, a], -FRAC_PI_2), 0.0, -1.0);
    }

    #[test]
    fn the_stick_round_trips_and_saturates_keeping_its_direction() {
        for yaw in [0.0f32, 0.7, 2.0, -1.2, 3.0, -3.0] {
            let want = [0.9, -1.1];
            let back = stick_to_world(world_to_stick(want, yaw), yaw);
            near(back[0], want[0], 1e-4);
            near(back[1], want[1], 1e-4);
            let hard = world_to_stick([300.0, -400.0], yaw);
            near(hard[0].hypot(hard[1]), 1.0, 1e-5);
            let w = stick_to_world(hard, yaw);
            near(w[0] / w[1], 300.0 / -400.0, 1e-3);
        }
        assert_eq!(world_to_stick([f32::NAN, 0.0], 0.0), [0.0, 0.0]);
        assert_eq!(stick_raw([1.0, -1.0]), (32767, -32767));
        assert_eq!(stick_raw([0.0, 2.0]), (0, 32767));
    }

    /// Falls from `height` onto a strip at the origin with drift +X, the
    /// controller steering at 30 Hz; the plant moves `gain` times as much as
    /// the controller thinks. Returns the player's position and velocity at
    /// touchdown, relative to the aim point.
    fn simulate(
        start: [f32; 2],
        v0: [f32; 2],
        height: f32,
        yaw: f32,
        gain: f32,
    ) -> ([f32; 2], [f32; 2]) {
        const DT: f32 = 1.0 / 30.0;
        let spot = spot_at([0.0, 0.0, 0.0], [1.0, 0.0]);
        let (mut p, mut v) = (start, v0);
        let (mut z, mut vz) = (height, 0.0f32);
        while z > 0.0 {
            let body = Body {
                position: [p[0], p[1], z],
                velocity: [v[0], v[1], vz],
                yaw,
                airborne: true,
            };
            let stick = plan(&[spot], &body).map_or([0.0; 2], |pl| pl.stick);
            let a = stick_to_world(stick, yaw);
            for i in 0..2 {
                v[i] += a[i] * gain * DT;
                p[i] += v[i] * DT;
            }
            vz -= GRAVITY * DT;
            z += vz * DT;
        }
        let aim = [-AIM_BACK, 0.0];
        ([p[0] - aim[0], p[1] - aim[1]], v)
    }

    #[test]
    fn the_controller_lands_a_fall_on_the_aim_point_at_the_arrival_speed() {
        // Each: where it starts (from the aim point), its drift, the drop.
        let cases = [
            ([0.5, 0.3], [0.0, 0.0], 2.4),
            ([-0.8, 0.6], [0.5, 0.0], 2.4),
            ([-0.4, -0.6], [-0.5, 0.3], 2.4),
        ];
        for yaw in [0.0f32, 1.1, -2.5] {
            for (off, v0, h) in cases {
                let start = [off[0] - AIM_BACK, off[1]];
                let miss0 = off[0].hypot(off[1]);
                let (p, v) = simulate(start, v0, h, yaw, 1.0);
                let err = p[0].hypot(p[1]);
                assert!(
                    err < 0.2 && err < miss0 / 2.0,
                    "yaw {yaw} {off:?}: error {err}"
                );
                // Along the drift at the landing speed, little across it.
                assert!(
                    v[0] > 0.2 && v[0] < 1.4,
                    "yaw {yaw} {off:?}: along {}",
                    v[0]
                );
                assert!(v[1].abs() < 0.5, "yaw {yaw} {off:?}: across {}", v[1]);
            }
        }
    }

    #[test]
    fn the_controller_still_lands_close_when_the_stick_is_weaker_or_stronger_than_thought() {
        for gain in [0.7f32, 1.4] {
            let (p, _) = simulate([0.4 - AIM_BACK, 0.3], [0.0; 2], 2.4, 0.5, gain);
            assert!(p[0].hypot(p[1]) < 0.3, "gain {gain}: {p:?}");
        }
    }

    // ---- the button ----

    fn pad_with(x: u8) -> Gamepad {
        let mut pad = Gamepad::default();
        pad.analog[BUTTON.index()] = x;
        pad.thumb_lx = 111;
        pad.thumb_ly = -222;
        pad
    }

    fn a_plan() -> Plan {
        plan(
            &[spot_at([0.0, 0.0, 0.0], [1.0, 0.0])],
            &falling([0.4, 0.2, 2.0], [0.0; 3]),
        )
        .unwrap()
    }

    #[test]
    fn engaged_the_stick_is_the_assists_and_x_is_gone() {
        let p = a_plan();
        let (mut latch, mut pad) = (Latch::default(), pad_with(255));
        assert!(apply(&mut latch, &mut pad, true, Some(&p)));
        assert_eq!((pad.thumb_lx, pad.thumb_ly), stick_raw(p.stick));
        assert_eq!(pad.analog(BUTTON), 0);
        assert!(latch.0);
    }

    #[test]
    fn with_no_strip_in_reach_x_and_the_stick_pass_through() {
        let (mut latch, mut pad) = (Latch::default(), pad_with(255));
        assert!(!apply(&mut latch, &mut pad, true, None));
        assert_eq!(pad, pad_with(255));
        assert!(!latch.0);
    }

    #[test]
    fn not_held_nothing_changes_even_with_a_plan() {
        let p = a_plan();
        let (mut latch, mut pad) = (Latch::default(), pad_with(0));
        assert!(!apply(&mut latch, &mut pad, false, Some(&p)));
        assert_eq!(pad, pad_with(0));
    }

    #[test]
    fn the_press_stays_swallowed_until_it_is_let_go() {
        let p = a_plan();
        let mut latch = Latch::default();
        apply(&mut latch, &mut pad_with(255), true, Some(&p));
        // The strip goes out of reach (the landing) with X still down: the
        // game must not see a press.
        let mut pad = pad_with(255);
        assert!(!apply(&mut latch, &mut pad, true, None));
        assert_eq!(pad.analog(BUTTON), 0);
        assert_eq!((pad.thumb_lx, pad.thumb_ly), (111, -222));
        // Let go, then a new press passes.
        let mut pad = pad_with(0);
        apply(&mut latch, &mut pad, false, None);
        assert!(!latch.0);
        let mut pad = pad_with(255);
        apply(&mut latch, &mut pad, true, None);
        assert_eq!(pad.analog(BUTTON), 255);
    }

    #[test]
    fn a_press_that_began_on_the_ground_passes_until_the_assist_engages() {
        let (mut latch, mut pad) = (Latch::default(), pad_with(255));
        apply(&mut latch, &mut pad, true, None);
        assert_eq!(pad.analog(BUTTON), 255);
        // Engaging later swallows it from then on.
        let p = a_plan();
        let mut pad = pad_with(255);
        apply(&mut latch, &mut pad, true, Some(&p));
        assert_eq!(pad.analog(BUTTON), 0);
    }

    /// Off until the player turns it on, following the setting live; the
    /// reply says which. One test: the switch is process-wide.
    #[test]
    fn the_assist_is_opt_in_follows_the_setting_and_the_reply_says_so() {
        use crate::setting_defs::{ALL, SUPERBOUNCE_FALL_ASSIST};
        let store = settings::Store::defaults(ALL);
        follow_setting(&store);
        assert!(!opted_in(), "off by default");
        assert!(!assist_reply().enabled);
        store
            .set(SUPERBOUNCE_FALL_ASSIST.key, settings::Value::Bool(true))
            .unwrap();
        assert!(opted_in() && assist_reply().enabled);
        store
            .set(SUPERBOUNCE_FALL_ASSIST.key, settings::Value::Bool(false))
            .unwrap();
        assert!(!opted_in());
    }

    #[test]
    fn the_test_hold_is_a_switch() {
        assert!(set_test_hold(true));
        assert!(assist_reply().test_hold);
        assert!(!set_test_hold(false));
        assert!(!assist_reply().test_hold);
    }

    #[test]
    fn the_reply_carries_the_plan_in_wide_numbers() {
        let p = a_plan();
        let r = Report::of(&p);
        assert!(r.engaged);
        assert_eq!(r.target, Some(p.target));
        assert_eq!(r.stick, Some(p.stick));
        assert_eq!(Report::EMPTY, Report::default());
        assert!(no_params(&serde_json::json!({})).is_ok());
        assert!(no_params(&serde_json::json!({"hold": true})).is_err());
    }

    #[test]
    fn the_predicted_landing_says_on_the_slot_fixable_or_a_miss() {
        use superbounce_mode::Course;
        let spots = [spot_at([0.0, 0.0, 0.0], [0.0, 0.0])];
        let on = predict(&spots, &falling([0.0, 0.0, 2.0], [0.0, 0.0, -1.0])).unwrap();
        assert_eq!(on.verdict, Course::OnSlot);
        assert_eq!(on.at, [0.0, 0.0, 0.0]);
        near(on.speed, (1.0 + 2.0 * GRAVITY * 2.0).sqrt(), 1e-5);
        near(on.need, (9.0 + 2.0 * GRAVITY * 1.0).sqrt(), 1e-5);
        // A metre off: the assist can still bring it in.
        let off = predict(&spots, &falling([1.0, 0.0, 2.0], [0.0, 0.0, -1.0])).unwrap();
        assert_eq!(off.verdict, Course::Fixable);
        // Nearly 3 off with a short fall left: it cannot.
        let far = predict(&spots, &falling([2.8, 0.0, 0.3], [0.0, 0.0, -1.0])).unwrap();
        assert_eq!(far.verdict, Course::Miss);
        // Rising, on the ground, under the strip or past its reach: none.
        assert_eq!(
            predict(&spots, &falling([0.0, 0.0, 2.0], [0.0, 0.0, 1.0])),
            None
        );
        let ground = Body {
            airborne: false,
            ..falling([0.0, 0.0, 2.0], [0.0, 0.0, -1.0])
        };
        assert_eq!(predict(&spots, &ground), None);
        assert_eq!(
            predict(&spots, &falling([0.0, 0.0, -1.0], [0.0, 0.0, -1.0])),
            None
        );
        assert_eq!(
            predict(&spots, &falling([5.0, 0.0, 2.0], [0.0, 0.0, -1.0])),
            None
        );
    }

    #[test]
    fn the_crouch_button_is_read_by_the_layouts_index() {
        let mut pad = Gamepad::default();
        assert_eq!(button_down(&pad, DEFAULT_CROUCH_BUTTON), Some(false));
        pad.buttons = input::gamepad::Buttons::LEFT_THUMB;
        assert_eq!(button_down(&pad, DEFAULT_CROUCH_BUTTON), Some(true));
        assert_eq!(button_down(&pad, 15), Some(false));
        pad.analog[AnalogButton::B.index()] = PRESSED;
        assert_eq!(button_down(&pad, 1), Some(true));
        assert_eq!(button_down(&pad, 16), None);
        assert_eq!(crouch_button(None), DEFAULT_CROUCH_BUTTON);
        publish_crouch(Some(true));
        assert_eq!(crouch_held(), Some(true));
        publish_crouch(None);
        assert_eq!(crouch_held(), None);
    }
}
