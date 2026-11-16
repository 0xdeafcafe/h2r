//! The Halo 1 health model on Halo 2's engine (docs/game/health.md).
//!
//! Halo 2 regenerates a player's body vitality once the body's stun timer
//! runs out; Halo 1 does not, and has health packs instead. For a Halo 1
//! level (campaign, or a Halo 1 map in multiplayer) the game runs Halo 1's
//! model; everywhere else it is retail's.
//!
//! - **No regeneration.** Around the object damage update
//!   ([`OBJECT_DAMAGE_UPDATE`](feature::points::OBJECT_DAMAGE_UPDATE),
//!   `0xd5de0`, once per object per tick): `before` notes a player biped's
//!   body health and its "body recharging" flag, `after` puts them back if
//!   the update raised the health. That update's only health write that
//!   raises it is the regeneration (`0xd63de`..`0xd64b5`); damage, kills,
//!   scripts, checkpoints and health packs write it elsewhere and are left
//!   alone.
//! - **Health packs** need no hook: Halo 2's powerup pickup (`0x151ea0`)
//!   already has a health case (equipment powerup type 5), which calls
//!   [`BODY_RESTORE`](feature::points::BODY_RESTORE) (`0xd6790`, body to
//!   full; refused when full or dead). An observer logs each use.
//! - **HUD.** A converted Halo 1 map's HUD draws the meter itself (a widget
//!   on HUD input 17, the unit's body). For a map whose HUD does not
//!   (a retail level under the debug switch), the window draws one over
//!   the game: [`overlay_meter`] is what it shows.
//!
//! **The switch.** [`set_mode`] is driven by the map's identity (the
//! campaigns and h1-mp lanes call it at each game start). The debug
//! override `H2_HEALTH_MODEL=halo1|halo2` beats it, so the model can be
//! tried on a retail level.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::view::objects;

/// The feature's id (`features.h1-health`).
pub const FEATURE: &str = "h1-health";

/// The debug override's environment variable.
pub const ENV: &str = "H2_HEALTH_MODEL";

/// Whose health model the game runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HealthModel {
    /// Retail: the body regenerates after the shield.
    Halo2,
    /// The body does not regenerate; health packs refill it; a health
    /// meter under the shield meter.
    Halo1,
}

impl HealthModel {
    /// `halo1` / `halo2` (also `h1`, `h2`, `1`, `2`), any case.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "halo1" | "h1" | "1" => Some(Self::Halo1),
            "halo2" | "h2" | "2" => Some(Self::Halo2),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Halo1 => "Halo 1",
            Self::Halo2 => "Halo 2",
        }
    }
    fn code(self) -> u8 {
        match self {
            Self::Halo2 => 1,
            Self::Halo1 => 2,
        }
    }
    fn from_code(c: u8) -> Option<Self> {
        match c {
            1 => Some(Self::Halo2),
            2 => Some(Self::Halo1),
            _ => None,
        }
    }
}

// ---- the switch ----

/// The model the loaded map asks for ([`set_mode`]); 0 means not set
/// (Halo 2).
static MAP_MODE: AtomicU8 = AtomicU8::new(0);
/// The debug override (`H2_HEALTH_MODEL`); 0 means none.
static FORCED: AtomicU8 = AtomicU8::new(0);
/// The loaded map's HUD draws the health meter itself.
static HUD_HAS_METER: AtomicBool = AtomicBool::new(false);
/// The health meter to draw: `f32` bits, NaN for none.
static METER: AtomicU32 = AtomicU32::new(NO_METER);
const NO_METER: u32 = 0x7fc0_0000;

/// Sets the model for the game being started, from the map's identity: a
/// Halo 1 map is [`HealthModel::Halo1`], anything else
/// [`HealthModel::Halo2`]. It stays until set again, so call it at every
/// game start, not only for Halo 1 maps. Safe from any thread; the change
/// is seen by the next object update.
pub fn set_mode(m: HealthModel) {
    MAP_MODE.store(m.code(), Ordering::SeqCst);
}

