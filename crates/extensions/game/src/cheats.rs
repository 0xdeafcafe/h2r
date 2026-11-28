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
//!   them, from the options' tick rate; held while not 1, restored once;
//! * `fly` ([`flight`]): flight with the player's own controls, through
//!   the game's physics (walls still stop you). On the ground, a crouch
//!   press takes off. In the air the move stick flies where you look (look
//!   up and push forward to climb), jump climbs, crouch sinks, and holding
//!   the stick fully for half a second ramps the speed up to four times.
//!   Coming down onto a floor with crouch held lands; switching fly off in
//!   the air drops you, shielded from the fall;
//! * `noclip` ([`flight`]): the same controls, flying from the moment it
//!   is on, through walls: we hold the body's position ourselves and write
//!   it over the physics' each tick. The body takes no damage meanwhile.
//!   Off hands the body back to the physics where it is; for three seconds
//!   it is shielded and watched, and put back on the last floor the
//!   physics stood it on if it is stuck (not moving while airborne or
//!   pushing the stick) or falls 20 units. With fly on too, turning noclip
//!   off leaves you flying;
//! * `hide_hud`: the HUD globals' shown flag, which the HUD's draw test
//!   (`0x13939b`) reads and a script builtin (`0x2aa61a`) writes, held at 0;
//!   the value the game last set is put back when off;
//! * `ai_off`: the AI globals' on byte, which `ai_update` (`0x1c83c0`)
//!   gates every actor on and the script's `ai` switch writes, held at 0
//!   (the actors stand still); the value the game last set is put back
//!   when off.
//!
//! The host methods: `cheat.list` (each cheat, its value, why it is
//! refused if it is, and why it waits if no game is running) and
//! `cheat.set {name, value}`. A cheat set at the main menu is kept and
//! takes hold when a campaign or multiplayer game starts; until then the
//! list says it is `pending`.
//!
//! Who may cheat, and where ([`policy`]): anyone off Xbox Live; on Live,
//! only the gamertags on [`policy::LIVE_ALLOWED`]. Whoever you are, a
//! lockstep run refuses every cheat, and a co-op session or a networked
//! game's guest refuses the ones that change the simulation (all but
//! `hide_hud`): only this machine would change. A networked game's host
//! keeps them (its simulation is the game) but not `game_speed`. A cheat
//! that becomes refused while on is switched off and undone.
//!
//! Research points (addresses and offsets only):
//! * game time at `[0x510c54]` (`guest_layout::points::GAME_TIME`): `+0x2` the
//!   tick rate (i16), `+0x4` seconds per tick (f32), `+0xc` the speed
//!   (f32). Its update (`0x146660`) runs `rate * speed * dt` ticks a
//!   frame; `0x1465b0` (a new map) puts the speed back to 1;
//! * the options' tick rate: i16 at `+0xe` in the game globals
//!   (`[0x4e6948]`);
//! * a weapon's magazine: loaded rounds at `+0x4` in its state (`0x228 +
//!   0x10 * i`); the definition's rounds-loaded maximum at `+0xa` in its
//!   magazine block (`0x101a10` fills a magazine from it);
//! * the unit control (`0xc6de0`, copied into the unit at `0xc6e02..`):
//!   control flags at unit `+0x148` (bit 0 crouch, read by the biped's
//!   crouch request at `0xe28b1`; bit 1 jump, read by its jump test at
//!   `0xe469c`), throttle at `+0x1b0` (forward, left, up), aiming vector
//!   at `+0x15c`; the object's position at `+0x64` (object_set_position,
//!   `0xb75a0`, stores it at `0xb74c4`);
//! * the HUD globals at `[0x510c4c]`: `+0x1d2` the shown flag (byte; set
//!   to 1 at `0x1391a1`);
//! * the AI globals at `[0x4f55d0]` (`guest_layout::points::AI_GLOBALS`): `+0`
//!   the on byte. `ai_update` checks it at `0x1c83d8` before running any
//!   actor (docs/game/bots.md 2.4, docs/engine/engine-map.md), and the
//!   script's `ai` switch stores its boolean there at `0x2a4d31`
//!   (docs/game/firefight.md, `firefight::guest::switch_ai_on`); `world.ai`
//!   reads it as `globals.on` (`agent::ai`).

pub mod flight;
pub mod policy;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use agent::Access;
use agent_proto::{RpcError, code};
use guest_layout::memory::{GuestMemory, MemFault};
use serde_json::{Value, json};

