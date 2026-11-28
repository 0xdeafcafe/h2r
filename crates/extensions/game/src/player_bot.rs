//! `player.bot`: hand `player.0`, the local human seat, to a Clanker brain
//! for benchmarks and spectating, and back (docs/runtime/agent-api-sim.md,
//! "`player.bot`: a Clanker plays the player's seat").
//!
//! The brain is the one phantoms run ([`bots::Brain`]), with the
//! personality hashed from the seat's own name, as a phantom's is from its
//! gamertag. It is not a bot seat: nothing signs in, no bot account is
//! made, and the player and the game record stay the player's. The brain
//! only holds the pad.
//!
//! It plays through the pad tap ([`crate::superbounce_assist::AssistTap`],
//! the one `player.goto` drives): at each read of the first controller
//! the tap calls [`pad_read`], which finds the seat on that port in a
//! multiplayer game, samples the brain once per game tick (its view read
//! from guest memory by the seat's machine id, as a phantom's is) and
//! gives the game the brain's pad in place of the one held. Outside a
//! multiplayer game the real pad goes through. A real press while the
//! brain plays hands the seat back at once; so does `on: false`, or the
//! `player.bot` setting turned off. The map's nav mesh is asked for as
//! `player.goto` asks for it ([`crate::player_goto::nav_for`]), so the
//! brain has it with no phantom in the game.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use agent::{Access, GameCtx};
use agent_proto::player_bot::{self as wire, BotParams, BotStatus};
use agent_proto::{RpcError, code};
use bots::mind::Skill;
use bots::pad_source::{Frame, PadSource};
use guest_layout::memory::GuestMemory;
use guest_layout::view::players;
use input::gamepad::Gamepad;
use serde_json::Value;

/// The controller port whose pad the brain holds: the first local player.
pub const PORT: u32 = crate::superbounce_assist::PORT;
/// A real stick this far from centre (either way, either axis) is a press.
pub const STICK_PRESS: i16 = i16::MAX / 2;
/// A real face button or trigger this far down is a press.
pub const BUTTON_PRESS: u8 = crate::superbounce_assist::PRESSED;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---- what is asked for ----

/// The Clankers page's Aggression.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    Calm,
    #[default]
    Normal,
    Wild,
}

impl Style {
    pub fn parse(s: &str) -> Option<Style> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "calm" => Style::Calm,
            "normal" => Style::Normal,
            "wild" => Style::Wild,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Style::Calm => "calm",
            Style::Normal => "normal",
            Style::Wild => "wild",
        }
    }

    /// The push bias a Clanker of this Aggression plays with
    /// (`clankers::Clanker::follow_page`).
    pub fn push_bias(self) -> f32 {
        match self {
            Style::Calm => -0.35,
            Style::Normal => 0.0,
            Style::Wild => 0.35,
        }
    }
}

fn skill_name(s: Skill) -> &'static str {
    match s {
        Skill::Easy => "easy",
        Skill::Normal => "normal",
        Skill::Heroic => "heroic",
        Skill::Legendary => "legendary",
    }
}

/// How the brain plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ask {
    pub skill: Skill,
    pub style: Style,
}

/// The ask `p` makes.
pub fn ask_for(p: &BotParams) -> Result<Ask, String> {
    let skill = match p.difficulty.as_deref() {
        None => Skill::Normal,
        Some(d) => Skill::parse(d)
            .ok_or_else(|| format!("difficulty {d:?}: easy, normal, heroic or legendary"))?,
    };
    let style = match p.style.as_deref() {
        None => Style::Normal,
        Some(s) => Style::parse(s).ok_or_else(|| format!("style {s:?}: calm, normal or wild"))?,
    };
    Ok(Ask { skill, style })
}

// ---- the seat ----

/// What the brain sees of its seat at a pad read.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    /// The game's tick.
    pub tick: u32,
    /// The seat's machine id, which the brain finds its player by.
    pub machine: [u8; 6],
    /// The seat's name, which its personality is hashed from.
    pub name: String,
    /// The loaded map, for its nav mesh.
    pub map: Option<String>,
}

/// The name a seat with none plays under.
pub const NO_NAME: &str = "Player";

/// The seat on [`PORT`] in a multiplayer game: `Ok(None)` with no such
/// game or seat (menus, a load, campaign), `Err` when the brain can't play
/// it: another listed player shares its machine (split screen), and the
/// brain, which finds itself by machine, can't tell them apart.
pub fn view(m: &dyn GuestMemory) -> Result<Option<View>, String> {
    if bots::guest::game_mode(m) != Some(bots::guest::MODE_MULTIPLAYER) {
        return Ok(None);
    }
    let Some(tick) = bots::guest::game_tick(m) else {
        return Ok(None);
    };
    let read = || -> Result<Option<([u8; 6], String, usize)>, guest_layout::memory::MemFault> {
        let Some(array) = players::array(m)? else {
            return Ok(None);
        };
        let mut seat = None;
        let mut machines = Vec::new();
        for entry in array.live_iter(m, 16)? {
            let (_, p) = entry?;
            if !p.listed(m)? {
                continue;
            }
            let machine = p.machine(m)?;
            if p.local_user(m)? == PORT as i16 && seat.is_none() {
                seat = Some((machine, p.name(m).unwrap_or_default()));
            }
            machines.push(machine);
        }
        Ok(seat.map(|(machine, name)| {
            let sharing = machines.iter().filter(|x| **x == machine).count();
            (machine, name, sharing)
        }))
    };
    match read() {
        Ok(Some((machine, name, sharing))) => {
            if sharing > 1 {
                return Err(format!(
                    "player.0 shares its machine with {} other local player(s) (split screen)",
                    sharing - 1
                ));
            }
            let name = if name.trim().is_empty() {
                NO_NAME.to_owned()
            } else {
                name
            };
            let map = crate::health::loaded_map_name(m)
                .ok()
                .filter(|n| !n.is_empty());
            Ok(Some(View {
                tick,
                machine,
                name,
                map,
            }))
        }
        Ok(None) | Err(_) => Ok(None),
    }
}