/// Whether the loaded map's own HUD has the health meter (a converted
/// Halo 1 map whose HUD has a widget on the body input). While it does,
/// the window's meter is not drawn. Set it with [`set_mode`].
pub fn set_hud_has_meter(on: bool) {
    HUD_HAS_METER.store(on, Ordering::SeqCst);
}

/// The model in force: the debug override if set, else the map's.
pub fn mode() -> HealthModel {
    HealthModel::from_code(FORCED.load(Ordering::Relaxed))
        .or_else(|| HealthModel::from_code(MAP_MODE.load(Ordering::Relaxed)))
        .unwrap_or(HealthModel::Halo2)
}

/// Whether [`mode`] comes from the debug override.
pub fn forced() -> Option<HealthModel> {
    HealthModel::from_code(FORCED.load(Ordering::Relaxed))
}

/// Sets or clears the debug override (what `H2_HEALTH_MODEL` does).
pub fn force(m: Option<HealthModel>) {
    FORCED.store(m.map_or(0, HealthModel::code), Ordering::SeqCst);
}

/// Reads the debug override's value: unset or empty is none; anything
/// else must name a model.
pub fn parse_override(v: Option<&str>) -> Result<Option<HealthModel>, String> {
    match v.map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) => HealthModel::parse(s)
            .map(Some)
            .ok_or_else(|| format!("{ENV}={s:?} is neither halo1 nor halo2")),
    }
}

/// The local player's health fraction for the window's meter: `Some` only
/// in the Halo 1 model, with a local player alive, and when the map's HUD
/// does not draw the meter itself.
pub fn overlay_meter() -> Option<f32> {
    let v = f32::from_bits(METER.load(Ordering::Relaxed));
    (!v.is_nan()).then_some(v)
}

fn publish_meter(v: Option<f32>) {
    METER.store(v.map_or(NO_METER, f32::to_bits), Ordering::Relaxed);
}

// ---- the guest layout (docs/game/health.md "Layouts") ----

/// Object body: health (body vitality) and shield, fractions (1.0 full),
/// as `object_get_health` (`0x2a1a20`) and `object_get_shield`
/// (`0x2a1a90`) read them.
pub const BODY_HEALTH: u32 = 0xec;
pub const BODY_SHIELD: u32 = 0xf0;
/// The high byte of the damage flags word (`+0x10a`): bit 0 is "body
/// recharging", set by the regeneration (`0xd6465`) and cleared when it
/// reaches its target (`0xd648d`).
pub const BODY_FLAGS_HIGH: u32 = 0x10b;
pub const BODY_RECHARGING: u8 = 0x01;
/// Unit: the controlling player's datum (none for AI and empty units).
pub const UNIT_PLAYER: u32 = 0x13c;

/// A player biped's health as noted before the damage update.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub body: u32,
    pub health: f32,
    pub flags_high: u8,
}

/// The body of `object` if it is a biped a player controls, with its
/// health now. AI, vehicles, weapons and everything else: `None`.
pub fn sample(m: &dyn GuestMemory, object: u32) -> Result<Option<Sample>, MemFault> {
    let Some(handle) = DatumHandle::from_raw(object) else {
        return Ok(None);
    };
    let Some((kind, body)) = objects::get(m, handle)? else {
        return Ok(None);
    };
    if kind != objects::BIPED || m.u32(body.addr.wrapping_add(UNIT_PLAYER))? == u32::MAX {
        return Ok(None);
    }
    Ok(Some(Sample {
        body: body.addr,
        health: m.f32(body.addr.wrapping_add(BODY_HEALTH))?,
        flags_high: m.u8(body.addr.wrapping_add(BODY_FLAGS_HIGH))?,
    }))
}

/// The rule: under Halo 1's model, a damage update that raised a player's
/// health is undone (the health and the recharging bit as they were).
/// `None` leaves the update's result.
pub fn hold(model: HealthModel, before: Sample, health: f32, flags_high: u8) -> Option<(f32, u8)> {
    (model == HealthModel::Halo1 && health > before.health).then(|| {
        (
            before.health,
            (flags_high & !BODY_RECHARGING) | (before.flags_high & BODY_RECHARGING),
        )
    })
}