use crate::skulls_custom::{
    BODY_DAMAGE_FLAGS, CANNOT_TAKE_DAMAGE, GAME_GLOBALS, GLOBALS_MODE, LocalUnit, MAGAZINE_STRIDE,
    MAX_MAGAZINES, UNIT_GRENADES, WEAP_MAGAZINE_BLOCK, WEAP_MAGAZINE_COUNT, WEAP_MAGAZINE_STRIDE,
    WEAPON_MAGAZINES, local_units, refill, topped, weapon_definition, weapons_of,
};
use flight::Pilot;
use policy::{Reach, Refusals, Session};

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
/// The HUD globals pointer, and its shown flag (byte).
pub const HUD_GLOBALS: u32 = 0x0051_0c4c;
pub const HUD_SHOWN: u32 = 0x1d2;
/// The AI globals' on byte (the pointer is `guest_layout::points::AI_GLOBALS`).
pub const AI_ON: u32 = 0x0;

/// The game modes cheats act in (the game globals' mode): campaign and
/// multiplayer, not the main menu.
const PLAYING: [u32; 2] = [1, 2];

/// Why a cheat that is on does nothing yet: no game is running.
pub const PENDING: &str = "no game is running: takes hold when a campaign or multiplayer game starts";

/// The speeds offered.
pub const SPEEDS: [f32; 7] = [0.1, 0.25, 0.5, 1.0, 1.5, 2.0, 4.0];

/// Whether a campaign or multiplayer game is running (the game globals'
/// mode), where cheats act. False when the globals can't be read.
pub fn playing(m: &dyn GuestMemory) -> bool {
    let Ok(globals) = m.u32(GAME_GLOBALS) else {
        return false;
    };
    globals != 0
        && m
            .u32(globals.wrapping_add(GLOBALS_MODE))
            .is_ok_and(|mode| PLAYING.contains(&mode))
}

/// Seconds per tick when game time can't say (30 ticks a second).
const DEFAULT_TICK: f32 = 1.0 / 30.0;

/// Why cheats are refused, if they are, in a session that is not on
/// Live: `sim_mode` is the simulation's mode (`coop_probe::read_sim`),
/// `lockstep` a lockstep run. The simulation cheats' answer.
pub fn refusal(sim_mode: Option<u32>, lockstep: bool) -> Option<String> {
    policy::refusals(&Session {
        sim_mode,
        lockstep,
        live: policy::Live::Off,
    })
    .simulation
}

/// The simulation's mode, read as `coop_probe::read_sim` does.
pub fn sim_mode(m: &dyn GuestMemory) -> Option<u32> {
    let g = m.u32(guest_layout::points::SIM_GLOBALS.base).ok()?;
    if g == 0 {
        return None;
    }
    m.u32(g.wrapping_add(0x8)).ok()
}

/// Game time's fields for speed `s`, the way the game's set-speed helper
/// computes them from the options' tick rate: (tick rate, seconds per
/// tick, speed). More ticks a second when slower, each one shorter. Game
/// time runs `rate * speed * seconds per tick` = `speed` seconds a real
/// second, so the speed is `s` itself; the rate is the nearest whole one
/// (4x at 30 ticks a second: 8 ticks a second of game time, 32 run).
pub fn speed_fields(options_rate: i16, s: f32) -> (i16, f32, f32) {
    let base = f32::from(options_rate.max(1));
    let s = s.max(0.01);
    let rate = (base / s).round().clamp(1.0, f32::from(i16::MAX)) as i16;
    (rate, 1.0 / f32::from(rate), s)
}

/// A held byte: what the game last set, and whether ours is there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Held {
    game: Option<u8>,
}

/// What is on, and what we did to undo.
#[derive(Debug)]
pub struct Cheats {
    pub god: bool,
    pub ammo: bool,
    pub bottomless: bool,
    pub speed: f32,
    pub fly: bool,
    pub noclip: bool,
    pub hide_hud: bool,
    pub ai_off: bool,
    /// Units whose damage flag we set (datums).
    flagged: HashSet<u32>,
    /// Game time is at a speed we set.
    sped: bool,
    /// Each local unit's flight (datums).
    pilots: HashMap<u32, Pilot>,
    /// The HUD's shown flag, while we hold it.
    hud: Held,
    /// The AI globals' on byte, while we hold it.
    ai: Held,
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
            fly: false,
            noclip: false,
            hide_hud: false,
            ai_off: false,
            flagged: HashSet::new(),
            sped: false,
            pilots: HashMap::new(),
            hud: Held::default(),
            ai: Held::default(),
            installed: true,
        }
    }
}

