//! Fly and noclip: flight with the player's own controls (the cheats
//! module doc has the controls and the research points).
//!
//! Both read what the game's unit control (`0xc6de0`, the copy at
//! `0xc6e02..0xc6e9a`) left in the unit after the tick: the control flags,
//! the throttle and the aiming vector. Fly sets the body's velocity and
//! leaves the move to the game's physics, collision and all, the way our
//! Acrophobia skull does. Noclip holds a position of its own and writes it
//! over whatever the physics did, so walls never stop it. Turning noclip
//! off hands the body back to the physics where it is, then watches it for
//! [`WATCH_SECONDS`]: shielded from damage, and put back on the last floor
//! the physics stood it on if it is stuck or falls far.

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::view::objects;

use crate::skulls_custom::{
    BIPED_MODE, BODY_PARENT, BODY_VELOCITY, FLY_SPEED, LocalUnit, MODE_GROUND,
};

/// Unit: the control flags (u32), copied from the unit control's `+0x10`
/// at `0xc6e5d`. Bit 0 crouch (the biped's crouch request reads it at
/// `0xe28b1`), bit 1 jump (the biped's jump test reads it at `0xe469c`).
pub const UNIT_CONTROL_FLAGS: u32 = 0x148;
pub const CONTROL_CROUCH: u32 = 1 << 0;
pub const CONTROL_JUMP: u32 = 1 << 1;
/// Unit: the throttle (three floats: forward, left, up; -1 to 1), copied
/// from the unit control's `+0x14` at `0xc6e02`.
pub const UNIT_THROTTLE: u32 = 0x1b0;
/// Unit: the aiming vector (three floats, world space), copied from the
/// unit control's `+0x34` at `0xc6e87`.
pub const UNIT_AIMING: u32 = 0x15c;
/// Object body: the position (three floats), as object_set_position
/// (`0xb75a0`, through `0xb7430` at `0xb74c4`) stores it.
pub const BODY_POSITION: u32 = 0x64;

// ---- tuning (ours) ----

/// Flight speed with the stick pushed all the way, world units a second
/// (a biped runs at about 2.3).
pub const CRUISE: f32 = 5.0;
/// The speed boost: holding the stick past [`BOOST_STICK`] for
/// [`BOOST_AFTER`] seconds ramps the speed up to this many times
/// [`CRUISE`] over [`BOOST_RAMP`] seconds.
pub const BOOST: f32 = 4.0;
pub const BOOST_STICK: f32 = 0.9;
pub const BOOST_AFTER: f32 = 0.5;
pub const BOOST_RAMP: f32 = 1.0;
/// Climbing (jump) and sinking (crouch) speed: Acrophobia's.
pub const VERTICAL: f32 = FLY_SPEED;
/// Fly's take-off: the lift off the floor (world units) and the climb
/// speed it starts with.
pub const TAKEOFF_LIFT: f32 = 0.1;
/// After noclip is off: how long the body is watched and shielded.
pub const WATCH_SECONDS: f32 = 3.0;
/// Stuck: moved less than this (world units) a tick ...
pub const STILL: f32 = 0.001;
/// ... for this long, while airborne or pushing the stick.
pub const STUCK_SECONDS: f32 = 0.5;
/// Fallen: this far (world units) below where noclip let go.
pub const FALL_RESCUE: f32 = 20.0;

/// What the player is asking for this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Controls {
    pub crouch: bool,
    pub jump: bool,
    pub throttle: [f32; 3],
    pub aim: [f32; 3],
}

impl Controls {
    pub fn read(m: &dyn GuestMemory, body: u32) -> Result<Self, MemFault> {
        let flags = m.u32(body.wrapping_add(UNIT_CONTROL_FLAGS))?;
        Ok(Controls {
            crouch: flags & CONTROL_CROUCH != 0,
            jump: flags & CONTROL_JUMP != 0,
            throttle: m.vec3(body.wrapping_add(UNIT_THROTTLE))?,
            aim: m.vec3(body.wrapping_add(UNIT_AIMING))?,
        })
    }

