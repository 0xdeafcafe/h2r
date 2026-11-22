//! Priming a super bounce (docs/game/superbounce.md, "The pair word"):
//! standing up out of a forced crouch leaves the player's level-pair word
//! at -3, and only a landing with the word at -3 bounces.
//!
//! The forced crouch needs a ceiling between the crouched and the standing
//! capsule heights (a `superbounce` crouch spot). The bot crouches under
//! it, lets go and the body cannot stand: the engine keeps the crouch
//! request and the word goes to -3. It stays there until the pair is
//! rebuilt (a crouch, or the body going to sleep), so the bot must not
//! crouch again until it has bounced.
//!
//! With the crouch spot's way out known ([`PrimeRun::at`]) the bot does it
//! as the measured recipe does (docs/game/superbounce.md, "The forced crouch leaves state", the recipe):
//! crouch, walk in, release, walk out. From a start outside the lip, a
//! little out along the way out, it presses crouch and waits for the
//! crouch to go down, crouch-walks in under the lip, holds, lets go while
//! still pushing in (the request stays set: the standing capsule does not
//! fit), then walks out along the way out until the word reads -3, where
//! the crouch lets go at the band's edge.
//! A try that does not turn the word walks back out and comes in again
//! from another angle. Without it ([`PrimeRun::new`]) it crouches and
//! lets go where it stands.
//!
//! Pure and deterministic; time in game ticks (30 a second).

use crate::brain::Vec3;

/// The pair word that primes a landing: -3, as the game stores it.
pub const PRIMED_WORD: u32 = (-3i32) as u32;

/// Whether `word` (the player's level pair word) is the primed one.
pub fn is_primed(word: u32) -> bool {
    word == PRIMED_WORD
}

/// Crouch held this long (ticks) under the ceiling: the crouch amount
/// moves at 5 a second, a full crouch in 6.
pub const CROUCH_TICKS: u32 = 12;
/// After letting go, the stand-up that gets refused and sets the word
/// takes this long (ticks): 6 for the stand-up rate, with a margin.
pub const FORCE_TICKS: u32 = 12;
/// Not read as -3 by then (ticks after letting go) is a failed try, when
/// the word can be read.
pub const READ_TICKS: u32 = 30;
/// Tries at the crouch spot before giving up.
pub const PRIME_TRIES: u32 = 2;
/// With the way out known, tries before giving up: one an approach angle
/// ([`APPROACH_TURNS`]).
pub const WALK_TRIES: u32 = 3;
/// Each try comes in turned this much (degrees) from straight in along
/// the way out: straight, then to one side, then the other.
pub const APPROACH_TURNS: [f32; 3] = [0.0, 25.0, -25.0];
/// A try starts this far (world units) out from the stand.
pub const START_OUT: f32 = 0.35;
/// The crouch-walk in aims this far past the stand, into the lip.
pub const PUSH_IN: f32 = 0.06;
/// Within this of the stand (world units) the bot is in under the lip.
pub const IN_NEAR: f32 = 0.08;
/// Within this of a try's start the walk back to it is over.
pub const BACK_NEAR: f32 = 0.08;
/// The walk back to the start and the crouch-walk in each give up after
/// this long (ticks) and go on.
pub const MOVE_TICKS: u32 = 60;
/// After letting go, still pushing in this long (ticks), so the refused
/// stand-up happens under the lip.
pub const RELEASE_TICKS: u32 = 6;
/// Walking out: with no word to read, primed is assumed this far out
/// (world units) from where it let go.
pub const OUT_DIST: f32 = 0.5;
/// The stick while crouch-walking in under the lip (0..1): slow, as the
/// recipe eases into the corner.
pub const IN_SPEED: f32 = 0.6;
/// Crouch is pressed and held this long (ticks) at the start, standing
/// still, before the walk in: the crouch amount moves at 5 a second, so
/// the bot is crouched before it reaches the lip.
pub const CROUCH_FIRST: u32 = 6;

/// How the priming ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Primed {
    /// The word was read at -3.
    Read,
    /// The word cannot be read: primed because the crouch and the
    /// stand-up were done under the ceiling, not because it was seen.
    Assumed,
    /// The word was readable and never went to -3.
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Crouch held, from the tick.
    Crouch(u32),
    /// Crouch let go, from the tick; the try's number.
    Stand(u32, u32),
    Done(Primed),
}

