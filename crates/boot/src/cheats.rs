//! Our cheats, for the debug menu and `h2 agent` (docs/runtime/debug-menu.md).
//!
//! The retail game ships no usable cheat commands, so these are our own
//! effects, applied after each game tick to the local players' units with
//! the helpers our skulls use (`skulls_custom`):
//!
//! * `god`: the body's cannot-take-damage flag (`BODY_DAMAGE_FLAGS` bit
//!   `CANNOT_TAKE_DAMAGE`), set while on; cleared on the units we set it
//!   on when switched off, never on a unit the game had flagged itself;
//! * `infinite_ammo`: each magazine's reserve to its definition's maximum
//!   (an energy weapon's charge to full) and both grenade counts topped;
//! * `bottomless_clip`: each magazine's loaded rounds to the definition's
//!   rounds-loaded maximum, so no reload is ever needed;
//! * `game_speed`: game time's tick rate, seconds per tick and speed
//!   fields set the way the game's own set-speed helper (`0x146980`) sets
//!   them, from the options' tick rate; held while not 1, restored once.
//!
//! The host methods: `cheat.list` (each cheat, its value, and why they
//! are refused if they are) and `cheat.set {name, value}`.
//!
//! Refused in a co-op or networked session (the simulation's mode is a
//! sync or distributed one): only this machine would cheat, and the
//! players would desync. Also refused in a lockstep run (`H2_LOCKSTEP`,
//! input record or replay), which compares deterministic runs. When a
//! session turns into one of those with cheats on, they are switched off.
//!
//! Research points (addresses and offsets only):
//! * game time at `[0x510c54]` (`engine::points::GAME_TIME`): `+0x2` the
//!   tick rate (i16), `+0x4` seconds per tick (f32), `+0xc` the speed
//!   (f32). Its update (`0x146660`) runs `rate * speed * dt` ticks a
//!   frame; `0x1465b0` (a new map) puts the speed back to 1;
//! * the options' tick rate: i16 at `+0xe` in the game globals
//!   (`[0x4e6948]`);
//! * a weapon's magazine: loaded rounds at `+0x4` in its state (`0x228 +
//!   0x10 * i`); the definition's rounds-loaded maximum at `+0xa` in its
//!   magazine block (`0x101a10` fills a magazine from it).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use agent::Access;
use agent_proto::{RpcError, code};
use engine::memory::{GuestMemory, MemFault};
use serde_json::{Value, json};

use crate::skulls_custom::{
    BODY_DAMAGE_FLAGS, CANNOT_TAKE_DAMAGE, GAME_GLOBALS, GLOBALS_MODE, LocalUnit, MAGAZINE_STRIDE,
    MAX_MAGAZINES, UNIT_GRENADES, WEAP_MAGAZINE_BLOCK, WEAP_MAGAZINE_COUNT, WEAP_MAGAZINE_STRIDE,
    WEAPON_MAGAZINES, local_units, refill, topped, weapon_definition, weapons_of,
};

/// The feature id in the plan and `H2_FEATURES`.
pub const FEATURE: &str = "cheats";

/// Game time's tick rate (i16).
pub const TIME_RATE: u32 = 0x2;
/// Game time's seconds per tick (f32).
pub const TIME_TICK_SECONDS: u32 = 0x4;
/// Game time's speed (f32).
pub const TIME_SPEED: u32 = 0xc;
/// The options' tick rate in the game globals (i16).
pub const GLOBALS_TICK_RATE: u32 = 0xe;
/// A magazine's loaded rounds (i16), in the weapon's state.
pub const MAGAZINE_LOADED: u32 = 0x4;
/// The definition's rounds-loaded maximum (i16), in a magazine block.
pub const WEAP_LOADED_MAX: u32 = 0xa;

/// The game modes cheats act in (the game globals' mode): campaign and
/// multiplayer, not the main menu.
const PLAYING: [u32; 2] = [1, 2];

/// The speeds offered.
pub const SPEEDS: [f32; 5] = [0.25, 0.5, 1.0, 1.5, 2.0];

/// Why cheats are refused, if they are: `sim_mode` is the simulation's
/// mode (`coop_probe::read_sim`), `lockstep` a lockstep run.
pub fn refusal(sim_mode: Option<u32>, lockstep: bool) -> Option<&'static str> {
    if lockstep {
        return Some(
            "a lockstep run (H2_LOCKSTEP or input record/replay) compares deterministic runs",
        );
    }
    match sim_mode {
        Some(2 | 3) => {
            Some("a co-op session: only this machine would cheat, and the players would desync")
        }
        Some(4 | 5) => Some("a networked session: only this machine would cheat"),
        _ => None,
    }
}

