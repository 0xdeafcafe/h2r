//! The iOS touch controls' game side (docs/runtime/ios.md, "Controls" and
//! "Layouts"): once a tick, while the touch controls show,
//!
//! 1. **Direct look.** The turn the touch host queued (`input::touch_look`)
//!    is added to player one's facing in the player-control record, divided
//!    by the zoom magnification and slowed while the reticle is red. The
//!    game's stick path (deadzone, response curve, maximum rate) is not
//!    involved, so a drag turns the camera by an angle.
//! 2. **Context.** Player one's seat, held weapon and target are read into
//!    an `input::touch_layout::Context` and published, for the layout the
//!    host draws and for Touch Auto Fire. A change of layout is logged once.
//!
//! **Player control** (static reading of the retail XBE; the record base is
//! `[0x4ed284] + 0x14 + 0x94 * i`, called `E` here):
//!
//! | Field | What | Evidence |
//! |---|---|---|
//! | `E+0x00` | the unit datum | `0x187130`; `fov_row` reads it |
//! | `E+0x10`, `E+0x14` | yaw, pitch (radians; yaw kept in `[0, 2pi)`) | `0x187560` adds the frame's yaw delta, `0x1879e5` the pitch delta and clamps it; `0x186ab0` copies both into the player's action (`0x18701d`) |
//! | `E+0x44` | the aim-assist target object, or none | written by `0x1a4900` through `0x186f1e` |
//! | `E+0x60`, `E+0x64` | aim-assist strengths (cone, pull) | `0x187420` returns their max |
//! | `E+0x80`, `E+0x84` | the live pitch limits | `0x187973` to `0x1879dd` |
//!
//! The main loop runs `0x1870a0` (each local player's look: stick to rate,
//! times the frame time, added to yaw and pitch) once a frame before the
//! ticks; a facing written after `game_tick` is what the next frame adds
//! to and what the next action carries. That integration is skipped while
//! byte `0x4e8c99 + 0x140 * player` is set (scripted control); then the
//! queued turn is dropped too.
//!
//! **Unit, seat and weapon** (docs/live/live-gamestate.md): the unit body's
//! parent (`+0x14`) is its vehicle and `+0x1fc` its seat index; seat flags
//! are a u32 at `+0` of the vehicle definition's seats block (`+0x1c8`
//! count, `+0x1cc` address, stride `0xb0`), bit 2 the driver (`0x18728b`;
//! warthog, ghost, banshee, tank drivers have it, passengers and gunners
//! never). The held weapon is `+0x218 + 4 * (i8)[+0x212]`; `+0x213` is the
//! left-hand weapon's index when dual-wielding (`0x186bd4`). In the weapon
//! definition: zoom levels `+0x1fe` (i16; sniper and beam rifle 2, battle
//! rifle, carbine and rocket launcher 1, others 0), the triggers block
//! (`+0x2c8`, stride `0x40`, none on the energy sword) with the behaviour
//! at `+6` (u16: 0 spews while held: SMG, plasma rifle, needler; 1, 4 and 5
//! fire once per pull: magnum, shotgun, sniper, battle rifle, carbine,
//! rocket launcher; 2 and 3 charge: plasma pistol), and the barrels block
//! (`+0x2d0`, stride `0xec`) with the maximum rounds a second at `+8` and
//! the time between shots at `+0x20`. Offsets and values were checked
//! against Zanzibar's weapon tags.

use std::sync::Arc;
use std::time::Instant;

use input::touch_layout::{Context, Seat, Trigger};

use crate::layout_glue::Gm;

/// The feature id in the plan and `H2_FEATURES`.
pub const FEATURE: &str = "touch-glue";