/// Whether `pad`, what the player holds, has a press in it: a button, a
/// trigger or a stick well off centre.
pub fn pressed(pad: &Gamepad) -> bool {
    pad.buttons.0 != 0
        || pad.analog.iter().any(|&a| a >= BUTTON_PRESS)
        || [pad.thumb_lx, pad.thumb_ly, pad.thumb_rx, pad.thumb_ry]
            .iter()
            .any(|&v| v.unsigned_abs() >= STICK_PRESS as u16)
}

/// The brain for a seat named `name`: a phantom's, its personality from
/// the name ([`bots::name_profile::NameProfile::for_name`], seeded by it
/// too, so the same name plays the same way).
pub fn brain(ask: Ask, name: &str) -> Box<dyn PadSource> {
    let seed = bots::name_profile::NameProfile::for_name(name).seed;
    let mut b = bots::Brain::new(ask.skill, seed).with_name_profile(name);
    b.set_clanker_style(ask.style.push_bias(), false);
    Box::new(b)
}

/// Where the pad comes from and how the seat is read: the brain and the
/// guest in a game, stand-ins in tests.
#[derive(Clone, Copy)]
struct Hooks {
    make: fn(Ask, &str) -> Box<dyn PadSource>,
    view: fn(&dyn GuestMemory) -> Result<Option<View>, String>,
}

const GAME: Hooks = Hooks { make: brain, view };

struct Seat {
    ask: Ask,
    hooks: Hooks,
    source: Option<Box<dyn PadSource>>,
    name: Option<String>,
    map: Option<String>,
    nav: bool,
    /// Samples taken: the brain's frame number.
    frame: u64,
    tick: Option<u32>,
    /// The pad given at the last sample, again for every read in its tick.
    pad: Gamepad,
    driving: bool,
}

impl Seat {
    fn new(ask: Ask, hooks: Hooks) -> Seat {
        Seat {
            ask,
            hooks,
            source: None,
            name: None,
            map: None,
            nav: false,
            frame: 0,
            tick: None,
            pad: Gamepad::default(),
            driving: false,
        }
    }

    /// Out of a game: the brain's input stream ends.
    fn idle(&mut self) {
        if std::mem::take(&mut self.driving)
            && let Some(s) = &mut self.source
        {
            s.reset_input();
        }
        self.tick = None;
    }

    /// The pad for a read at `v`.
    fn sample(&mut self, v: &View, m: &dyn GuestMemory) -> Gamepad {
        if self.tick == Some(v.tick) {
            return self.pad;
        }
        self.tick = Some(v.tick);
        self.frame += 1;
        if self.map != v.map {
            self.map.clone_from(&v.map);
            self.nav = false;
        }
        if !self.nav
            && let Some(map) = &self.map
        {
            let file = crate::map_slots::active_file(&crate::layout_glue::ReadGm(m));
            self.nav = matches!(
                crate::player_goto::nav_for(map, file),
                crate::player_goto::Nav::Ready(_)
            );
        }
        if self.name.as_deref() != Some(v.name.as_str()) {
            self.name = Some(v.name.clone());
            self.source = None;
        }
        let (ask, make) = (self.ask, self.hooks.make);
        let source = self.source.get_or_insert_with(|| make(ask, &v.name));
        let host = source.pad(&Frame {
            frame: self.frame,
            tick: v.tick,
            machine: v.machine,
            guest: m,
        });
        self.pad = host.to_gamepad(&Default::default(), false);
        self.driving = true;
        self.pad
    }
}

#[derive(Default)]
struct Books {
    seat: Option<Seat>,
    /// Ticks played since it was turned on.
    ticks: u64,
    off: Option<String>,
    /// The last ask, for the status once off.
    last: Ask,
}

static BOOKS: Mutex<Books> = Mutex::new(Books {
    seat: None,
    ticks: 0,
    off: None,
    last: Ask {
        skill: Skill::Normal,
        style: Style::Normal,
    },
});
/// The brain has the seat: the tap's one check when it hasn't.
static ON: AtomicBool = AtomicBool::new(false);

/// Whether the brain has `player.0`'s pad.
pub fn active() -> bool {
    ON.load(Ordering::Relaxed)
}