/// What priming asks of the pad this tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PrimeOut {
    pub crouch: bool,
    /// The walk this tick: a world direction (X and Y) times the stick,
    /// 0..1; `None` stands still. Only with the way out known.
    pub walk: Option<[f32; 2]>,
    /// Over, and how.
    pub done: Option<Primed>,
}

/// Where a walking prime happens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geo {
    pub stand: Vec3,
    /// The way out, unit X and Y.
    pub out: [f32; 2],
    /// A verified start: offset from `stand` (X and Y) and the approach yaw
    /// (degrees, the direction walked in).
    pub start: Option<([f32; 2], Option<f32>)>,
}

/// The phase of a walking prime, by name, for the agent.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Walking {
    /// Walking (standing) to the try's start, from the tick.
    Back(u32),
    /// Crouch-walking in under the lip, from the tick; the tick it got
    /// there.
    In(u32, Option<u32>),
    /// Let go, still pushing in, from the tick.
    Release(u32),
    /// Walking out along the way out, from the tick and the place.
    Out(u32, Vec3),
    Done(Primed),
}

/// One priming: crouch, let go, wait for the word.
#[derive(Clone, Copy, Debug)]
pub struct PrimeRun {
    phase: Phase,
    tries: u32,
    geo: Option<Geo>,
    walking: Walking,
}

fn turn(d: [f32; 2], deg: f32) -> [f32; 2] {
    let (s, c) = deg.to_radians().sin_cos();
    [d[0] * c - d[1] * s, d[0] * s + d[1] * c]
}

fn toward(from: Vec3, to: [f32; 2], speed: f32) -> ([f32; 2], f32) {
    let d = [to[0] - from[0], to[1] - from[1]];
    let l = d[0].hypot(d[1]);
    if l < 1e-5 {
        ([0.0, 0.0], 0.0)
    } else {
        ([d[0] / l * speed, d[1] / l * speed], l)
    }
}

impl Geo {
    /// The way in for try `n` (from 1): the direction walked in, unit.
    pub fn way_in(&self, n: u32) -> [f32; 2] {
        if n == 1
            && let Some((_, Some(yaw))) = self.start
        {
            let (s, c) = yaw.to_radians().sin_cos();
            return [c, s];
        }
        let i = (n.saturating_sub(1) as usize) % APPROACH_TURNS.len();
        turn([-self.out[0], -self.out[1]], APPROACH_TURNS[i])
    }

    /// Where try `n` (from 1) starts.
    pub fn start(&self, n: u32) -> [f32; 2] {
        if n == 1
            && let Some((off, _)) = self.start
        {
            return [self.stand[0] + off[0], self.stand[1] + off[1]];
        }
        let w = self.way_in(n);
        [
            self.stand[0] - w[0] * START_OUT,
            self.stand[1] - w[1] * START_OUT,
        ]
    }
}

impl PrimeRun {
    /// Starts at `tick`, standing under the ceiling: crouch and let go
    /// where it stands.
    pub fn new(tick: u32) -> PrimeRun {
        PrimeRun {
            phase: Phase::Crouch(tick),
            tries: 1,
            geo: None,
            walking: Walking::Done(Primed::Failed),
        }
    }

    /// Starts at `tick` near a crouch spot whose way out is known: walk to
    /// the try's start, crouch-walk in, let go, walk out.
    pub fn at(tick: u32, geo: Geo) -> PrimeRun {
        PrimeRun {
            phase: Phase::Done(Primed::Failed),
            tries: 1,
            geo: Some(geo),
            walking: Walking::Back(tick),
        }
    }

    /// The try under way (from 1).
    pub fn tries(&self) -> u32 {
        self.tries
    }

