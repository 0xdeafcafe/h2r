//! One crouch or drop trial, tick by tick (docs/game/superbounce.md,
//! "Verifying spots"). Pure: each tick takes what the seat reads
//! ([`Read`]) and gives what to do ([`Act`]); the job does the teleports,
//! the pad and the word writes.
//!
//! **Crouch** (the recipe: crouch, walk in, let go, walk out). Teleported
//! outside the lip on the approach, facing in; settled with the word
//! cleared; crouch pressed standing still until it is down; crouch-walked
//! in to just past `stand`; held there; let go while still under (the
//! request should stay set: forced); walked out along the way out. The
//! word turning -3 from the letting go on is a prime.
//!
//! **Drop.** Teleported onto the slot point at rest, settled (the word
//! written as asked), lifted `height` straight up (a teleport keeps the
//! velocity, about zero, and the pair word), then let fall, the word
//! rewritten each tick there is a pair. The contact tick is the first one
//! whose vertical speed is back over -0.5 after falling faster than -1;
//! the landing is classified as the HUD's are
//! ([`superbounce_charge::classify`]) over it and the three ticks after.

use agent_proto::superbounce::{VerifyTick, VerifyTrial};
use superbounce::{Spot, Vec3};

use super::plan::{
    AFTER_TICKS, CROUCH_FIRST_MAX, Config, IN_TICKS, Prime, RELEASE_TICKS, TELEPORT_TICKS, Trial,
    TrialKind, degrees,
};
use crate::superbounce_charge::{self, BOUNCE_VZ, LANDED_VZ, LandingResult, TOUCHDOWN_VZ};

/// The pair word of a primed body, as the game stores it.
pub const PRIMED: u32 = (-3i32) as u32;
/// Crouch: ticks of crouch held standing still before walking in, at
/// least (the bots' `CROUCH_FIRST`), and the crouch amount that counts as
/// down.
pub const CROUCH_FIRST: u32 = bots::glitches::prime::CROUCH_FIRST;
pub const CROUCHED: f32 = 0.9;
/// Crouch: the stick walking in, held under the lip and walking out.
pub const IN_SPEED: f32 = bots::glitches::prime::IN_SPEED;
pub const PUSH_SPEED: f32 = 0.3;
pub const OUT_SPEED: f32 = 1.0;
/// Crouch: the walk in is there within this of its aim point.
pub const IN_NEAR: f32 = bots::glitches::prime::IN_NEAR;
/// Crouch: a body that never got within this of `stand` was not under.
pub const REACH: f32 = 0.2;

/// What the seat reads at a tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Read {
    pub pos: Option<Vec3>,
    pub vel: Option<Vec3>,
    pub yaw: Option<f32>,
    pub airborne: bool,
    /// The crouch request, unit flags bit 23.
    pub request: bool,
    /// The crouch amount, 0 to 1.
    pub crouch: Option<f32>,
    pub standing_fits: Option<bool>,
    /// The level pair word; `None` with no single pair.
    pub word: Option<u32>,
}

/// The pad a tick holds: the walk as a world direction scaled 0..1 (the
/// job turns it into the stick against the facing), crouch and jump.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pad {
    pub walk: [f32; 2],
    pub crouch: bool,
    pub jump: bool,
}

/// What to do at a tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Act {
    pub pad: Pad,
    /// Teleport to the point, facing the yaw (radians).
    pub teleport: Option<(Vec3, f32)>,
    /// Write the pair word (when there is a pair).
    pub word: Option<u32>,
    /// The trial is over.
    pub done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// Waiting for the teleport to `start` (crouch) or the rest point
    /// (drop), from the tick it was asked at; `false` until asked.
    Place(bool, u32),
    Settle(u32),
    /// Crouch.
    CrouchFirst(u32),
    In(u32),
    Hold(u32),
    Release(u32),
    Out(u32),
    /// Drop: waiting for the lift, then falling, then the ticks after
    /// contact.
    Lift(bool, u32),
    Fall(u32),
    After(u32),
    Done,
}

/// The tick before, for a drop's contact.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Prev {
    pos: Vec3,
    vz: f32,
    word: Option<u32>,
}