/// The simulation's mode, read as `coop_probe::read_sim` does.
pub fn sim_mode(m: &dyn GuestMemory) -> Option<u32> {
    let g = m.u32(engine::points::SIM_GLOBALS.base).ok()?;
    if g == 0 {
        return None;
    }
    m.u32(g.wrapping_add(0x8)).ok()
}

/// Game time's fields for speed `s`, the way the game's set-speed helper
/// computes them from the options' tick rate: (tick rate, seconds per
/// tick, speed). More ticks a second when slower, each one shorter.
pub fn speed_fields(options_rate: i16, s: f32) -> (i16, f32, f32) {
    let base = f32::from(options_rate.max(1));
    let rate = (base / s.max(0.01)).round().clamp(1.0, f32::from(i16::MAX)) as i16;
    (rate, 1.0 / f32::from(rate), base / f32::from(rate))
}

/// What is on, and what we did to undo.
#[derive(Debug)]
pub struct Cheats {
    pub god: bool,
    pub ammo: bool,
    pub bottomless: bool,
    pub speed: f32,
    /// Units whose damage flag we set (datums).
    flagged: HashSet<u32>,
    /// Game time is at a speed we set.
    sped: bool,
    /// The feature is in the plan.
    pub installed: bool,
}

impl Default for Cheats {
    fn default() -> Self {
        Cheats {
            god: false,
            ammo: false,
            bottomless: false,
            speed: 1.0,
            flagged: HashSet::new(),
            sped: false,
            installed: true,
        }
    }
}

/// One cheat as `cheat.list` lists it.
struct Info {
    name: &'static str,
    label: &'static str,
    about: &'static str,
}

const GOD: Info = Info {
    name: "god",
    label: "god mode",
    about: "the local players take no damage",
};
const AMMO: Info = Info {
    name: "infinite_ammo",
    label: "infinite ammo",
    about: "reserve ammo, battery and grenades kept full; still reloads",
};
const BOTTOMLESS: Info = Info {
    name: "bottomless_clip",
    label: "bottomless clip",
    about: "the loaded magazine kept full: no reloads (not plasma heat)",
};
const SPEED: Info = Info {
    name: "game_speed",
    label: "game speed",
    about: "game time against real time; back to 1 on a new map",
};

impl Cheats {
    pub fn any(&self) -> bool {
        self.god || self.ammo || self.bottomless || self.speed != 1.0
    }

    /// `cheat.list`'s answer.
    pub fn list(&self, refused: Option<&str>) -> Value {
        let b = |i: &Info, v: bool| json!({"name": i.name, "label": i.label, "about": i.about, "kind": "bool", "value": v});
        let choices: Vec<Value> = SPEEDS
            .iter()
            .map(|s| json!({"value": s, "label": format!("{s}x")}))
            .collect();
        json!({
            "refused": refused,
            "cheats": [
                b(&GOD, self.god),
                b(&AMMO, self.ammo),
                b(&BOTTOMLESS, self.bottomless),
                {
                    "name": SPEED.name, "label": SPEED.label, "about": SPEED.about,
                    "kind": "choice", "value": self.speed, "choices": choices,
                },
            ],
        })
    }

