//! The Halo 1 health model on Halo 2's engine (docs/halo1/health.md).
//!
//! Halo 2 regenerates a player's body vitality once the body's stun timer
//! runs out; Halo 1 does not, and has health packs instead. For a Halo 1
//! level (campaign, or a Halo 1 map in multiplayer) the game runs Halo 1's
//! model; everywhere else it is retail's.
//!
//! - **No regeneration.** Around the object damage update
//!   ([`OBJECT_DAMAGE_UPDATE`](hooks::points::OBJECT_DAMAGE_UPDATE),
//!   `0xd5de0`, once per object per tick): `before` notes a player biped's
//!   body health and its "body recharging" flag, `after` puts them back if
//!   the update raised the health. That update's only health write that
//!   raises it is the regeneration (`0xd63de`..`0xd64b5`); damage, kills,
//!   scripts, checkpoints and health packs write it elsewhere and are left
//!   alone.
//! - **Health packs** need no hook: Halo 2's powerup pickup (`0x151ea0`)
//!   already has a health case (equipment powerup type 5), which calls
//!   [`BODY_RESTORE`](hooks::points::BODY_RESTORE) (`0xd6790`, body to
//!   full; refused when full or dead). An observer logs each use.
//! - **HUD.** The meter is part of Halo 2's HUD: the map converter gives
//!   the converted Halo 1 player a copy of its `nhdt` with a widget on HUD
//!   input 17, the unit's body (`h1objects::health_hud`), so the game draws,
//!   places and hides it with the shield meter. Nothing here draws it. A
//!   retail map under the debug switch has no meter.
//!
//! **The switch.** At each game start [`set_mode`] gets [`model_for_map`]
//! of the loaded map's name: converted Halo 1 maps (`h1_<map>` in the Halo
//! 1 sandbox, Halo 1's campaign levels) are Halo 1's. The debug
//! override `H2_HEALTH_MODEL=halo1|halo2` beats it, so the model can be
//! tried on a retail level.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::view::objects;

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
    /// meter beside the shield meter (in the converted map's HUD).
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

/// Sets the model for the game being started, from the map's identity: a
/// Halo 1 map is [`HealthModel::Halo1`], anything else
/// [`HealthModel::Halo2`]. It stays until set again, so call it at every
/// game start, not only for Halo 1 maps. Safe from any thread; the change
/// is seen by the next object update.
pub fn set_mode(m: HealthModel) {
    MAP_MODE.store(m.code(), Ordering::SeqCst);
}

/// Whether converted Halo 1 multiplayer maps were installed in their Halo
/// 1 sandbox this run (`multiplayer.halo1_sandbox`, read at boot).
static HALO1_SANDBOX: AtomicBool = AtomicBool::new(true);

/// Records which sandbox `boot::new_maps` installed converted Halo 1
/// multiplayer maps in (`multiplayer.halo1_sandbox` is `h1`): the game
/// start then picks [`model_for_map`] by it.
pub fn set_halo1_sandbox(on: bool) {
    HALO1_SANDBOX.store(on, Ordering::SeqCst);
}

/// Whether converted Halo 1 multiplayer maps are in their Halo 1 sandbox
/// this run ([`set_halo1_sandbox`]).
pub fn halo1_sandbox() -> bool {
    HALO1_SANDBOX.load(Ordering::SeqCst)
}

/// Halo 1's campaign levels, as a converted campaign names its map.
pub const HALO1_LEVELS: [&str; 10] = [
    "a10", "a30", "a50", "b30", "b40", "c10", "c20", "c40", "d20", "d40",
];

/// The model a map asks for by its name (the cache header's): a converted
/// Halo 1 multiplayer map (`h1_<map>`) is Halo 1's in the Halo 1 sandbox
/// and Halo 2's in the Halo 2 sandbox, a converted Halo 1 campaign level
/// is Halo 1's, anything else Halo 2's.
pub fn model_for_map(name: &str, halo1_sandbox: bool) -> HealthModel {
    let name = name.trim().to_ascii_lowercase();
    let halo1 = if name.starts_with("h1_") {
        halo1_sandbox
    } else {
        HALO1_LEVELS.contains(&name.as_str())
    };
    if halo1 {
        HealthModel::Halo1
    } else {
        HealthModel::Halo2
    }
}