/// [`hold`] against the body as the update left it; the health it held
/// back from, when it did.
pub fn apply_hold(m: &mut dyn GuestMemory, model: HealthModel, before: Sample) -> Result<Option<f32>, MemFault> {
    let health = m.f32(before.body.wrapping_add(BODY_HEALTH))?;
    let flags = m.u8(before.body.wrapping_add(BODY_FLAGS_HIGH))?;
    let Some((h, f)) = hold(model, before, health, flags) else {
        return Ok(None);
    };
    m.set_f32(before.body.wrapping_add(BODY_HEALTH), h)?;
    m.set_u8(before.body.wrapping_add(BODY_FLAGS_HIGH), f)?;
    Ok(Some(health))
}

/// Local user 0's unit's health (for the window's meter), if it has one
/// alive.
pub fn local_health(m: &dyn GuestMemory) -> Result<Option<f32>, MemFault> {
    let units = crate::skulls_custom::local_units(m)?;
    let Some(u) = units.iter().find(|u| u.user == 0).or(units.first()) else {
        return Ok(None);
    };
    let flags = m.u16(u.body.wrapping_add(0x10a))?;
    if flags & 0x4 != 0 {
        // Body depleted: dead.
        return Ok(None);
    }
    Ok(Some(m.f32(u.body.wrapping_add(BODY_HEALTH))?.clamp(0.0, 1.0)))
}

/// What the meter shows now, from the model and the guest.
pub fn meter_now(m: &dyn GuestMemory) -> Option<f32> {
    if mode() != HealthModel::Halo1 || HUD_HAS_METER.load(Ordering::Relaxed) {
        return None;
    }
    local_health(m).ok().flatten()
}

// ---- the feature ----

#[derive(Default)]
struct State {
    /// The update in progress (it does not recurse).
    pending: Mutex<Option<Sample>>,
    armed: AtomicBool,
    /// A restore in progress: (object datum, health before).
    restore: Mutex<Option<(u32, Option<f32>)>>,
    /// Said once per game: the first held regeneration.
    said_hold: AtomicBool,
}

/// The feature: hooks on the damage update and the body restore, a tick
/// observer for the meter, a game-start line.
pub struct H1Health {
    state: Arc<State>,
}

impl feature::Feature for H1Health {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(FEATURE, "Halo 1's health model on Halo 1 maps").doc("docs/game/health.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        use feature::points::{BODY_RESTORE, OBJECT_DAMAGE_UPDATE};
        {
            let s = Arc::clone(&self.state);
            b.raw().before(&OBJECT_DAMAGE_UPDATE, "note a player's health", move |c| {
                if mode() != HealthModel::Halo1 {
                    if s.armed.swap(false, Ordering::Relaxed) {
                        *s.pending.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    }
                    return Ok(feature::Flow::Continue);
                }
                let object = c.arg_u32("object").unwrap_or(u32::MAX);
                let noted = {
                    let mem = c.ctx.runtime().memory();
                    sample(&*mem, object).ok().flatten()
                };
                *s.pending.lock().unwrap_or_else(|e| e.into_inner()) = noted;
                s.armed.store(noted.is_some(), Ordering::Relaxed);
                Ok(feature::Flow::Continue)
            });
        }
        {
            let s = Arc::clone(&self.state);
            b.raw().after(&OBJECT_DAMAGE_UPDATE, "hold the body's health", move |c| {
                if !s.armed.swap(false, Ordering::Relaxed) {
                    return Ok(());
                }
                let Some(before) = s.pending.lock().unwrap_or_else(|e| e.into_inner()).take() else {
                    return Ok(());
                };
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                if let Ok(Some(would)) = apply_hold(&mut *mem, mode(), before)
                    && !s.said_hold.swap(true, Ordering::Relaxed)
                {
                    eprintln!(
                        "health: Halo 1 model: held a player's body at {:.3} (Halo 2 would have regenerated it to {would:.3}); said once per game",
                        before.health
                    );
                }
                Ok(())
            });
        }
        {
            let s = Arc::clone(&self.state);
            b.raw().observe_before(&BODY_RESTORE, "note the health pack", move |o| {
                let object = o.cpu.eax;
                let health = sample(&*o.guest(), object).ok().flatten().map(|x| x.health);
                *s.restore.lock().unwrap_or_else(|e| e.into_inner()) = Some((object, health));
            });
        }
        {
            let s = Arc::clone(&self.state);
            b.raw().observe_after(&BODY_RESTORE, "log the health pack", move |o| {
                let Some((object, before)) = s.restore.lock().unwrap_or_else(|e| e.into_inner()).take() else {
                    return;
                };
                eprintln!("{}", restore_line(object, before, o.cpu.eax & 0xff != 0, mode()));
            });
        }
        b.frame().on_tick("publish the health meter", |o| {
            let v = meter_now(&*o.guest());
            publish_meter(v);
        });
        {
            let s = Arc::clone(&self.state);
            b.game().on_start("say the health model", move |_| {
                s.said_hold.store(false, Ordering::Relaxed);
                eprintln!("{}", start_line(mode(), forced().is_some(), HUD_HAS_METER.load(Ordering::Relaxed)));
            });
        }
        Ok(())
    }
}