    /// The phase, for the agent: `start`, `in`, `release`, `out`, `crouch`,
    /// `stand` or `done`.
    pub fn phase(&self) -> &'static str {
        if self.geo.is_some() {
            match self.walking {
                Walking::Back(_) => "start",
                Walking::In(..) => "in",
                Walking::Release(_) => "release",
                Walking::Out(..) => "out",
                Walking::Done(_) => "done",
            }
        } else {
            match self.phase {
                Phase::Crouch(_) => "crouch",
                Phase::Stand(..) => "stand",
                Phase::Done(_) => "done",
            }
        }
    }

    /// This tick, standing at `at`. `word`: the player's level pair word,
    /// or `None` when it cannot be read.
    pub fn step_at(&mut self, tick: u32, at: Vec3, word: Option<u32>) -> PrimeOut {
        match self.geo {
            Some(g) => self.step_walking(g, tick, at, word),
            None => self.step(tick, word),
        }
    }

    fn step_walking(&mut self, g: Geo, tick: u32, at: Vec3, word: Option<u32>) -> PrimeOut {
        let mut out = PrimeOut::default();
        let n = self.tries;
        let way = g.way_in(n);
        let push = [g.stand[0] + way[0] * PUSH_IN, g.stand[1] + way[1] * PUSH_IN];
        // At most one phase change a tick, then the pad for the phase.
        match self.walking {
            Walking::Back(from) => {
                let (dir, far) = toward(at, g.start(n), 1.0);
                if far <= BACK_NEAR || tick.wrapping_sub(from) > MOVE_TICKS {
                    // Crouch first, standing still.
                    self.walking = Walking::In(tick, None);
                    out.crouch = true;
                } else {
                    let pace = (far * 4.0).clamp(0.25, 1.0);
                    out.walk = Some([dir[0] * pace, dir[1] * pace]);
                }
            }
            Walking::In(from, reached) => {
                out.crouch = true;
                let near = (g.stand[0] - at[0]).hypot(g.stand[1] - at[1]) <= IN_NEAR;
                let reached = reached.or(near.then_some(tick));
                let held = reached.is_some_and(|r| tick.wrapping_sub(r) >= CROUCH_TICKS);
                if held || tick.wrapping_sub(from) > CROUCH_FIRST + MOVE_TICKS + CROUCH_TICKS {
                    self.walking = Walking::Release(tick);
                    out.crouch = false;
                } else {
                    self.walking = Walking::In(from, reached);
                }
                if tick.wrapping_sub(from) >= CROUCH_FIRST {
                    let (dir, _) = toward(at, push, IN_SPEED);
                    out.walk = Some(dir);
                }
            }
            Walking::Release(from) => {
                if tick.wrapping_sub(from) >= RELEASE_TICKS {
                    self.walking = Walking::Out(tick, at);
                    out.walk = Some(g.out);
                } else {
                    let (dir, _) = toward(at, push, IN_SPEED);
                    out.walk = Some(dir);
                }
            }
            Walking::Out(from, let_go) => {
                let t = tick.wrapping_sub(from);
                let gone = (at[0] - let_go[0]).hypot(at[1] - let_go[1]);
                out.walk = Some(g.out);
                match word {
                    Some(w) if is_primed(w) => self.walking = Walking::Done(Primed::Read),
                    Some(_) if t >= READ_TICKS => {
                        if n >= WALK_TRIES {
                            self.walking = Walking::Done(Primed::Failed);
                        } else {
                            self.tries = n + 1;
                            self.walking = Walking::Back(tick);
                        }
                    }
                    None if t >= FORCE_TICKS && gone >= OUT_DIST => {
                        self.walking = Walking::Done(Primed::Assumed)
                    }
                    None if t >= READ_TICKS * 2 => self.walking = Walking::Done(Primed::Assumed),
                    _ => {}
                }
            }
            Walking::Done(_) => {}
        }
        if let Walking::Done(how) = self.walking {
            out.done = Some(how);
            out.walk = None;
            out.crouch = false;
        }
        out
    }

    /// This tick, in place (no way out known). `word`: the player's level
    /// pair word, or `None` when it cannot be read (no pair against the
    /// level yet, or no reader).
    pub fn step(&mut self, tick: u32, word: Option<u32>) -> PrimeOut {
        let mut out = PrimeOut::default();
        match self.phase {
            Phase::Crouch(from) => {
                out.crouch = true;
                if tick.wrapping_sub(from) >= CROUCH_TICKS {
                    self.phase = Phase::Stand(tick, self.tries);
                    out.crouch = false;
                }
            }
            Phase::Stand(from, n) => {
                let t = tick.wrapping_sub(from);
                match word {
                    Some(w) if is_primed(w) && t >= 1 => self.phase = Phase::Done(Primed::Read),
                    Some(_) if t >= READ_TICKS => {
                        if n >= PRIME_TRIES {
                            self.phase = Phase::Done(Primed::Failed);
                        } else {
                            self.tries = n + 1;
                            self.phase = Phase::Crouch(tick);
                            out.crouch = true;
                        }
                    }
                    None if t >= FORCE_TICKS => self.phase = Phase::Done(Primed::Assumed),
                    _ => {}
                }
            }
            Phase::Done(_) => {}
        }
        if let Phase::Done(how) = self.phase {
            out.done = Some(how);
        }
        out
    }
}