/// The loaded map's short name, in the cache header the game keeps
/// (docs/live/live-gamestate.md; `live_telemetry`'s `CACHE_MAP_NAME`).
pub const CACHE_MAP_NAME: u32 = 0x0054_7898;

/// The loaded map's name ([`CACHE_MAP_NAME`], at most 32 bytes).
pub fn loaded_map_name(m: &dyn GuestMemory) -> Result<String, MemFault> {
    Ok(read_map_name(m, &mut [0; 32])?.into_owned())
}

/// Read the resident cache identity for a draw without allocating an owned name.
pub(crate) fn read_map_name<'a>(
    m: &dyn GuestMemory, field: &'a mut [u8; 32],
) -> Result<std::borrow::Cow<'a, str>, MemFault> {
    let len = if m.read(CACHE_MAP_NAME, field).is_ok() {
        field.iter().position(|&b| b == 0).unwrap_or(field.len())
    } else {
        // A partial mapping is sufficient when the terminator is readable.
        let mut len = 0;
        for byte in field.iter_mut() {
            *byte = m.u8(CACHE_MAP_NAME + len as u32)?;
            if *byte == 0 { break }
            len += 1;
        }
        len
    };
    Ok(String::from_utf8_lossy(&field[..len]))
}

/// After a machine snapshot loads: the model for the loaded map, as the
/// game start picks it (the debug override still beats it).
pub fn reload_mode(m: &dyn GuestMemory) -> Result<(), String> {
    let name = loaded_map_name(m).map_err(|e| format!("the map's name: {e:?}"))?;
    set_mode(model_for_map(&name, HALO1_SANDBOX.load(Ordering::Relaxed)));
    Ok(())
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

// ---- the guest layout (docs/halo1/health.md "What we do") ----

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
    (model == HealthModel::Halo1 && health > before.health).then_some((
        before.health,
        (flags_high & !BODY_RECHARGING) | (before.flags_high & BODY_RECHARGING),
    ))
}

/// [`hold`] against the body as the update left it; the health it held
/// back from, when it did.
pub fn apply_hold(
    m: &mut dyn GuestMemory,
    model: HealthModel,
    before: Sample,
) -> Result<Option<f32>, MemFault> {
    let health = m.f32(before.body.wrapping_add(BODY_HEALTH))?;
    let flags = m.u8(before.body.wrapping_add(BODY_FLAGS_HIGH))?;
    let Some((h, f)) = hold(model, before, health, flags) else {
        return Ok(None);
    };
    m.set_f32(before.body.wrapping_add(BODY_HEALTH), h)?;
    m.set_u8(before.body.wrapping_add(BODY_FLAGS_HIGH), f)?;
    Ok(Some(health))
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

impl State {
    /// Why a snapshot can't be taken now: a damage update or a body restore
    /// is between its two hooks. Otherwise the model is a function of the
    /// loaded map (`set_mode` at game start, from guest memory), so a load
    /// rebuilds it.
    fn live(&self) -> Option<String> {
        if self.armed.load(Ordering::Relaxed)
            || self
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some()
        {
            return Some("a damage update is between its hooks".into());
        }
        if self
            .restore
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return Some("a body restore is between its hooks".into());
        }
        None
    }
}

/// The feature: hooks on the damage update and the body restore, a
/// game-start line.
pub struct H1Health {
    state: Arc<State>,
}