    /// `cheat.set {name, value}`; the effect follows at the next tick.
    pub fn set(&mut self, p: &Value, refused: Option<&str>) -> Result<Value, RpcError> {
        if let Some(why) = refused {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                format!("cheats are off: {why}"),
            ));
        }
        let invalid = |m: String| RpcError::new(code::INVALID_PARAMS, m);
        let name = p["name"]
            .as_str()
            .ok_or_else(|| invalid("name: the cheat (cheat.list lists them)".into()))?;
        let v = &p["value"];
        let flag = |v: &Value| {
            v.as_bool()
                .ok_or_else(|| invalid(format!("{name}: true or false, not {v}")))
        };
        match name {
            "god" => self.god = flag(v)?,
            "infinite_ammo" => self.ammo = flag(v)?,
            "bottomless_clip" => self.bottomless = flag(v)?,
            "game_speed" => {
                let s = v
                    .as_f64()
                    .ok_or_else(|| invalid(format!("game_speed: a number, not {v}")))?
                    as f32;
                if !SPEEDS.contains(&s) {
                    return Err(invalid(format!("game_speed: one of {SPEEDS:?}")));
                }
                self.speed = s;
            }
            _ => return Err(invalid(format!("no cheat {name:?}"))),
        }
        Ok(self.list(None))
    }

    /// Switches everything off (a session that refuses them).
    pub fn all_off(&mut self) {
        self.god = false;
        self.ammo = false;
        self.bottomless = false;
        self.speed = 1.0;
    }

    /// After a game tick: applies what is on to the local players, and
    /// undoes what was switched off. Nothing is read when nothing is on
    /// and nothing needs undoing.
    pub fn tick(
        &mut self,
        m: &mut dyn GuestMemory,
        refused: Option<&str>,
    ) -> Result<Vec<String>, MemFault> {
        let mut said = Vec::new();
        if refused.is_some() && self.any() {
            self.all_off();
            said.push(format!(
                "cheats: switched off: {}",
                refused.unwrap_or_default()
            ));
        }
        if !self.any() && self.flagged.is_empty() && !self.sped {
            return Ok(said);
        }
        let globals = m.u32(GAME_GLOBALS)?;
        let playing = globals != 0 && PLAYING.contains(&m.u32(globals.wrapping_add(GLOBALS_MODE))?);
        if !playing {
            // The menus: a new map resets the speed and the units go.
            self.flagged.clear();
            self.sped = false;
            return Ok(said);
        }
        self.time(m, globals)?;
        let units = local_units(m)?;
        self.units(m, &units)?;
        Ok(said)
    }

    fn time(&mut self, m: &mut dyn GuestMemory, globals: u32) -> Result<(), MemFault> {
        if self.speed == 1.0 && !self.sped {
            return Ok(());
        }
        let t = m.u32(engine::points::GAME_TIME.base)?;
        if t == 0 {
            return Ok(());
        }
        let opt = m.u16(globals.wrapping_add(GLOBALS_TICK_RATE))? as i16;
        if opt <= 0 {
            return Ok(());
        }
        let (rate, secs, speed) = speed_fields(opt, self.speed);
        if m.u16(t.wrapping_add(TIME_RATE))? as i16 != rate {
            m.set_u16(t.wrapping_add(TIME_RATE), rate as u16)?;
        }
        if m.f32(t.wrapping_add(TIME_TICK_SECONDS))? != secs {
            m.set_f32(t.wrapping_add(TIME_TICK_SECONDS), secs)?;
        }
        if m.f32(t.wrapping_add(TIME_SPEED))? != speed {
            m.set_f32(t.wrapping_add(TIME_SPEED), speed)?;
        }
        self.sped = self.speed != 1.0;
        Ok(())
    }

    fn units(&mut self, m: &mut dyn GuestMemory, units: &[LocalUnit]) -> Result<(), MemFault> {
        for u in units {
            let b = u.body;
            let at = b.wrapping_add(BODY_DAMAGE_FLAGS);
            let f = m.u16(at)?;
            if self.god {
                if f & CANNOT_TAKE_DAMAGE == 0 {
                    m.set_u16(at, f | CANNOT_TAKE_DAMAGE)?;
                    self.flagged.insert(u.unit);
                }
            } else if self.flagged.remove(&u.unit) && f & CANNOT_TAKE_DAMAGE != 0 {
                m.set_u16(at, f & !CANNOT_TAKE_DAMAGE)?;
            }
            if self.ammo {
                let now = [
                    m.u8(b.wrapping_add(UNIT_GRENADES))? as i8,
                    m.u8(b.wrapping_add(UNIT_GRENADES + 1))? as i8,
                ];
                let want = topped(now);
                if want != now {
                    m.set_u8(b.wrapping_add(UNIT_GRENADES), want[0] as u8)?;
                    m.set_u8(b.wrapping_add(UNIT_GRENADES + 1), want[1] as u8)?;
                }
            }
            if self.ammo || self.bottomless {
                for w in weapons_of(m, b)? {
                    let Some((wb, def)) = weapon_definition(m, w)? else {
                        continue;
                    };
                    if self.ammo {
                        refill(m, wb, def)?;
                    }
                    if self.bottomless {
                        load(m, wb, def)?;
                    }
                }
            }
        }
        if !self.god {
            // Units gone (dead, a new map): nothing left to undo on them.
            self.flagged.retain(|d| units.iter().any(|u| u.unit == *d));
        }
        Ok(())
    }
}