/// The player-control array pointer; elements start `0x14` in.
pub use engine::hs_actions::PLAYER_CONTROL;
pub const PC_HEADER: u32 = 0x14;
pub const PC_STRIDE: u32 = 0x94;
pub const PC_UNIT: u32 = 0x00;
pub const PC_YAW: u32 = 0x10;
pub const PC_PITCH: u32 = 0x14;
pub const PC_TARGET: u32 = 0x44;
pub const PC_CONE: u32 = 0x60;
pub const PC_PULL: u32 = 0x64;
pub const PC_PITCH_MIN: u32 = 0x80;
pub const PC_PITCH_MAX: u32 = 0x84;
/// Per local player (stride `0x140`): set, the game does not integrate look.
pub const LOOK_LOCKED: u32 = 0x004e_8c99;
pub const LOOK_LOCKED_STRIDE: u32 = 0x140;
/// The engine's own pitch bound (`[0x45e050]`, about 85.5 degrees).
pub const PITCH_LIMIT: f32 = 1.492_256_5;

/// The object header table pointer: data at `+0x44`, 12-byte headers (salt
/// `+0`, type `+3`, body `+8`).
const OBJECT_HEADERS: u32 = 0x004e_0300;
/// The tag instance table pointer: 16-byte entries, definition at `+8`.
const TAG_INSTANCES: u32 = 0x004e_3b44;

const BODY_PARENT: u32 = 0x14;
const UNIT_SEAT: u32 = 0x1fc;
const UNIT_WEAPON: u32 = 0x212;
const UNIT_LEFT_WEAPON: u32 = 0x213;
const UNIT_WEAPONS: u32 = 0x218;
const SEATS: u32 = 0x1c8;
const SEAT_STRIDE: u32 = 0xb0;
const SEAT_DRIVER: u32 = 1 << 2;
const WEAP_ZOOM_LEVELS: u32 = 0x1fe;
const WEAP_TRIGGERS: u32 = 0x2c8;
const TRIGGER_BEHAVIOUR: u32 = 6;
const WEAP_BARRELS: u32 = 0x2d0;
const BARREL_RATE_MAX: u32 = 8;
const BARREL_RECOVERY: u32 = 0x20;

const NONE: u32 = u32::MAX;
const BIPED: u8 = 0;
const VEHICLE: u8 = 1;
const WEAPON: u8 = 2;

fn f32_at<M: Gm>(m: &M, a: u32) -> Option<f32> {
    m.u32(a).map(f32::from_bits)
}

/// Player-control record `i`'s address.
pub fn record<M: Gm>(m: &M, i: u32) -> Option<u32> {
    let base = m.u32(PLAYER_CONTROL)?;
    (base != 0).then(|| base.wrapping_add(PC_HEADER + PC_STRIDE * i))
}

/// The live object `datum` names: its type and body address.
fn object<M: Gm>(m: &M, datum: u32) -> Option<(u8, u32)> {
    if datum == NONE || datum == 0 {
        return None;
    }
    let headers = m.u32(m.u32(OBJECT_HEADERS)?.wrapping_add(0x44))?;
    let header = headers.wrapping_add((datum & 0xffff) * 12);
    if u32::from(m.u16(header)?) != datum >> 16 {
        return None;
    }
    let body = m.u32(header.wrapping_add(8))?;
    (body != 0).then_some((m.u8(header.wrapping_add(3))?, body))
}

/// The definition of the object at `body`.
fn definition<M: Gm>(m: &M, body: u32) -> Option<u32> {
    let tag = m.u32(body)? & 0xffff;
    let def = m.u32(m.u32(TAG_INSTANCES)?.wrapping_add(tag * 16 + 8))?;
    (def != 0).then_some(def)
}

/// The `i`th element (of `stride` bytes) of the tag block at `def + block`.
fn element<M: Gm>(m: &M, def: u32, block: u32, i: u32, stride: u32) -> Option<u32> {
    let count = m.u32(def.wrapping_add(block))?;
    if i >= count || count > 64 {
        return None;
    }
    let at = m.u32(def.wrapping_add(block + 4))?;
    (at != 0).then(|| at.wrapping_add(i * stride))
}

