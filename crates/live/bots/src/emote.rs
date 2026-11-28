//! Emotes, the teabag layer (docs/game/bots.md §3.5): after a dumb death
//! near it, a bot may crouch a few times over the body, with the real crouch
//! input, at a human rhythm.
//!
//! [`classify`] says which deaths are dumb; [`Emoter`] holds the gates (body
//! near, no enemy in view for a while, no live objective, not too often),
//! rolls `taunt × severity` once per death and plays the burst. Deaths come
//! from the game's kill function through a process-wide [`KillLog`], which
//! each brain reads from its own cursor.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};

use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::view::{objects, players};

use crate::brain::Vec3;

/// One death, as the game's kill function (`PLAYER_KILLED`) reports it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Kill {
    /// Its place in the log.
    pub seq: u64,
    pub tick: u32,
    /// The killer's player index; `None` when no player killed (the
    /// world).
    pub attacker: Option<u32>,
    pub victim: u32,
    /// A teammate killed in a team game ([`kill_from_guest`]).
    pub betrayal: bool,
    /// Where the body is: the victim's unit when it died.
    pub at: Option<Vec3>,
}

/// A death worth an emote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dumb {
    /// Killed by themselves or by the world: their own grenade, a fall, a
    /// suicide. The kill function does not tell these apart.
    SelfKill,
    /// Killed a teammate.
    Betrayal,
}

impl Dumb {
    /// How dumb, 0 to 1: scales the chance.
    pub fn severity(self) -> f32 {
        match self {
            Dumb::SelfKill => 0.8,
            Dumb::Betrayal => 1.0,
        }
    }
}

/// Which deaths are dumb. Not yet: stuck on geometry, point-blank misses,
/// out of ammo mid-fight and the lost 1v1 with a big advantage (they need
/// the shots hook, ammo and shields: §3.5).
pub fn classify(k: &Kill) -> Option<Dumb> {
    if k.betrayal {
        return Some(Dumb::Betrayal);
    }
    match k.attacker {
        None => Some(Dumb::SelfKill),
        Some(a) if a == k.victim => Some(Dumb::SelfKill),
        Some(_) => None,
    }
}

/// The last deaths, for every brain to read from its own cursor.
#[derive(Default)]
pub struct KillLog {
    kills: Mutex<(u64, VecDeque<Kill>)>,
}

/// Deaths kept: far more than a game makes between two frames.
const KEPT: usize = 64;

impl KillLog {
    /// Logs a death; its `seq` is set here.
    pub fn push(&self, mut k: Kill) {
        let mut g = self.kills.lock().unwrap_or_else(|e| e.into_inner());
        g.0 += 1;
        k.seq = g.0;
        g.1.push_back(k);
        if g.1.len() > KEPT {
            g.1.pop_front();
        }
    }

    /// The deaths after `cursor`, and the new cursor.
    pub fn since(&self, cursor: u64) -> (Vec<Kill>, u64) {
        let mut out = Vec::new();
        let tick = self.since_into(cursor, &mut out);
        (out, tick)
    }

    pub(crate) fn since_into(&self, cursor: u64, out: &mut Vec<Kill>) -> u64 {
        let g = self.kills.lock().unwrap_or_else(|e| e.into_inner());
        out.clear();
        out.extend(g.1.iter().filter(|k| k.seq > cursor).copied());
        g.0
    }
}

/// The process-wide log the boot glue fills.
pub fn kills() -> &'static KillLog {
    static G: OnceLock<KillLog> = OnceLock::new();
    G.get_or_init(KillLog::default)
}

/// A death read from the kill function's attacker and victim player
/// datums (`u32::MAX` for none). The body's place is the victim's unit,
/// which is still there.
///
/// A betrayal is a teammate killed in a team game (the variant's team
/// flag), as Live telemetry tells it (`live_proto::game::is_betrayal`).
/// The kill function's third argument is not a betrayal flag: it is set in
/// free-for-all too, so it is not read.
pub fn kill_from_guest(
    m: &dyn GuestMemory,
    attacker: u32,
    victim: u32,
) -> Result<Option<Kill>, MemFault> {
    let Some(v) = DatumHandle::from_raw(victim) else {
        return Ok(None);
    };
    let a = DatumHandle::from_raw(attacker);
    let mut at = None;
    let mut betrayal = false;
    if let Some(array) = players::array(m)?
        && let Some(p) = array.get(m, v)?
    {
        at = match p.unit(m)? {
            Some(u) => objects::get(m, u)?
                .map(|(_, b)| b.position(m))
                .transpose()?,
            None => None,
        };
        if let Some(killer) = a.filter(|a| a.index() != v.index())
            && team_game(m)?
            && let Some(k) = array.get(m, killer)?
        {
            betrayal = k.team(m)? == p.team(m)?;
        }
    }
    Ok(Some(Kill {
        seq: 0,
        tick: crate::guest::game_tick(m).unwrap_or(0),
        attacker: a.map(|h| u32::from(h.index())),
        victim: u32::from(v.index()),
        betrayal,
        at,
    }))
}