/// One trial running.
#[derive(Clone, Debug)]
pub struct Run {
    pub index: usize,
    kind: TrialKind,
    trial: Trial,
    spot: Spot,
    prime: Prime,
    settle: u32,
    hold: u32,
    out_ticks: u32,
    fall: u32,
    drift_stick: f32,
    phase: Phase,
    /// Ticks since the trial's first move (after its settle).
    t: u32,
    // Crouch.
    crouched_first: Option<bool>,
    under: Option<bool>,
    forced: Option<bool>,
    closest: Option<f32>,
    released_at: Option<u32>,
    primed_tick: Option<u32>,
    primed_at: Option<Vec3>,
    saw_word: bool,
    early: bool,
    // Drop.
    falling: bool,
    prev: Option<Prev>,
    contact: Option<(u32, Prev)>,
    vz_out: Option<f32>,
    note: Option<String>,
}

fn dist_xy(a: Vec3, b: Vec3) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// The walk toward `to` from `at`, at `speed`.
fn toward(at: Vec3, to: Vec3, speed: f32) -> [f32; 2] {
    let d = [to[0] - at[0], to[1] - at[1]];
    let l = d[0].hypot(d[1]);
    if l < 1e-4 {
        [0.0, 0.0]
    } else {
        [d[0] / l * speed, d[1] / l * speed]
    }
}

impl Run {
    pub fn new(index: usize, trial: Trial, spot: Spot, c: &Config) -> Run {
        Run {
            index,
            kind: c.kind,
            trial,
            spot,
            prime: c.prime,
            settle: c.settle,
            hold: c.hold,
            out_ticks: c.out,
            fall: c.fall,
            drift_stick: c.drift_stick,
            phase: Phase::Place(false, 0),
            t: 0,
            crouched_first: None,
            under: None,
            forced: None,
            closest: None,
            released_at: None,
            primed_tick: None,
            primed_at: None,
            saw_word: false,
            early: false,
            falling: false,
            prev: None,
            contact: None,
            vz_out: None,
            note: None,
        }
    }

    pub fn done(&self) -> bool {
        self.phase == Phase::Done
    }