/// The weapon's trigger, from its definition.
pub fn trigger<M: Gm>(m: &M, def: u32) -> Trigger {
    if m.u32(def.wrapping_add(WEAP_TRIGGERS)) == Some(0) {
        return Trigger::Melee;
    }
    let Some(t) = element(m, def, WEAP_TRIGGERS, 0, 0x40) else {
        return Trigger::Unknown;
    };
    let rate = || -> f32 {
        let Some(b) = element(m, def, WEAP_BARRELS, 0, 0xec) else {
            return 0.0;
        };
        let recovery = f32_at(m, b.wrapping_add(BARREL_RECOVERY)).unwrap_or(0.0);
        if recovery.is_finite() && recovery > 0.01 {
            return 1.0 / recovery;
        }
        let max = f32_at(m, b.wrapping_add(BARREL_RATE_MAX)).unwrap_or(0.0);
        if max.is_finite() && max > 0.0 {
            max
        } else {
            0.0
        }
    };
    match m.u16(t.wrapping_add(TRIGGER_BEHAVIOUR)) {
        Some(0) => Trigger::Automatic,
        Some(1 | 4 | 5) => Trigger::SemiAuto { rate: rate() },
        Some(2 | 3) => Trigger::Charge { rate: rate() },
        _ => Trigger::Unknown,
    }
}

/// Player `i`'s state for the touch layout. [`Seat::None`] when there is no
/// game or no live unit.
pub fn context<M: Gm>(m: &M, i: u32) -> Context {
    let mut ctx = Context::default();
    let Some(pc) = record(m, i) else { return ctx };
    let Some(unit_datum) = m.u32(pc.wrapping_add(PC_UNIT)) else {
        return ctx;
    };
    let Some((kind, unit)) = object(m, unit_datum) else {
        return ctx;
    };
    if kind != BIPED && kind != VEHICLE {
        return ctx;
    }
    ctx.seat = Seat::OnFoot;
    let parent = m.u32(unit.wrapping_add(BODY_PARENT)).unwrap_or(NONE);
    if let Some((VEHICLE, vehicle)) = object(m, parent) {
        let seat = m.u16(unit.wrapping_add(UNIT_SEAT)).map_or(-1, |s| s as i16);
        let flags = (seat >= 0)
            .then(|| definition(m, vehicle))
            .flatten()
            .and_then(|def| element(m, def, SEATS, seat as u32, SEAT_STRIDE))
            .and_then(|s| m.u32(s));
        ctx.seat = match flags {
            Some(f) if f & SEAT_DRIVER != 0 => Seat::Driver,
            _ => Seat::Passenger,
        };
    }
    let weapon_def = |index: u32| -> Option<u32> {
        let idx = m.u8(unit.wrapping_add(index))? as i8;
        if !(0..4).contains(&idx) {
            return None;
        }
        let datum = m.u32(unit.wrapping_add(UNIT_WEAPONS + 4 * idx as u32))?;
        match object(m, datum)? {
            (WEAPON, body) => definition(m, body),
            _ => None,
        }
    };
    if let Some(def) = weapon_def(UNIT_WEAPON) {
        ctx.zoom = m
            .u16(def.wrapping_add(WEAP_ZOOM_LEVELS))
            .is_some_and(|z| (z as i16) > 0);
        ctx.trigger = trigger(m, def);
    } else {
        ctx.trigger = Trigger::Melee; // nothing to shoot
    }
    ctx.dual = m
        .u8(unit.wrapping_add(UNIT_LEFT_WEAPON))
        .is_some_and(|i| (0..4).contains(&(i as i8)));
    ctx.on_target = on_target(m, pc);
    ctx
}

/// The crosshair is on an aim-assist target: a target object and a
/// positive aim cone or pull strength (what the red reticle and the
/// game's own friction follow; the target search only keeps targets aim
/// assist applies to).
pub fn on_target<M: Gm>(m: &M, pc: u32) -> bool {
    let target = m.u32(pc.wrapping_add(PC_TARGET)).unwrap_or(NONE);
    if target == NONE || target == 0 {
        return false;
    }
    let s = |o: u32| {
        f32_at(m, pc.wrapping_add(o))
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
    };
    s(PC_CONE).max(s(PC_PULL)) > 0.0
}