fn turn_on_with(ask: Ask, hooks: Hooks) {
    let mut b = lock(&BOOKS);
    let same = b.seat.as_ref().is_some_and(|s| s.ask == ask);
    if !same {
        b.seat = Some(Seat::new(ask, hooks));
        b.ticks = 0;
    }
    b.last = ask;
    b.off = None;
    ON.store(true, Ordering::Relaxed);
}

/// Hands the seat to the brain (a new ask starts a new brain).
pub fn turn_on(ask: Ask) {
    turn_on_with(ask, GAME);
}

/// Hands the seat back, saying why.
pub fn turn_off(why: &str) {
    let mut b = lock(&BOOKS);
    off(&mut b, why);
}

fn off(b: &mut Books, why: &str) {
    if b.seat.take().is_some() {
        b.off = Some(why.to_owned());
    }
    ON.store(false, Ordering::Relaxed);
}

/// Where the seat stands.
pub fn status() -> BotStatus {
    let b = lock(&BOOKS);
    let ask = b.seat.as_ref().map_or(b.last, |s| s.ask);
    BotStatus {
        on: b.seat.is_some(),
        driving: b.seat.as_ref().is_some_and(|s| s.driving),
        difficulty: skill_name(ask.skill).into(),
        style: ask.style.name().into(),
        name: b.seat.as_ref().and_then(|s| s.name.clone()),
        ticks: b.ticks,
        off: b.off.clone(),
    }
}

/// At a read of [`PORT`]'s pad (the tap): `real` is what the player
/// holds. The pad to give the game in its place, or `None` to let the
/// real one through: the brain is off, there is no multiplayer game, or
/// the press in `real` has just handed the seat back.
pub fn pad_read(real: &Gamepad, m: &dyn GuestMemory) -> Option<Gamepad> {
    if !active() {
        return None;
    }
    let mut b = lock(&BOOKS);
    let seat = b.seat.as_mut()?;
    let v = match (seat.hooks.view)(m) {
        Ok(Some(v)) => v,
        Ok(None) => {
            seat.idle();
            return None;
        }
        Err(why) => {
            off(&mut b, &why);
            return None;
        }
    };
    if pressed(real) {
        off(&mut b, "pad");
        return None;
    }
    let fresh = seat.tick != Some(v.tick);
    let pad = seat.sample(&v, m);
    if fresh {
        b.ticks += 1;
    }
    Some(pad)
}

// ---- the setting ----

/// Makes the brain follow the `player.bot` setting in `store`
/// ([`crate::setting_defs::PLAYER_BOT`]): on now if it is, then on or off
/// at each change. A real press still hands the seat back.
pub fn follow_setting(store: &settings::Store) {
    let key = crate::setting_defs::PLAYER_BOT.key;
    if store.handle::<bool>(key).get() {
        turn_on(Ask::default());
    }
    store.on_change(key, |v| match v {
        settings::Value::Bool(true) => turn_on(Ask::default()),
        _ if active() => turn_off("setting"),
        _ => {}
    });
}

// ---- the method ----

fn invalid(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::INVALID_PARAMS, msg)
}

fn unavailable(msg: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, msg)
}

/// What a call asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// No `on`: the state only.
    Read,
    Off,
    On(Ask),
}

/// What `p` asks for.
pub fn want(p: &BotParams) -> Result<Want, String> {
    match p.on {
        Some(true) => ask_for(p).map(Want::On),
        _ if p.difficulty.is_some() || p.style.is_some() => {
            Err("difficulty and style go with on: true".into())
        }
        Some(false) => Ok(Want::Off),
        None => Ok(Want::Read),
    }
}

/// `player.bot {on?, difficulty?, style?}`.
fn bot(v: Value, ctx: &mut GameCtx<'_>) -> Result<Value, RpcError> {
    let p: BotParams = if v.is_null() {
        BotParams::default()
    } else {
        serde_json::from_value(v).map_err(|e| invalid(format!("{}: {e}", wire::BOT)))?
    };
    match want(&p).map_err(|e| invalid(format!("{}: {e}", wire::BOT)))? {
        Want::Read => {}
        Want::Off => {
            if active() {
                turn_off("agent");
            }
        }
        Want::On(ask) => {
            if !crate::superbounce_assist::tapped() {
                return Err(unavailable(
                    "the pad tap is not in place (the superbounce-assist feature is off), so \
                     player.0 can't be driven",
                ));
            }
            if crate::player_goto::running() {
                return Err(invalid(
                    "a player.goto drive holds player.0; player.goto_cancel stops it",
                ));
            }
            if crate::superbounce_verify::drives_pad() {
                return Err(invalid(
                    "a superbounce.verify batch is driving player.0; superbounce.verify_cancel stops it",
                ));
            }
            ctx.read_guest(view)?.map_err(unavailable)?;
            turn_on(ask);
        }
    }
    Ok(serde_json::to_value(status()).expect("player.bot serialises"))
}

/// Adds the method to `agent`.
pub fn install(agent: &agent::Agent) {
    agent.add_game_method(wire::BOT, Access::Act, bot);
}

#[cfg(test)]
mod tests;