impl hooks::Feature for H1Health {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Halo 1's health model on Halo 1 maps")
            .doc("docs/halo1/health.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::points::{BODY_RESTORE, OBJECT_DAMAGE_UPDATE};
        {
            let s = Arc::clone(&self.state);
            b.raw()
                .before(&OBJECT_DAMAGE_UPDATE, "note a player's health", move |c| {
                    if mode() != HealthModel::Halo1 {
                        if s.armed.swap(false, Ordering::Relaxed) {
                            *s.pending.lock().unwrap_or_else(|e| e.into_inner()) = None;
                        }
                        return Ok(hooks::Flow::Continue);
                    }
                    let object = c.arg_u32("object").unwrap_or(u32::MAX);
                    let noted = {
                        let mem = c.ctx.runtime().memory();
                        sample(&*mem, object).ok().flatten()
                    };
                    *s.pending.lock().unwrap_or_else(|e| e.into_inner()) = noted;
                    s.armed.store(noted.is_some(), Ordering::Relaxed);
                    Ok(hooks::Flow::Continue)
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
            b.raw()
                .observe_before(&BODY_RESTORE, "note the health pack", move |o| {
                    let object = o.cpu.eax;
                    let health = sample(&*o.guest(), object).ok().flatten().map(|x| x.health);
                    *s.restore.lock().unwrap_or_else(|e| e.into_inner()) = Some((object, health));
                });
        }
        {
            let s = Arc::clone(&self.state);
            b.raw()
                .observe_after(&BODY_RESTORE, "log the health pack", move |o| {
                    let Some((object, before)) =
                        s.restore.lock().unwrap_or_else(|e| e.into_inner()).take()
                    else {
                        return;
                    };
                    eprintln!(
                        "{}",
                        restore_line(object, before, o.cpu.eax & 0xff != 0, mode())
                    );
                });
        }
        {
            // Nothing saved: the model follows the loaded map, picked again
            // from guest memory as at a game start (docs/runtime/snapshots.md).
            let s = Arc::clone(&self.state);
            b.snapshot(hooks::SnapshotPolicy::section_forbid_while(
                move || s.live(),
                Vec::new,
                |_, rt| reload_mode(&*rt.memory()),
            ));
        }
        {
            let s = Arc::clone(&self.state);
            b.game()
                .on_start("pick and say the health model", move |o| {
                    s.said_hold.store(false, Ordering::Relaxed);
                    // The map's identity picks the model (the debug override
                    // still beats it).
                    match loaded_map_name(&*o.guest()) {
                        Ok(name) => {
                            set_mode(model_for_map(&name, HALO1_SANDBOX.load(Ordering::Relaxed)))
                        }
                        Err(e) => eprintln!(
                            "health: the map's name could not be read ({e:?}); model unchanged"
                        ),
                    }
                    eprintln!("{}", start_line(mode(), forced().is_some()));
                });
        }
        Ok(())
    }
}

/// The health pack's log line.
pub fn restore_line(
    object: u32,
    before: Option<f32>,
    restored: bool,
    model: HealthModel,
) -> String {
    let was = before.map_or("a non-player's body".to_string(), |h| {
        format!("a player's body at {h:.3}")
    });
    if restored {
        format!(
            "health: health pack: object {object:#010x}, {was}, restored to full ({} model)",
            model.name()
        )
    } else {
        format!(
            "health: health pack refused: object {object:#010x}, {was} (full or dead; the pack stays)"
        )
    }
}

/// The game-start line.
pub fn start_line(model: HealthModel, forced: bool) -> String {
    let source = if forced {
        format!(" (debug override {ENV})")
    } else {
        String::new()
    };
    match model {
        HealthModel::Halo2 => format!("health: game start: Halo 2 model{source}"),
        HealthModel::Halo1 => format!(
            "health: game start: Halo 1 model{source}: no body regeneration; the health meter is in a converted Halo 1 map's HUD"
        ),
    }
}

/// Adds the feature to `registry`, after reading the debug override from
/// `H2_HEALTH_MODEL`. Returns whether it was added.
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<bool> {
    let env = std::env::var(ENV).ok();
    match parse_override(env.as_deref()) {
        Ok(Some(m)) => {
            force(Some(m));
            eprintln!(
                "health: DEBUG OVERRIDE {ENV}={}: the {} health model on every map (not for play; docs/halo1/health.md)",
                env.as_deref().unwrap_or_default().trim(),
                m.name()
            );
        }
        Ok(None) => {}
        Err(e) => eprintln!("health: {e}; ignored"),
    }
    let added = registry
        .add(&H1Health {
            state: Arc::default(),
        })
        .map_err(|e| anyhow::anyhow!("health: {e}"))?;
    if !added {
        eprintln!("health: off: switched off (H2_FEATURES); Halo 2's model on every map");
    }
    Ok(added)
}

/// The install line, once `plan` is in.
pub fn report(plan: &hooks::Plan, installed: &hooks::Installed) -> String {
    use hooks::points::{BODY_RESTORE, OBJECT_DAMAGE_UPDATE};
    if let Some(why) = installed.off_reason(FEATURE) {
        return format!("health: NOT installed: {why}; Halo 2's model on every map");
    }
    if !plan.is_on(FEATURE) {
        return "health: NOT installed: not in the feature plan".into();
    }
    let at = |p: &hooks::points::Point, what: &str| {
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