/// Adds `turn` (radians, +x right, +y up) to player-control record `pc`'s
/// facing: yaw kept in `[0, 2pi)`, pitch inside the record's live limits
/// (the engine's own bound when they are unreadable). False when nothing
/// could be written.
pub fn turn<M: Gm>(m: &mut M, pc: u32, turn: (f64, f64)) -> bool {
    let (Some(yaw), Some(pitch)) = (
        f32_at(m, pc.wrapping_add(PC_YAW)),
        f32_at(m, pc.wrapping_add(PC_PITCH)),
    ) else {
        return false;
    };
    if !(yaw.is_finite() && pitch.is_finite()) {
        return false;
    }
    let tau = std::f64::consts::TAU;
    // Halo's yaw grows turning left (counter-clockwise, Z up).
    let y = (f64::from(yaw) - turn.0).rem_euclid(tau);
    let y = if y >= tau { 0.0 } else { y };
    let limit = |o: u32, d: f32| {
        f32_at(m, pc.wrapping_add(o))
            .filter(|v| v.is_finite() && v.abs() <= PITCH_LIMIT + 1e-3)
            .unwrap_or(d)
    };
    let (lo, hi) = (
        limit(PC_PITCH_MIN, -PITCH_LIMIT),
        limit(PC_PITCH_MAX, PITCH_LIMIT),
    );
    let (lo, hi) = if lo <= hi {
        (lo, hi)
    } else {
        (-PITCH_LIMIT, PITCH_LIMIT)
    };
    let p = (f64::from(pitch) + turn.1).clamp(f64::from(lo), f64::from(hi));
    m.wr(pc.wrapping_add(PC_YAW), &(y as f32).to_le_bytes())
        && m.wr(pc.wrapping_add(PC_PITCH), &(p as f32).to_le_bytes())
}

/// One tick's work for player one (record 0): publish the context, and
/// apply `pending` (the queued turn) unless the game holds look. Returns
/// the layout change to log, if any.
pub fn tick<M: Gm>(m: &mut M, pending: (f64, f64), zoom: f32) -> Option<String> {
    let ctx = context(m, 0);
    let change = input::touch_layout::publish(ctx);
    if ctx.seat != Seat::None
        && pending != (0.0, 0.0)
        && m.u8(LOOK_LOCKED) == Some(0)
        && let Some(pc) = record(m, 0)
    {
        turn(
            m,
            pc,
            input::touch_look::applied(pending, zoom, ctx.on_target),
        );
    }
    change.map(|(from, to)| format!("touch: layout {} -> {}", from.name(), to.name()))
}

/// Why `touch-glue` can't be snapshotted now: on-screen controls are
/// shown, so touch input not yet applied (and the layout it follows) is
/// host state. With no controls shown the hook does nothing.
pub fn touch_live() -> Option<String> {
    input::touch::controls_shown().then(|| "touch controls are shown".into())
}

/// The tick hook.
pub struct TouchGlue;

impl feature::Feature for TouchGlue {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(FEATURE, "Touch look and layouts").doc("docs/runtime/ios.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        use feature::points::GAME_TICK;
        b.raw()
            .after(&GAME_TICK, "touch look and layout", move |c| {
                if !input::touch::controls_shown() {
                    return Ok(());
                }
                // Taken before guest memory: the host thread adds under the
                // same lock.
                let pending = input::touch_look::take(Instant::now());
                let zoom = input::touch_look::zoom();
                let rt = Arc::clone(c.ctx.runtime());
                let line = tick(&mut *rt.memory(), pending, zoom);
                if let Some(line) = line {
                    eprintln!("{line}");
                }
                Ok(())
            })
            // Our skulls hook the same tick; neither reads what the other
            // writes, so any fixed order will do. The Halo 1 weapon HUD's
            // sample runs outside, after every tick-end write.
            .after_feature(crate::skulls_custom::FEATURE)
            .after_feature(crate::h1_hud_text::FEATURE);
        b.snapshot(feature::SnapshotPolicy::forbid_while(touch_live));
        Ok(())
    }
}

/// Adds the touch glue to `registry`; touch look goes direct from then on.
pub fn install(registry: &mut feature::Registry) -> anyhow::Result<bool> {
    let added = registry
        .add(&TouchGlue)
        .map_err(|e| anyhow::anyhow!("touch glue: {e}"))?;
    input::touch_look::set_consumer(added);
    if !added {
        eprintln!("touch glue: off (H2_FEATURES); touch look is a stick");
    }
    Ok(added)
}

#[cfg(test)]
#[path = "touch_glue_tests.rs"]
mod tests;