/// Bottomless clip for one weapon: each magazine's loaded rounds to the
/// definition's rounds-loaded maximum; an energy weapon's charge to full.
/// Returns whether anything changed.
pub fn load(m: &mut dyn GuestMemory, weapon_body: u32, def: u32) -> Result<bool, MemFault> {
    let count = m.i32(def.wrapping_add(WEAP_MAGAZINE_COUNT))?;
    if count <= 0 {
        return refill(m, weapon_body, def);
    }
    let block = m.u32(def.wrapping_add(WEAP_MAGAZINE_BLOCK))?;
    let mut changed = false;
    for i in 0..(count as u32).min(MAX_MAGAZINES) {
        let max = m.u16(block.wrapping_add(i * WEAP_MAGAZINE_STRIDE + WEAP_LOADED_MAX))? as i16;
        let at = weapon_body.wrapping_add(WEAPON_MAGAZINES + i * MAGAZINE_STRIDE + MAGAZINE_LOADED);
        if max > 0 && (m.u16(at)? as i16) < max {
            m.set_u16(at, max as u16)?;
            changed = true;
        }
    }
    Ok(changed)
}

/// The cheats' after-tick hook.
pub struct CheatsFeature(pub Arc<Mutex<Cheats>>, pub bool);

fn lock(c: &Mutex<Cheats>) -> std::sync::MutexGuard<'_, Cheats> {
    c.lock().unwrap_or_else(|e| e.into_inner())
}

impl feature::Feature for CheatsFeature {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(
            FEATURE,
            "Our cheats: god mode, ammo, game speed (debug menu)",
        )
        .doc("docs/runtime/debug-menu.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        use feature::points::GAME_TICK;
        let c = Arc::clone(&self.0);
        let lockstep = self.1;
        b.raw()
            .after(&GAME_TICK, "our cheats", move |cx| {
                let mut c = lock(&c);
                if !c.any() && c.flagged.is_empty() && !c.sped {
                    return Ok(());
                }
                let rt = Arc::clone(cx.ctx.runtime());
                let mut mem = rt.memory();
                let m: &mut dyn GuestMemory = &mut *mem;
                let refused = refusal(sim_mode(m), lockstep);
                match c.tick(m, refused) {
                    Ok(said) => said.iter().for_each(|l| eprintln!("{l}")),
                    Err(e) => eprintln!("cheats: {e:?}"),
                }
                Ok(())
            })
            // touch-glue and skulls-custom also hook after the tick. Cheats run
            // outside them (last), so a cheat's writes (health, ammo, speed)
            // are what the next tick starts from, whatever they set.
            .after_feature(crate::touch_glue::FEATURE)
            .after_feature(crate::skulls_custom::FEATURE);
        Ok(())
    }
}

/// Adds the cheats to `registry` and `cheat.list`/`cheat.set` to `agent`.
/// `lockstep`: a lockstep run, which refuses them.
pub fn install(
    registry: &mut feature::Registry,
    agent: &agent::Agent,
    rt: &Arc<runtime::Runtime>,
    lockstep: bool,
) -> anyhow::Result<bool> {
    let cheats = Arc::new(Mutex::new(Cheats::default()));
    let added = registry
        .add(&CheatsFeature(Arc::clone(&cheats), lockstep))
        .map_err(|e| anyhow::anyhow!("cheats: {e}"))?;
    lock(&cheats).installed = added;
    let off = "the cheats feature is switched off (H2_FEATURES)";
    let refused = move |rt: &runtime::Runtime, installed: bool| -> Option<&'static str> {
        if !installed {
            return Some(off);
        }
        let m = rt.memory();
        refusal(sim_mode(&*m), lockstep)
    };
    let (c, r) = (Arc::clone(&cheats), Arc::clone(rt));
    agent.add_method(
        debug_menu::sources::method::CHEAT_LIST,
        Access::Read,
        move |_| {
            let c = lock(&c);
            Ok(c.list(refused(&r, c.installed)))
        },
    );
    let (c, r) = (Arc::clone(&cheats), Arc::clone(rt));
    agent.add_method(
        debug_menu::sources::method::CHEAT_SET,
        Access::Act,
        move |p| {
            let mut c = lock(&c);
            let why = refused(&r, c.installed);
            c.set(&p, why)
        },
    );
    if !added {
        eprintln!("cheats: off: switched off (H2_FEATURES)");
    }
    Ok(added)
}

#[cfg(test)]
mod tests;