/// One cheat as `cheat.list` lists it.
struct Info {
    name: &'static str,
    label: &'static str,
    about: &'static str,
    reach: Reach,
}

const GOD: Info = Info {
    name: "god",
    label: "God mode",
    about: "the local players take no damage",
    reach: Reach::Simulation,
};
const AMMO: Info = Info {
    name: "infinite_ammo",
    label: "Infinite ammo",
    about: "reserve ammo, battery and grenades kept full; still reloads",
    reach: Reach::Simulation,
};
const BOTTOMLESS: Info = Info {
    name: "bottomless_clip",
    label: "Bottomless clip",
    about: "the loaded magazine kept full: no reloads (not plasma heat)",
    reach: Reach::Simulation,
};
const SPEED: Info = Info {
    name: "game_speed",
    label: "Game speed",
    about: "game time against real time; back to 1 on a new map",
    reach: Reach::Clock,
};
const FLY: Info = Info {
    name: "fly",
    label: "Fly",
    about: "crouch to take off; stick flies where you look, jump up, crouch down; land with crouch",
    reach: Reach::Simulation,
};
const NOCLIP: Info = Info {
    name: "noclip",
    label: "Noclip",
    about: "fly through walls: stick where you look, jump up, crouch down; off drops you here",
    reach: Reach::Simulation,
};
const HUD: Info = Info {
    name: "hide_hud",
    label: "Hide HUD",
    about: "the HUD hidden, as a script's HUD switch hides it",
    reach: Reach::Local,
};
const AI_OFF: Info = Info {
    name: "ai_off",
    label: "AI off",
    about: "the actors stand still, as the script's ai switch stops them",
    reach: Reach::Simulation,
};

const ALL: [&Info; 8] = [&GOD, &AMMO, &BOTTOMLESS, &SPEED, &FLY, &NOCLIP, &HUD, &AI_OFF];

impl Cheats {
    pub fn any(&self) -> bool {
        ALL.iter().any(|i| self.on(i.name))
    }

    /// Whether cheat `name` is on.
    fn on(&self, name: &str) -> bool {
        match name {
            "god" => self.god,
            "infinite_ammo" => self.ammo,
            "bottomless_clip" => self.bottomless,
            "game_speed" => self.speed != 1.0,
            "fly" => self.fly,
            "noclip" => self.noclip,
            "hide_hud" => self.hide_hud,
            "ai_off" => self.ai_off,
            _ => false,
        }
    }

    /// Switches cheat `name` off.
    fn off(&mut self, name: &str) {
        match name {
            "god" => self.god = false,
            "infinite_ammo" => self.ammo = false,
            "bottomless_clip" => self.bottomless = false,
            "game_speed" => self.speed = 1.0,
            "fly" => self.fly = false,
            "noclip" => self.noclip = false,
            "hide_hud" => self.hide_hud = false,
            "ai_off" => self.ai_off = false,
            _ => {}
        }
    }

    /// Whether a tick has anything to do: something on or left to undo.
    pub fn busy(&self) -> bool {
        self.any()
            || !self.flagged.is_empty()
            || self.sped
            || self.hud.game.is_some()
            || self.ai.game.is_some()
            || self.pilots.values().any(|p| !p.idle())
    }

    /// `cheat.list`'s answer. `playing`: a campaign or multiplayer game
    /// runs ([`playing`]); else every cheat that is on is `pending`.
    pub fn list(&self, refused: &Refusals, playing: bool) -> Value {
        let pending = (!playing).then_some(PENDING);
        let cheats: Vec<Value> = ALL
            .iter()
            .map(|i| {
                let mut v = json!({
                    "name": i.name, "label": i.label, "about": i.about,
                    "refused": refused.of(i.reach),
                    "pending": pending.filter(|_| self.on(i.name) && refused.of(i.reach).is_none()),
                });
                if i.name == SPEED.name {
                    // As decimals (0.1, not 0.10000000149); cheat.set takes
                    // either.
                    let shown = |s: f32| (f64::from(s) * 1000.0).round() / 1000.0;
                    let choices: Vec<Value> = SPEEDS
                        .iter()
                        .map(|s| json!({"value": shown(*s), "label": format!("{s}x")}))
                        .collect();
                    v["kind"] = json!("choice");
                    v["value"] = json!(shown(self.speed));
                    v["choices"] = json!(choices);
                } else {
                    v["kind"] = json!("bool");
                    v["value"] = json!(self.on(i.name));
                }
                v
            })
            .collect();
        json!({ "refused": refused.all, "pending": pending, "cheats": cheats })
    }