/// A multiplayer game whose variant is a team game.
fn team_game(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    use crate::guest::{MODE_MULTIPLAYER, OPTIONS, VARIANT_FLAGS};
    let globals = m.u32(guest_layout::points::GAME_GLOBALS.base)?;
    Ok(globals != 0
        && m.u32(globals + OPTIONS)? == MODE_MULTIPLAYER
        && m.u32(globals + OPTIONS + VARIANT_FLAGS)? & 1 != 0)
}

/// The taunt of a bot with no personality of its own (§9.1's default).
pub const DEFAULT_TAUNT: f32 = 0.3;
/// The body within 1.5 world units (about 4.5 m): the game's teabag
/// radius (`game::teabag::RADIUS`, docs/game/teabag.md), so every emote is
/// a teabag the incel medal sees.
pub const NEAR: f32 = 1.5;
/// No enemy in view for 1.5 s.
pub const CLEAR_TICKS: u32 = 45;
/// At most one emote in 30 s.
pub const COOLDOWN_TICKS: u32 = 900;
/// A death stays worth an emote for 5 s while the gates close.
pub const WINDOW_TICKS: u32 = 150;
/// A burst lasts 2 s at most.
pub const BURST_TICKS: u32 = 60;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Burst {
    start: u32,
    presses: u32,
    /// Ticks per press: 4 to 6 (130 to 200 ms).
    period: u32,
}

/// One bot's emotes.
#[derive(Clone, Debug)]
pub struct Emoter {
    /// 0 to 1; 0 never (the Clankers page's Taunts off).
    pub taunt: f32,
    rng: u64,
    /// The body to emote over, and until when.
    body: Option<(Vec3, u32)>,
    burst: Option<Burst>,
    last: Option<u32>,
    enemy_at: Option<u32>,
}

impl Emoter {
    pub fn new(taunt: f32, seed: u64) -> Self {
        Emoter {
            taunt,
            rng: seed | 1,
            body: None,
            burst: None,
            last: None,
            enemy_at: None,
        }
    }

    fn uniform(&mut self) -> f32 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        (self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Hears deaths: each dumb one with a body rolls `taunt × severity`
    /// once; a hit makes it the body to emote over.
    pub fn hear(&mut self, kills: &[Kill]) {
        self.hear_with_style(kills, None, 1.0);
    }

    pub(crate) fn hear_with_style(&mut self, kills: &[Kill], player: Option<u32>, weight: f32) {
        for k in kills.iter().filter(|k| Some(k.victim) != player) {
            let severity = classify(k)
                .map(Dumb::severity)
                .or_else(|| (player.is_some() && k.attacker == player).then_some(0.35));
            let (Some(severity), Some(at)) = (severity, k.at) else {
                continue;
            };
            let chance = (self.taunt * weight * severity).clamp(0.0, 1.0);
            if self.uniform() < chance {
                self.body = Some((at, k.tick.saturating_add(WINDOW_TICKS)));
            }
        }
    }

    /// Whether it is emoting.
    pub fn busy(&self) -> bool {
        self.burst.is_some()
    }

    /// One frame: `Some(crouch)` while a burst plays (the pad's crouch, and
    /// the bot should stand still), `None` otherwise. `me` is the bot's
    /// position, `enemy_in_view` whether it sees an enemy now.
    pub fn step(
        &mut self,
        me: Vec3,
        enemy_in_view: bool,
        objective_live: bool,
        tick: u32,
    ) -> Option<bool> {
        if enemy_in_view {
            self.enemy_at = Some(tick);
            // An enemy: stop at once.
            if self.burst.take().is_some() {
                self.last = Some(tick);
            }
            return None;
        }
        if self.taunt <= 0.0 {
            self.body = None;
        }
        if self.taunt <= 0.0 || objective_live {
            if self.burst.take().is_some() {
                self.last = Some(tick);
            }
            return None;
        }
        if let Some(b) = self.burst {
            let t = tick.wrapping_sub(b.start);
            if t >= b.presses * b.period || t >= BURST_TICKS {
                self.burst = None;
                self.last = Some(tick);
                return None;
            }
            return Some(t % b.period < b.period / 2);
        }
        let (at, until) = self.body?;
        if tick > until {
            self.body = None;
            return None;
        }
        let near = {
            let d = [at[0] - me[0], at[1] - me[1], at[2] - me[2]];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() <= NEAR
        };
        let clear = self
            .enemy_at
            .is_none_or(|e| tick.wrapping_sub(e) >= CLEAR_TICKS);
        let rested = self
            .last
            .is_none_or(|l| tick.wrapping_sub(l) >= COOLDOWN_TICKS);
        if !(near && clear && rested) {
            return None;
        }
        self.body = None;
        let presses = 3 + (self.uniform() * 4.0) as u32 % 4;
        let period = 4 + (self.uniform() * 3.0) as u32 % 3;
        self.burst = Some(Burst {
            start: tick,
            presses,
            period,
        });
        Some(true)
    }
}

#[cfg(test)]
mod tests;