    /// The phase's name, for the report and the trace.
    pub fn phase(&self) -> &'static str {
        match self.phase {
            Phase::Place(..) => "place",
            Phase::Settle(_) => "settle",
            Phase::CrouchFirst(_) => "crouch",
            Phase::In(_) => "in",
            Phase::Hold(_) => "hold",
            Phase::Release(_) => "release",
            Phase::Out(_) => "out",
            Phase::Lift(..) => "lift",
            Phase::Fall(_) => "fall",
            Phase::After(_) => "after",
            Phase::Done => "done",
        }
    }

    /// Ends the trial with a note (the seat gone, a teleport refused).
    pub fn abort(&mut self, note: impl Into<String>) {
        self.note.get_or_insert_with(|| note.into());
        self.phase = Phase::Done;
    }

    /// The teleport asked for came back: `Err` ends the trial.
    pub fn teleported(&mut self, r: Result<(), String>) {
        match (r, self.phase) {
            (Err(e), _) => self.abort(format!("teleport: {e}")),
            (Ok(()), Phase::Place(true, _)) => self.phase = Phase::Settle(0),
            (Ok(()), Phase::Lift(true, _)) => self.phase = Phase::Fall(0),
            _ => {}
        }
    }

    /// Waiting on a teleport the job has asked for.
    pub fn waiting(&self) -> bool {
        matches!(self.phase, Phase::Place(true, _) | Phase::Lift(true, _))
    }

    /// One tick.
    pub fn step(&mut self, r: &Read) -> Act {
        let mut act = Act::default();
        match self.phase {
            Phase::Done => {
                act.done = true;
                return act;
            }
            Phase::Place(asked, n) | Phase::Lift(asked, n) => {
                let lift = matches!(self.phase, Phase::Lift(..));
                if !asked {
                    let to = match (self.kind, lift) {
                        (TrialKind::Drop, false) => self.trial.point,
                        _ => self.trial.start,
                    };
                    act.teleport = Some((to, self.trial.yaw));
                    self.phase = if lift {
                        Phase::Lift(true, 0)
                    } else {
                        Phase::Place(true, 0)
                    };
                } else if n >= TELEPORT_TICKS {
                    self.abort("teleport: never ran");
                    act.done = true;
                } else {
                    self.phase = if lift {
                        Phase::Lift(true, n + 1)
                    } else {
                        Phase::Place(true, n + 1)
                    };
                }
                return act;
            }
            _ => {}
        }
        let Some(pos) = r.pos else {
            self.abort("the seat has no unit");
            act.done = true;
            return act;
        };
        match self.kind {
            TrialKind::Crouch => self.crouch_step(r, pos, &mut act),
            TrialKind::Drop => self.drop_step(r, pos, &mut act),
            TrialKind::Route => self.abort("a route is not a body trial"),
        }
        act.done = self.done();
        act
    }

    fn crouch_step(&mut self, r: &Read, pos: Vec3, act: &mut Act) {
        let stand = self.spot.stand;
        let push = self.trial.point;
        let d = dist_xy(pos, stand);
        if !matches!(self.phase, Phase::Settle(_)) {
            self.t += 1;
            self.closest = Some(self.closest.map_or(d, |c| c.min(d)));
            if r.standing_fits == Some(false) {
                self.under = Some(true);
            } else if r.standing_fits == Some(true) && self.under.is_none() {
                self.under = Some(false);
            }
        }
        let primed = r.word == Some(PRIMED);
        if r.word.is_some() && !matches!(self.phase, Phase::Settle(_)) {
            self.saw_word = true;
        }
        match self.phase {
            Phase::Settle(n) => {
                // Clear a word left from before (a teleport keeps it).
                if r.word.is_some_and(|w| w != 0) {
                    act.word = Some(0);
                }
                self.phase = if n + 1 >= self.settle {
                    Phase::CrouchFirst(0)
                } else {
                    Phase::Settle(n + 1)
                };
            }
            Phase::CrouchFirst(n) => {
                act.pad.crouch = true;
                let down = r.crouch.map_or(r.request, |c| c >= CROUCHED);
                if (n + 1 >= CROUCH_FIRST && down) || n + 1 >= CROUCH_FIRST_MAX {
                    self.crouched_first = Some(down);
                    self.phase = Phase::In(0);
                } else {
                    self.phase = Phase::CrouchFirst(n + 1);
                }
            }
            Phase::In(n) => {
                act.pad.crouch = true;
                act.pad.walk = toward(pos, push, IN_SPEED);
                if primed {
                    self.early = true;
                }
                if dist_xy(pos, push) <= IN_NEAR || n + 1 >= IN_TICKS {
                    self.phase = Phase::Hold(0);
                } else {
                    self.phase = Phase::In(n + 1);
                }
            }
            Phase::Hold(n) => {
                act.pad.crouch = true;
                act.pad.walk = toward(pos, push, PUSH_SPEED);
                if primed {
                    self.early = true;
                }
                self.phase = if n + 1 >= self.hold {
                    self.released_at = Some(self.t);
                    Phase::Release(0)
                } else {
                    Phase::Hold(n + 1)
                };
            }
            Phase::Release(n) => {
                act.pad.walk = toward(pos, push, PUSH_SPEED);
                if self.watch_prime(primed, pos) {
                    return;
                }
                if n + 1 >= RELEASE_TICKS {
                    // Button up for these ticks: a request still set is
                    // the lip forcing it.
                    self.forced = Some(r.request);
                    self.phase = Phase::Out(0);
                } else {
                    self.phase = Phase::Release(n + 1);
                }
            }
            Phase::Out(n) => {
                let o = self.trial.way_out;
                act.pad.walk = [o[0] * OUT_SPEED, o[1] * OUT_SPEED];
                if self.watch_prime(primed, pos) {
                    return;
                }
                self.phase = if n + 1 >= self.out_ticks {
                    Phase::Done
                } else {
                    Phase::Out(n + 1)
                };
            }
            _ => {}
        }
    }

    /// A -3 from the letting go on: primed, and the trial is over.
    fn watch_prime(&mut self, primed: bool, pos: Vec3) -> bool {
        if !primed {
            return false;
        }
        let from = self.released_at.unwrap_or(self.t);
        self.primed_tick = Some(self.t.saturating_sub(from));
        self.primed_at = Some(pos);
        if self.forced.is_none() {
            self.forced = Some(true);
        }
        self.phase = Phase::Done;
        true
    }

    fn word_for_fall(&self, word: Option<u32>) -> Option<u32> {
        let want = match self.prime {
            Prime::Set => PRIMED,
            Prime::None => 0,
            Prime::Keep => return None,
        };
        word.filter(|&w| w != want).map(|_| want)
    }

    fn drop_step(&mut self, r: &Read, pos: Vec3, act: &mut Act) {
        let vz = r.vel.map_or(0.0, |v| v[2]);
        match self.phase {
            Phase::Settle(n) => {
                act.word = self.word_for_fall(r.word);
                self.phase = if n + 1 >= self.settle {
                    Phase::Lift(false, 0)
                } else {
                    Phase::Settle(n + 1)
                };
            }
            Phase::Fall(n) => {
                self.t += 1;
                let d = self.trial.drift;
                act.pad.walk = [d[0] * self.drift_stick, d[1] * self.drift_stick];
                act.word = self.word_for_fall(r.word);
                if vz < TOUCHDOWN_VZ {
                    self.falling = true;
                }
                if self.falling && vz > LANDED_VZ {
                    let prev = self.prev.unwrap_or(Prev {
                        pos,
                        vz,
                        word: r.word,
                    });
                    self.contact = Some((n, prev));
                    self.vz_out = Some(vz);
                    self.phase = Phase::After(0);
                } else if n + 1 >= self.fall {
                    self.phase = Phase::Done;
                } else {
                    self.phase = Phase::Fall(n + 1);
                }
                // What the game reads next tick: the word as written.
                self.prev = Some(Prev {
                    pos,
                    vz,
                    word: act.word.or(r.word),
                });
            }
            Phase::After(n) => {
                self.t += 1;
                self.vz_out = Some(self.vz_out.map_or(vz, |v| v.max(vz)));
                self.phase = if n + 1 >= AFTER_TICKS {
                    Phase::Done
                } else {
                    Phase::After(n + 1)
                };
            }
            _ => {}
        }
    }

    /// The tick as a trace row, after [`Run::step`] gave `act`.
    pub fn trace(&self, r: &Read, act: &Act) -> VerifyTick {
        VerifyTick {
            t: self.t,
            phase: self.phase().into(),
            pos: r.pos,
            vz: r.vel.map(|v| v[2]),
            airborne: r.airborne,
            request: r.request,
            crouch_button: act.pad.crouch,
            crouch: r.crouch,
            standing_fits: r.standing_fits,
            word: r.word.map(|w| w as i32),
        }
    }

    /// What came of it.
    pub fn outcome(&self) -> VerifyTrial {
        let t = &self.trial;
        let mut v = VerifyTrial {
            index: self.index,
            spot: t.spot,
            start: t.start,
            yaw: Some(degrees(t.yaw)),
            distance: t.distance,
            offset: t.offset,
            height: t.height,
            note: self.note.clone(),
            ..VerifyTrial::default()
        };
        match self.kind {
            TrialKind::Crouch => {
                v.ok = self.primed_tick.is_some();
                v.crouched_first = self.crouched_first;
                v.under = self.under;
                v.forced = self.forced;
                v.primed_tick = self.primed_tick;
                v.primed_at = self.primed_at;
                v.closest = self.closest;
                if self.early && v.note.is_none() {
                    v.note = Some("the word read -3 before letting go".into());
                } else if !self.saw_word && v.note.is_none() && !v.ok {
                    v.note = Some("no level pair to read".into());
                }
            }
            TrialKind::Drop => match self.contact {
                Some((tick, prev)) => {
                    let vz_out = self.vz_out.unwrap_or(prev.vz);
                    let speed_in = -prev.vz;
                    let result = superbounce_charge::classify(
                        &self.spot, prev.word, prev.pos, speed_in, vz_out,
                    );
                    v.ok = result == LandingResult::Bounce && vz_out > BOUNCE_VZ;
                    v.contact_tick = Some(tick);
                    v.speed_in = Some(speed_in);
                    v.vz_out = Some(vz_out);
                    v.landed_at = Some(prev.pos);
                    v.word_before = prev.word.map(|w| w as i32);
                    v.landing = Some(landing_name(result).into());
                }
                None if self.note.is_none() => v.landing = Some("no_touchdown".into()),
                None => {}
            },
            TrialKind::Route => {}
        }
        v
    }
}

/// The wire name of a landing's result.
pub fn landing_name(r: LandingResult) -> &'static str {
    match r {
        LandingResult::Bounce => "bounce",
        LandingResult::NotPrimed => "not_primed",
        LandingResult::Missed => "missed",
        LandingResult::TooSlow => "too_slow",
        LandingResult::NoBounce => "no_bounce",
    }
}