    /// `cheat.set {name, value}`; the effect follows at the next tick of a
    /// running game (the answer's `pending` says when none runs).
    pub fn set(&mut self, p: &Value, refused: &Refusals, playing: bool) -> Result<Value, RpcError> {
        let invalid = |m: String| RpcError::new(code::INVALID_PARAMS, m);
        let name = p["name"]
            .as_str()
            .ok_or_else(|| invalid("name: the cheat (cheat.list lists them)".into()))?;
        let Some(info) = ALL.iter().find(|i| i.name == name) else {
            return Err(invalid(format!("no cheat {name:?}")));
        };
        if let Some(why) = refused.of(info.reach) {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                format!("cheats are off: {why}"),
            ));
        }
        let v = &p["value"];
        let flag = |v: &Value| {
            v.as_bool()
                .ok_or_else(|| invalid(format!("{name}: true or false, not {v}")))
        };
        match name {
            "god" => self.god = flag(v)?,
            "infinite_ammo" => self.ammo = flag(v)?,
            "bottomless_clip" => self.bottomless = flag(v)?,
            "fly" => self.fly = flag(v)?,
            "noclip" => self.noclip = flag(v)?,
            "hide_hud" => self.hide_hud = flag(v)?,
            "ai_off" => self.ai_off = flag(v)?,
            _ => {
                let s = v
                    .as_f64()
                    .ok_or_else(|| invalid(format!("game_speed: a number, not {v}")))?
                    as f32;
                if !SPEEDS.contains(&s) {
                    return Err(invalid(format!("game_speed: one of {SPEEDS:?}")));
                }
                self.speed = s;
            }
        }
        Ok(self.list(refused, playing))
    }

    /// After a game tick: applies what is on to the local players, and
    /// undoes what was switched off. Nothing is read when nothing is on
    /// and nothing needs undoing.
    pub fn tick(
        &mut self,
        m: &mut dyn GuestMemory,
        refused: &Refusals,
    ) -> Result<Vec<String>, MemFault> {
        let mut said = Vec::new();
        for i in ALL {
            if self.on(i.name)
                && let Some(why) = refused.of(i.reach)
            {
                self.off(i.name);
                said.push(format!("cheats: {} switched off: {why}", i.name));
            }
        }
        if !self.busy() {
            return Ok(said);
        }
        let globals = m.u32(GAME_GLOBALS)?;
        let playing = globals != 0 && PLAYING.contains(&m.u32(globals.wrapping_add(GLOBALS_MODE))?);
        if !playing {
            // The menus: a new map resets the speed, the HUD and the AI,
            // and the units go. What is on waits for the next game.
            self.flagged.clear();
            self.sped = false;
            self.pilots.clear();
            self.hud = Held::default();
            self.ai = Held::default();
            return Ok(said);
        }
        self.time(m, globals)?;
        self.hud(m)?;
        self.ai(m)?;
        let units = local_units(m)?;
        let dt = self.tick_seconds(m)?;
        said.extend(self.flight(m, &units, dt)?);
        self.units(m, &units)?;
        Ok(said)
    }

    fn tick_seconds(&self, m: &dyn GuestMemory) -> Result<f32, MemFault> {
        let t = m.u32(guest_layout::points::GAME_TIME.base)?;
        if t == 0 {
            return Ok(DEFAULT_TICK);
        }
        let s = m.f32(t.wrapping_add(TIME_TICK_SECONDS))?;
        Ok(if s.is_finite() && s > 0.0 && s < 1.0 {
            s
        } else {
            DEFAULT_TICK
        })
    }

    fn time(&mut self, m: &mut dyn GuestMemory, globals: u32) -> Result<(), MemFault> {
        if self.speed == 1.0 && !self.sped {
            return Ok(());
        }
        let t = m.u32(guest_layout::points::GAME_TIME.base)?;
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

    /// Hide HUD: the shown flag held at 0. A value the game writes while
    /// we hold it (a script showing or hiding the HUD) is what goes back.
    fn hud(&mut self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        hold_zero(m, &mut self.hud, self.hide_hud, HUD_GLOBALS, HUD_SHOWN)
    }

    /// AI off: the AI globals' on byte held at 0. A value the game writes
    /// while we hold it (a script's `ai` switch) is what goes back.
    fn ai(&mut self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        hold_zero(m, &mut self.ai, self.ai_off, guest_layout::points::AI_GLOBALS.base, AI_ON)
    }

    /// Fly and noclip for each local biped on foot.
    fn flight(
        &mut self,
        m: &mut dyn GuestMemory,
        units: &[LocalUnit],
        dt: f32,
    ) -> Result<Vec<String>, MemFault> {
        let mut said = Vec::new();
        if !self.fly && !self.noclip && self.pilots.values().all(Pilot::idle) {
            self.pilots.clear();
            return Ok(said);
        }
        for u in units {
            if !flight::on_foot(m, u)? {
                // In a vehicle or a seat: the vehicle moves it.
                self.pilots.remove(&u.unit);
                continue;
            }
            let p = self.pilots.entry(u.unit).or_default();
            if let Some(line) = flight::step(m, u, p, self.fly, self.noclip, dt)? {
                said.push(line);
            }
        }
        // Units gone (dead, a new map): nothing to fly.
        self.pilots
            .retain(|d, p| units.iter().any(|u| u.unit == *d) && !(p.idle() && !self.fly));
        Ok(said)
    }

    fn units(&mut self, m: &mut dyn GuestMemory, units: &[LocalUnit]) -> Result<(), MemFault> {
        for u in units {
            let b = u.body;
            let at = b.wrapping_add(BODY_DAMAGE_FLAGS);
            let f = m.u16(at)?;
            let shield = self.god || self.pilots.get(&u.unit).is_some_and(Pilot::shielded);
            if shield {
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
        // Units gone (dead, a new map): nothing left to undo on them.
        self.flagged.retain(|d| units.iter().any(|u| u.unit == *d));
        Ok(())
    }
}

/// A byte at `[pointer] + field` held at 0 while `on`: the value the game
/// last wrote is kept in `held` and put back once `on` goes, unless the
/// game changed it meanwhile. A null pointer (no globals yet) forgets it.
fn hold_zero(
    m: &mut dyn GuestMemory,
    held: &mut Held,
    on: bool,
    pointer: u32,
    field: u32,
) -> Result<(), MemFault> {
    if !on && held.game.is_none() {
        return Ok(());
    }
    let g = m.u32(pointer)?;
    if g == 0 {
        *held = Held::default();
        return Ok(());
    }
    let at = g.wrapping_add(field);
    let now = m.u8(at)?;
    if on {
        if now != 0 || held.game.is_none() {
            held.game = Some(now);
        }
        if now != 0 {
            m.set_u8(at, 0)?;
        }
    } else if let Some(v) = held.game.take()
        && now == 0
    {
        m.set_u8(at, v)?;
    }
    Ok(())
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

/// Live as the cheats need it: who is signed in, by gamertag.
pub type LiveProbe = Box<dyn Fn() -> policy::Live + Send + Sync>;

/// The Live probe over the Live client: its signed-in users, named from
/// this machine's accounts file (re-read only when the users change).
pub fn live_probe(hle: Option<Arc<live_hle::Hle>>) -> LiveProbe {
    let Some(hle) = hle else {
        return Box::new(|| policy::Live::Off);
    };
    let names: Mutex<(Vec<u64>, HashMap<u64, String>)> = Mutex::default();
    Box::new(move || {
        let users: Vec<u64> = (0..4).map(|c| hle.user(c)).collect();
        let mut n = names.lock().unwrap_or_else(|e| e.into_inner());
        if n.0 != users {
            let entries = hle
                .config()
                .accounts_path
                .as_deref()
                .and_then(|p| live_hle::account_setup::load(p).ok())
                .unwrap_or_default();
            *n = (
                users.clone(),
                entries
                    .into_iter()
                    .map(|e| (e.xuid.0, e.gamertag))
                    .collect(),
            );
        }
        policy::live_from(&users, |x| n.1.get(&x).cloned())
    })
}

/// The cheats' after-tick hook.
pub struct CheatsFeature(pub Arc<Mutex<Cheats>>, pub Arc<Policy>);

/// What decides the refusals: the run and Live.
pub struct Policy {
    pub lockstep: bool,
    pub live: LiveProbe,
}

impl Policy {
    pub fn refusals(&self, m: &dyn GuestMemory, installed: bool) -> Refusals {
        if !installed {
            return Refusals::every("the cheats feature is switched off (H2_FEATURES)");
        }
        policy::refusals(&Session {
            sim_mode: sim_mode(m),
            lockstep: self.lockstep,
            live: (self.live)(),
        })
    }
}

fn lock(c: &Mutex<Cheats>) -> std::sync::MutexGuard<'_, Cheats> {
    c.lock().unwrap_or_else(|e| e.into_inner())
}

/// Why `cheats` can't be snapshotted now (docs/runtime/snapshots.md): a
/// cheat is on or has something left to undo (units we flagged, a game
/// speed we set, the HUD flag we hold, a flight). That is host state a
/// load would not put back. With nothing on and nothing to undo the tick
/// hook does nothing.
pub fn snapshot_live(c: &Mutex<Cheats>) -> Option<String> {
    lock(c)
        .busy()
        .then(|| "a cheat is on or has something to undo".into())
}

impl hooks::Feature for CheatsFeature {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            FEATURE,
            "Our cheats: god mode, ammo, game speed, fly, noclip, HUD, AI (debug menu)",
        )
        .doc("docs/runtime/debug-menu.md")
    }
    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::points::GAME_TICK;
        let c = Arc::clone(&self.0);
        let policy = Arc::clone(&self.1);
        b.raw()
            .after(&GAME_TICK, "our cheats", move |cx| {
                let mut c = lock(&c);
                if !c.busy() {
                    return Ok(());
                }
                let rt = Arc::clone(cx.ctx.runtime());
                let mut mem = rt.memory();
                let m: &mut dyn GuestMemory = &mut *mem;
                let refused = policy.refusals(m, true);
                match c.tick(m, &refused) {
                    Ok(said) => said.iter().for_each(|l| eprintln!("{l}")),
                    Err(e) => eprintln!("cheats: {e:?}"),
                }
                Ok(())
            })
            // touch-glue and skulls-custom also hook after the tick. Cheats run
            // outside them (after them), so a cheat's writes (health, ammo, speed,
            // position) are what the next tick starts from, whatever they set.
            // The Halo 1 weapon HUD's sample runs outside cheats and only reads,
            // so it shows the state a cheat left (a bottomless-clip refill).
            .after_feature(crate::touch_glue::FEATURE)
            .after_feature(crate::skulls_custom::FEATURE)
            .after_feature(crate::h1_hud_text::FEATURE);
        let live = Arc::clone(&self.0);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            snapshot_live(&live)
        }));
        Ok(())
    }
}