    /// How far the move stick is pushed (0 to 1).
    pub fn stick(&self) -> f32 {
        let [x, y, _] = self.throttle.map(clean);
        (x * x + y * y).sqrt().min(1.0)
    }
}

fn clean(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn normalized(v: [f32; 3]) -> Option<[f32; 3]> {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (l.is_finite() && l > 1e-4).then(|| v.map(|c| c / l))
}

/// The boost factor after the stick has been held past [`BOOST_STICK`]
/// for `held` seconds.
pub fn boost(held: f32) -> f32 {
    let t = ((held - BOOST_AFTER) / BOOST_RAMP).clamp(0.0, 1.0);
    1.0 + (BOOST - 1.0) * t
}

/// The velocity flight asks for: the stick moves along the view (forward
/// is where the player looks, up and down included) and sideways level;
/// jump climbs, crouch sinks.
pub fn velocity(c: &Controls, boost: f32) -> [f32; 3] {
    let fwd =
        normalized(c.aim.map(|v| if v.is_finite() { v } else { 0.0 })).unwrap_or([1.0, 0.0, 0.0]);
    // Left, level: the view's heading turned a quarter to the left.
    let left = normalized([-fwd[1], fwd[0], 0.0]).unwrap_or([0.0, 1.0, 0.0]);
    let [x, y, _] = c.throttle.map(clean);
    let s = CRUISE * boost;
    let mut v = [0.0; 3];
    for i in 0..3 {
        v[i] = (fwd[i] * x + left[i] * y) * s;
    }
    v[2] += VERTICAL * (f32::from(u8::from(c.jump)) - f32::from(u8::from(c.crouch)));
    v
}

/// After noclip let go: the watch over the body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Watch {
    /// Seconds left.
    pub left: f32,
    /// Where noclip let go.
    pub from: [f32; 3],
    /// Last tick's position.
    pub last: [f32; 3],
    /// Seconds without moving.
    pub still: f32,
    /// Whether it may put the body back (noclip let go, not fly).
    pub rescue: bool,
}

impl Watch {
    pub fn new(at: [f32; 3], rescue: bool) -> Self {
        Watch {
            left: WATCH_SECONDS,
            from: at,
            last: at,
            still: 0.0,
            rescue,
        }
    }

    /// One tick at `pos`: whether the body is stuck or has fallen and
    /// should go back to a floor.
    pub fn step(&mut self, pos: [f32; 3], grounded: bool, pushing: bool, dt: f32) -> bool {
        self.left -= dt;
        let d = (0..3)
            .map(|i| (pos[i] - self.last[i]).powi(2))
            .sum::<f32>()
            .sqrt();
        self.last = pos;
        if d < STILL && (!grounded || pushing) {
            self.still += dt;
        } else {
            self.still = 0.0;
        }
        self.rescue && (self.still >= STUCK_SECONDS || pos[2] < self.from[2] - FALL_RESCUE)
    }
}

/// One local unit's flight.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pilot {
    /// Fly: in the air on our velocity.
    pub flying: bool,
    /// Fly: has left the ground since taking off (landing needs it).
    pub lifted: bool,
    /// Crouch was held last tick (take-off is a fresh press).
    pub crouch_was: bool,
    /// Seconds the stick has been held past [`BOOST_STICK`].
    pub held: f32,
    /// Noclip: the position we hold.
    pub at: Option<[f32; 3]>,
    /// The last floor the physics stood the body on (or where flight
    /// began): where a stuck or falling body goes back to.
    pub safe: Option<[f32; 3]>,
    pub watch: Option<Watch>,
}

impl Pilot {
    /// Whether the body should be shielded from damage now.
    pub fn shielded(&self) -> bool {
        self.at.is_some() || self.watch.is_some()
    }

    /// Whether there is anything left to do with flight off.
    pub fn idle(&self) -> bool {
        !self.flying && self.at.is_none() && self.watch.is_none()
    }
}

fn add(a: [f32; 3], v: [f32; 3], dt: f32) -> [f32; 3] {
    [a[0] + v[0] * dt, a[1] + v[1] * dt, a[2] + v[2] * dt]
}