/// The health pack's log line.
pub fn restore_line(object: u32, before: Option<f32>, restored: bool, model: HealthModel) -> String {
    let was = before.map_or("a non-player's body".to_string(), |h| format!("a player's body at {h:.3}"));
    if restored {
        format!("health: health pack: object {object:#010x}, {was}, restored to full ({} model)", model.name())
    } else {
        format!("health: health pack refused: object {object:#010x}, {was} (full or dead; the pack stays)")
    }
}

/// The game-start line.
pub fn start_line(model: HealthModel, forced: bool, hud_meter: bool) -> String {
    let source = if forced { format!(" (debug override {ENV})") } else { String::new() };
    match model {
        HealthModel::Halo2 => format!("health: game start: Halo 2 model{source}"),
        HealthModel::Halo1 => format!(
            "health: game start: Halo 1 model{source}: no body regeneration; meter {}",
            if hud_meter { "from the map's HUD" } else { "drawn by the window" }
        ),
    }
}

/// Adds the feature to `registry`, after reading the debug override from
/// `H2_HEALTH_MODEL`. Returns whether it was added.
pub fn install(registry: &mut feature::Registry) -> anyhow::Result<bool> {
    let env = std::env::var(ENV).ok();
    match parse_override(env.as_deref()) {
        Ok(Some(m)) => {
            force(Some(m));
            eprintln!(
                "health: DEBUG OVERRIDE {ENV}={}: the {} health model on every map (not for play; docs/game/health.md)",
                env.as_deref().unwrap_or_default().trim(),
                m.name()
            );
        }
        Ok(None) => {}
        Err(e) => eprintln!("health: {e}; ignored"),
    }
    let added = registry
        .add(&H1Health { state: Arc::default() })
        .map_err(|e| anyhow::anyhow!("health: {e}"))?;
    if !added {
        eprintln!("health: off: switched off (H2_FEATURES); Halo 2's model on every map");
    }
    Ok(added)
}

/// The install line, once `plan` is in.
pub fn report(plan: &feature::Plan, installed: &feature::Installed) -> String {
    use feature::points::{BODY_RESTORE, OBJECT_DAMAGE_UPDATE};
    if let Some(why) = installed.off_reason(FEATURE) {
        return format!("health: NOT installed: {why}; Halo 2's model on every map");
    }
    if !plan.is_on(FEATURE) {
        return "health: NOT installed: not in the feature plan".into();
    }
    let at = |p: &feature::points::Point, what: &str| {
        let chain = plan
            .chains
            .values()
            .find(|c| std::ptr::eq(c.point, p) && c.hooks().any(|h| h.feature == FEATURE));
        match chain {
            Some(c) if installed.placed.contains(&c.addr) => format!("{:#x} ({what})", c.addr),
            Some(c) => format!("{:#x} ({what}, NOT placed)", c.addr),
            None => format!("{} ({what}, NOT in the plan)", p.name),
        }
    };
    format!(
        "health: hooks installed via feature plan at {}, {}; model now {}",
        at(&OBJECT_DAMAGE_UPDATE, "regeneration"),
        at(&BODY_RESTORE, "health pack log"),
        mode().name()
    )
}

#[cfg(test)]
mod tests;