/// Adds the cheats to `registry` and `cheat.list`/`cheat.set` to `agent`.
/// `lockstep`: a lockstep run, which refuses them; `live`: the Live
/// client, whose signed-in gamertags decide on Live.
pub fn install(
    registry: &mut hooks::Registry,
    agent: &agent::Agent,
    rt: &Arc<runtime::Runtime>,
    lockstep: bool,
    live: Option<Arc<live_hle::Hle>>,
) -> anyhow::Result<bool> {
    let cheats = Arc::new(Mutex::new(Cheats::default()));
    let policy = Arc::new(Policy {
        lockstep,
        live: live_probe(live),
    });
    let added = registry
        .add(&CheatsFeature(Arc::clone(&cheats), Arc::clone(&policy)))
        .map_err(|e| anyhow::anyhow!("cheats: {e}"))?;
    lock(&cheats).installed = added;
    let (c, r, p) = (Arc::clone(&cheats), Arc::clone(rt), Arc::clone(&policy));
    agent.add_method(
        debug_menu::sources::method::CHEAT_LIST,
        Access::Read,
        move |_| {
            let c = lock(&c);
            let refused = p.refusals(&*r.memory(), c.installed);
            let in_game = playing(&*r.memory());
            Ok(c.list(&refused, in_game))
        },
    );
    let (c, r, p) = (Arc::clone(&cheats), Arc::clone(rt), Arc::clone(&policy));
    agent.add_method(
        debug_menu::sources::method::CHEAT_SET,
        Access::Act,
        move |params| {
            let mut c = lock(&c);
            let refused = p.refusals(&*r.memory(), c.installed);
            let in_game = playing(&*r.memory());
            c.set(&params, &refused, in_game)
        },
    );
    if !added {
        eprintln!("cheats: off: switched off (H2_FEATURES)");
    }
    Ok(added)
}

#[cfg(test)]
mod tests;