fn set_vec3(m: &mut dyn GuestMemory, at: u32, v: [f32; 3]) -> Result<(), MemFault> {
    for (i, c) in v.iter().enumerate() {
        m.set_f32(at.wrapping_add(4 * i as u32), *c)?;
    }
    Ok(())
}

/// Whether flight can act on this unit: a biped on foot (not in a
/// vehicle or seat).
pub fn on_foot(m: &dyn GuestMemory, u: &LocalUnit) -> Result<bool, MemFault> {
    Ok(u.kind == objects::BIPED && m.u32(u.body.wrapping_add(BODY_PARENT))? == u32::MAX)
}

/// One tick of fly and noclip for one local biped on foot, after the
/// game's tick. `dt` is the tick's seconds. Says what happened, if
/// anything worth a line.
pub fn step(
    m: &mut dyn GuestMemory,
    u: &LocalUnit,
    p: &mut Pilot,
    fly: bool,
    noclip: bool,
    dt: f32,
) -> Result<Option<String>, MemFault> {
    let b = u.body;
    let c = Controls::read(m, b)?;
    let grounded = m.u8(b.wrapping_add(BIPED_MODE))? == MODE_GROUND;
    let pos = m.vec3(b.wrapping_add(BODY_POSITION))?;
    let press = c.crouch && !p.crouch_was;
    p.crouch_was = c.crouch;
    p.held = if c.stick() >= BOOST_STICK {
        p.held + dt
    } else {
        0.0
    };
    let v = velocity(&c, boost(p.held));
    let vel = b.wrapping_add(BODY_VELOCITY);
    let mut said = None;
    if grounded && pos.iter().all(|c| c.is_finite()) {
        p.safe = Some(pos);
    }
    if noclip {
        let at = *p.at.get_or_insert(pos);
        if p.safe.is_none() {
            p.safe = Some(at);
        }
        p.watch = None;
        let next = add(at, v, dt);
        let at = if next.iter().all(|c| c.is_finite()) {
            next
        } else {
            at
        };
        p.at = Some(at);
        set_vec3(m, b.wrapping_add(BODY_POSITION), at)?;
        set_vec3(m, vel, [0.0; 3])?;
        // Back with fly on: keep flying from here.
        p.flying = fly;
        p.lifted = fly;
        return Ok(None);
    }
    if let Some(at) = p.at.take() {
        p.watch = Some(Watch::new(at, true));
        said = Some(format!(
            "cheats: noclip off for player {}: back to the physics at ({:.2}, {:.2}, {:.2})",
            u.user, at[0], at[1], at[2]
        ));
    }
    if fly {
        if !p.flying {
            if press && grounded {
                p.flying = true;
                p.lifted = false;
                p.safe = Some(pos);
                let up = [pos[0], pos[1], pos[2] + TAKEOFF_LIFT];
                set_vec3(m, b.wrapping_add(BODY_POSITION), up)?;
                set_vec3(m, vel, [0.0, 0.0, VERTICAL])?;
            }
        } else if !grounded {
            p.lifted = true;
            set_vec3(m, vel, v)?;
        } else if p.lifted && c.crouch {
            // Down onto a floor with crouch held: landed.
            p.flying = false;
        } else {
            set_vec3(m, vel, v)?;
        }
    } else if p.flying {
        p.flying = false;
        if !grounded && p.watch.is_none() {
            // Dropped from the air: shielded for the fall, no rescue.
            p.watch = Some(Watch::new(pos, false));
        }
    }
    if let Some(w) = &mut p.watch {
        let pushing = c.stick() > 0.5;
        if w.step(pos, grounded, pushing, dt) {
            if let Some(safe) = p.safe {
                set_vec3(m, b.wrapping_add(BODY_POSITION), safe)?;
                set_vec3(m, vel, [0.0; 3])?;
                said = Some(format!(
                    "cheats: player {} was stuck or falling after noclip: put back at ({:.2}, {:.2}, {:.2})",
                    u.user, safe[0], safe[1], safe[2]
                ));
            }
            w.rescue = false;
            w.last = p.safe.unwrap_or(pos);
        }
        if w.left <= 0.0 {
            p.watch = None;
        }
    }
    Ok(said)
}
