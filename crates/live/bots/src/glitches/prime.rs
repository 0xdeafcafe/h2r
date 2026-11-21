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
//! Pure and deterministic; time in game ticks (30 a second).

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

/// What priming asks of the pad this tick. It never walks: the caller
/// brought the bot under the ceiling and it stays there.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PrimeOut {
    pub crouch: bool,
    /// Over, and how.
    pub done: Option<Primed>,
}

/// One priming: crouch, let go, wait for the word.
#[derive(Clone, Copy, Debug)]
pub struct PrimeRun {
    phase: Phase,
    tries: u32,
}

impl PrimeRun {
    /// Starts at `tick`, standing under the ceiling.
    pub fn new(tick: u32) -> PrimeRun {
        PrimeRun {
            phase: Phase::Crouch(tick),
            tries: 1,
        }
    }

    /// This tick. `word`: the player's level pair word, or `None` when
    /// it cannot be read (no pair against the level yet, or no reader).
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
