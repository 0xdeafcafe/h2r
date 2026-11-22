//! The bounce search (docs/game/bounce-search.md): a practising bot flies a
//! bounce site with varied knobs, each try is scored, and the best knobs
//! are kept in the map's learned file (`superbounce::learned`).
//!
//! Pure: [`Search`] takes try outcomes in ([`Try`]) and hands knobs out
//! ([`Search::next`]), a round at a time ([`Search::fold`]). Its whole
//! state is the site's line, so a batch resumes from the file. Draws are a
//! pure function of the site, the seed, the try and the axis ([`draw`]),
//! so the same results give the same search. [`Board`] schedules a
//! training run's bots over the sites of its shard; [`set`], [`done`] and
//! [`with_board`] keep the process's one board, as `drill::set` keeps the
//! drill.

use std::sync::Mutex;

pub use superbounce::learned::spot_id;
use superbounce::learned::{self, AIM_ALONG, AIM_SIDE, Entry, FROM_SIDE, How, Knobs, Stage, State};

use crate::bounce_route::{AIR_DEAD, BOUNCE_VZ, Leap, RUN_UP, TAKEOFF_EARLY};
use crate::brain::Vec3;
use crate::glitches::prime::is_primed;
use crate::tuning::BOUNCE_DEFAULT;

/// Tries an arm in the `arms` round.
pub const ARM_TRIES: usize = 3;
/// Tries at the centre, and tries with the aim moved, in a `shift` round.
pub const SHIFT_TRIES: usize = 3;
/// The most `shift` rounds.
pub const SHIFT_ROUNDS: u32 = 3;
/// A `shift` moves the aim this share of the mean miss.
pub const SHIFT_GAIN: f32 = 0.7;
/// Tries at the centre, a step under and a step over, in a `compass`
/// round.
pub const PROBE_TRIES: usize = 2;
/// A compass side moves the centre when its mean beats the centre's score
/// by this.
pub const MARGIN: f32 = 0.05;
/// The compass stage ends when the steps would halve this often.
pub const SHRINK_END: u32 = 3;
/// Tries in the `scatter` round.
pub const SCATTER_TRIES: usize = 16;
/// `learned`: the centre has this many tries...
pub const LEARNED_N: u32 = 10;
/// ...and bounces at least this share of them.
pub const LEARNED_RATE: f64 = 0.6;
/// `blocked`: this many rounds running were all void, at a site never
/// flown (one flown before starts the count again).
pub const VOID_ROUNDS: u32 = 3;
/// A void try's slot is asked this many times in all (again twice at
/// most), then given up for the round.
pub const SLOT_ASKS: u32 = 3;
/// The run sticks the `arms` round tries a drop at: full, and the tuning's
/// `run_off_speed` ([`Setup::run_off`]).
pub const ARM_RUN: f32 = 1.0;

/// One axis of the search: an offset from the route's own point (the aim
/// along and to the side, the launch point to the side) or a knob.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    AimAlong,
    AimSide,
    FromSide,
    Yaw,
    Back,
    Run,
    Early,
    Gain,
    Letgo,
    Drift,
    Still,
}

/// The [`draw`] axis of the scatter's `how`.
pub const HOW_AXIS: u32 = 11;

impl Axis {
    pub const ALL: [Axis; 11] = [
        Axis::AimAlong,
        Axis::AimSide,
        Axis::FromSide,
        Axis::Yaw,
        Axis::Back,
        Axis::Run,
        Axis::Early,
        Axis::Gain,
        Axis::Letgo,
        Axis::Drift,
        Axis::Still,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Axis::AimAlong => "aim_along",
            Axis::AimSide => "aim_side",
            Axis::FromSide => "from_side",
            Axis::Yaw => "yaw",
            Axis::Back => "back",
            Axis::Run => "run",
            Axis::Early => "early",
            Axis::Gain => "gain",
            Axis::Letgo => "letgo",
            Axis::Drift => "drift",
            Axis::Still => "still",
        }
    }

    /// The [`draw`] axis number.
    pub fn index(self) -> u32 {
        Axis::ALL.iter().position(|a| *a == self).unwrap_or(0) as u32
    }

    /// The compass's first step: 0.1 units, 0.2 of stick, 0.6 of `back`,
    /// 5 degrees, 0.4 of `gain`, 0.02 of `letgo`, 4 ticks.
    pub fn first_step(self) -> f32 {
        match self {
            Axis::AimAlong | Axis::AimSide | Axis::FromSide | Axis::Early => 0.1,
            Axis::Run | Axis::Drift => 0.2,
            Axis::Back => 0.6,
            Axis::Yaw => 5.0,
            Axis::Gain => 0.4,
            Axis::Letgo => 0.02,
            Axis::Still => 4.0,
        }
    }

    /// The range searched: an offset for the points, else the knob's own.
    pub fn range(self) -> (f32, f32) {
        match self {
            Axis::AimAlong => (-AIM_ALONG, AIM_ALONG),
            Axis::AimSide => (-AIM_SIDE, AIM_SIDE),
            Axis::FromSide => (-FROM_SIDE, FROM_SIDE),
            Axis::Yaw => learned::YAW,
            Axis::Back => learned::BACK,
            Axis::Run => learned::RUN,
            Axis::Early => learned::EARLY,
            Axis::Gain => learned::GAIN,
            Axis::Letgo => learned::LETGO,
            Axis::Drift => learned::DRIFT,
            Axis::Still => (learned::STILL.0 as f32, learned::STILL.1 as f32),
        }
    }
}

// ---- tries and their score ----

/// A touchdown by the strip, as a try's score reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Down {
    /// How far from the site's own target it came down (world units, XY).
    pub miss: f32,
    /// The site's own target minus the touchdown (world XY): the way the
    /// landing fell short.
    pub off: [f32; 2],
    /// The speed in, down, and the vertical speed out (world units a
    /// second).
    pub speed_in: f32,
    pub vz_out: f32,
}

impl Down {
    pub fn bounced(&self) -> bool {
        self.vz_out > BOUNCE_VZ
    }
}

/// What one training try came to: what the route's last
/// [`Leap`] and the brain say of it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Try {
    /// Why the try says nothing about the knobs (`not_primed`,
    /// `pair_lost`, `no_path`, `map_changed`, `practice_off`, ...): not
    /// scored, costs no budget. `None` for a scored try.
    pub void: Option<&'static str>,
    /// The touchdown by the strip; `None` for none (`no_drop`,
    /// `no_landing`, `timeout`).
    pub down: Option<Down>,
    /// The bot died before it stood again.
    pub died: bool,
}

impl Try {
    /// A try that says nothing about the knobs.
    pub fn void(why: &'static str) -> Try {
        Try {
            void: Some(why),
            ..Try::default()
        }
    }

    pub fn scored(&self) -> bool {
        self.void.is_none()
    }

    pub fn bounced(&self) -> bool {
        self.scored() && self.down.is_some_and(|d| d.bounced())
    }

    /// The try a route's last leap makes. `target` is the site's own target
    /// (`Site::target` without the line), not the try's aim, so moving the
    /// aim moves the miss. A leap whose word read as not -3 at touchdown is
    /// void; no leap is a try with no touchdown.
    pub fn from_leap(leap: Option<&Leap>, target: Vec3, died: bool) -> Try {
        let Some(l) = leap else {
            return Try {
                died,
                ..Try::default()
            };
        };
        match l.end {
            Some("not_primed") => return Try::void("not_primed"),
            Some("pair_lost") => return Try::void("pair_lost"),
            _ => {}
        }
        if let Some(t) = l.landing
            && !t.bounced()
            && t.word.is_some_and(|w| !is_primed(w))
        {
            return Try::void("not_primed");
        }
        let down = l.landing.map(|t| {
            let off = [target[0] - t.at[0], target[1] - t.at[1]];
            Down {
                miss: off[0].hypot(off[1]),
                off,
                speed_in: t.speed_in,
                vz_out: t.vz_out,
            }
        });
        Try {
            void: None,
            down,
            died,
        }
    }
}

/// A try's one number (docs/game/bounce-search.md, "A try and its
/// score"): a bounce 1 to 1.5 by its launch; a landing that did not bounce
/// up to 0.8 as the miss shrinks and the landing speeds up toward `need`
/// (unknown, 0: the speed term is full); no touchdown 0; a death takes 1
/// off. A void try scores 0 and is never counted.
pub fn score(t: &Try, need: f32) -> f32 {
    if !t.scored() {
        return 0.0;
    }
    let s = match t.down {
        None => 0.0,
        Some(d) if d.bounced() => 1.0 + 0.5 * (d.vz_out / 20.0).min(1.0),
        Some(d) => {
            let speed = if need > 0.0 {
                0.2 * (d.speed_in / need).clamp(0.0, 1.0)
            } else {
                0.2
            };
            0.6 / (1.0 + d.miss.max(0.0) / 0.1) + speed
        }
    };
    if t.died { s - 1.0 } else { s }
}

// ---- draws ----

fn splitmix64(z: u64) -> u64 {
    let z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    let z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The scatter's one random number, in `[0, 1)`: splitmix64 over the spot
/// id ([`spot_id`]), the batch seed, the try's index (the site's `tries`
/// at the round's start plus the slot) and the axis ([`Axis::index`],
/// [`HOW_AXIS`]). The caller maps it to the axis's range.
pub fn draw(spot: u64, seed: u64, try_index: u32, axis: u32) -> f32 {
    let h = splitmix64(
        splitmix64(splitmix64(splitmix64(spot) ^ seed) ^ u64::from(try_index)) ^ u64::from(axis),
    );
    (h >> 40) as f32 / (1u64 << 24) as f32
}

// ---- the search ----

/// What a site's search needs besides its line.
#[derive(Clone, Debug, PartialEq)]
pub struct Setup {
    /// The site's `stand`, as the scan gave it.
    pub at: Vec3,
    /// [`spot_id`] of the site.
    pub spot: u64,
    /// The batch's seed.
    pub seed: u64,
    /// Scored tries the site may take.
    pub budget: u32,
    /// The `how`s the site allows, the route's own choice first.
    pub hows: Vec<How>,
    /// The route's own knobs, every one set (`bounce_route::flown` turned
    /// back into knobs): what an unset knob plays, and the centre the
    /// offsets are taken from. One left unset reads as the route's
    /// built-in constant.
    pub base: Knobs,
    /// The tuning's `run_off_speed`: the `arms` round's second stick.
    pub run_off: f32,
    /// The site's `need` (0 unknown).
    pub need: f32,
    /// A V's arm flown from its own side, not the mirror: the control
    /// (docs/game/bounce-search.md, "Vs: the mirror and its control").
    /// Its id is [`learned::control_id`] and its line a control line.
    pub control: bool,
}

/// The route's built-in values, for a base knob the caller left unset.
fn fallback() -> Knobs {
    Knobs {
        how: Some(How::Runoff),
        from: None,
        aim: None,
        yaw: Some(0.0),
        back: Some(RUN_UP),
        run: Some(ARM_RUN),
        early: Some(TAKEOFF_EARLY),
        gain: Some(BOUNCE_DEFAULT.air_gain),
        letgo: Some(AIR_DEAD),
        drift: Some(BOUNCE_DEFAULT.drift),
        still: Some(BOUNCE_DEFAULT.still_ticks),
    }
}

/// The geometry the offsets are taken in.
#[derive(Clone, Copy, Debug)]
struct Frame {
    aim: [f32; 2],
    from: Option<Vec3>,
    along: [f32; 2],
    side: [f32; 2],
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

/// Which slot of a compass round a try is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Centre,
    Under,
    Over,
}

/// A round's tries matched to its slots.
struct Walked {
    /// Each finished slot's scored try; `None` when it was given up void.
    results: Vec<Option<Try>>,
    /// The next slot to fly and its knobs.
    pending: Option<(usize, Knobs)>,
    /// The void reasons seen.
    voids: Vec<&'static str>,
}

/// What a round came to, for the log.
#[derive(Clone, Debug, PartialEq)]
pub struct Folded {
    /// The round's stage and, in `compass`, its axis.
    pub stage: Stage,
    pub axis: Option<Axis>,
    /// Every slot was void.
    pub void: bool,
    /// The centre's score before and after.
    pub before: f32,
    pub after: f32,
    /// How the centre moved, when it did.
    pub change: Option<String>,
    /// The site's state, tries and hits after it.
    pub state: State,
    pub tries: u32,
    pub hits: u32,
}

impl Folded {
    /// `compass aim_along: 0.71 -> 1.18, aim +0.10 along; tries 54 hits 3`.
    pub fn describe(&self) -> String {
        let mut o = self.stage.name().to_string();
        if let Some(a) = self.axis {
            o.push(' ');
            o.push_str(a.name());
        }
        if self.void {
            o.push_str(": void");
        } else {
            o.push_str(&format!(": {:.2} -> {:.2}", self.before, self.after));
            if let Some(c) = &self.change {
                o.push_str(", ");
                o.push_str(c);
            }
        }
        o.push_str(&format!("; tries {} hits {}", self.tries, self.hits));
        if self.state != State::Open {
            o.push_str("; ");
            o.push_str(self.state.name());
        }
        o
    }
}

fn mean_of(ts: &[Try], need: f32) -> (u32, u32, f32) {
    let n = ts.len() as u32;
    let ok = ts.iter().filter(|t| t.bounced()).count() as u32;
    let sum: f32 = ts.iter().map(|t| score(t, need)).sum();
    (n, ok, if n == 0 { 0.0 } else { sum / n as f32 })
}

/// One site's search: a coordinate search with shrinking steps, in rounds,
/// with one scatter for a site that never bounces. Its state is the site's
/// line ([`Search::entry`]) and, kept only in memory, the void rounds
/// running.
#[derive(Clone, Debug, PartialEq)]
pub struct Search {
    setup: Setup,
    line: Entry,
    voids: u32,
    void_why: Vec<&'static str>,
}

impl Search {
    /// The search of a site from its line (`None`: a site never flown). A
    /// `partial` line reopens when the budget is over its `tries`.
    pub fn new(mut setup: Setup, line: Option<&Entry>) -> Search {
        setup.base = setup.base.or(fallback()).clamped().rounded();
        let mut hows: Vec<How> = Vec::new();
        for h in setup
            .hows
            .iter()
            .copied()
            .chain(setup.base.how.filter(|_| setup.hows.is_empty()))
        {
            if !hows.contains(&h) {
                hows.push(h);
            }
        }
        setup.hows = hows;
        let line = match line {
            Some(e) => {
                let mut e = e.clone();
                if e.state == State::Partial && e.tries < setup.budget {
                    e.state = State::Open;
                }
                // Only a site never flown is blocked: one with tries is
                // flown again.
                if e.state == State::Blocked && e.tries > 0 {
                    e.state = State::Open;
                    e.why = None;
                }
                e
            }
            None => Entry {
                at: setup.at,
                control: setup.control,
                ..Entry::default()
            },
        };
        Search {
            setup,
            line,
            voids: 0,
            void_why: Vec::new(),
        }
    }

    pub fn setup(&self) -> &Setup {
        &self.setup
    }

    /// The site's line as the file holds it.
    pub fn entry(&self) -> &Entry {
        &self.line
    }

    pub fn state(&self) -> State {
        self.line.state
    }

    /// The best knobs so far (unset knobs: the route's own).
    pub fn centre(&self) -> Knobs {
        self.line.knobs
    }

    /// The round's axis, in `compass`.
    pub fn axis(&self) -> Option<Axis> {
        (self.line.stage == Stage::Compass).then(|| {
            let axes = self.axes();
            axes[(self.line.axis as usize).min(axes.len() - 1)]
        })
    }

    /// The round's next slot to fly from its results so far, `None` when
    /// the round is over (or the site is not open).
    pub fn next(&self, done: &[Try]) -> Option<Knobs> {
        self.next_slot(done).map(|(_, k)| k)
    }

    /// [`Search::next`] with the slot's index in the round.
    pub fn next_slot(&self, done: &[Try]) -> Option<(usize, Knobs)> {
        if self.line.state != State::Open {
            return None;
        }
        self.walk(done).pending
    }

    fn frame(&self) -> Frame {
        let b = &self.setup.base;
        let aim = b.aim.unwrap_or([self.setup.at[0], self.setup.at[1]]);
        let along = b
            .from
            .and_then(|f| {
                let d = [aim[0] - f[0], aim[1] - f[1]];
                let l = d[0].hypot(d[1]);
                (l > 0.01).then(|| [d[0] / l, d[1] / l])
            })
            .unwrap_or([1.0, 0.0]);
        Frame {
            aim,
            from: b.from,
            along,
            side: [-along[1], along[0]],
        }
    }

    /// The centre with every unset knob the route's own.
    fn full_centre(&self) -> Knobs {
        let mut c = self.line.knobs.or(self.setup.base);
        if self.line.knobs.how.is_none() {
            c.how = self.setup.hows.first().copied().or(c.how);
        }
        c
    }

    fn get(&self, f: &Frame, k: &Knobs, a: Axis) -> f32 {
        let b = &self.setup.base;
        match a {
            Axis::AimAlong | Axis::AimSide => {
                let aim = k.aim.unwrap_or(f.aim);
                let d = [aim[0] - f.aim[0], aim[1] - f.aim[1]];
                dot(d, if a == Axis::AimAlong { f.along } else { f.side })
            }
            Axis::FromSide => match (k.from, f.from) {
                (Some(p), Some(o)) => dot([p[0] - o[0], p[1] - o[1]], f.side),
                _ => 0.0,
            },
            Axis::Yaw => k.yaw.or(b.yaw).unwrap_or(0.0),
            Axis::Back => k.back.or(b.back).unwrap_or(RUN_UP),
            Axis::Run => k.run.or(b.run).unwrap_or(ARM_RUN),
            Axis::Early => k.early.or(b.early).unwrap_or(TAKEOFF_EARLY),
            Axis::Gain => k.gain.or(b.gain).unwrap_or(BOUNCE_DEFAULT.air_gain),
            Axis::Letgo => k.letgo.or(b.letgo).unwrap_or(AIR_DEAD),
            Axis::Drift => k.drift.or(b.drift).unwrap_or(BOUNCE_DEFAULT.drift),
            Axis::Still => k.still.or(b.still).unwrap_or(BOUNCE_DEFAULT.still_ticks) as f32,
        }
    }

    /// `k` with axis `a` at `v`, held to the axis's range.
    fn set(&self, f: &Frame, k: Knobs, a: Axis, v: f32) -> Knobs {
        let (lo, hi) = a.range();
        let v = v.clamp(lo, hi);
        let mut k = k;
        match a {
            Axis::AimAlong | Axis::AimSide => {
                let (al, sd) = if a == Axis::AimAlong {
                    (v, self.get(f, &k, Axis::AimSide))
                } else {
                    (self.get(f, &k, Axis::AimAlong), v)
                };
                k.aim = Some([
                    f.aim[0] + f.along[0] * al + f.side[0] * sd,
                    f.aim[1] + f.along[1] * al + f.side[1] * sd,
                ]);
            }
            Axis::FromSide => {
                if let Some(o) = f.from {
                    k.from = Some([o[0] + f.side[0] * v, o[1] + f.side[1] * v, o[2]]);
                }
            }
            Axis::Yaw => k.yaw = Some(v),
            Axis::Back => k.back = Some(v),
            Axis::Run => k.run = Some(v),
            Axis::Early => k.early = Some(v),
            Axis::Gain => k.gain = Some(v),
            Axis::Letgo => k.letgo = Some(v),
            Axis::Drift => k.drift = Some(v),
            Axis::Still => k.still = Some(v.round().max(0.0) as u32),
        }
        k
    }

    /// The axes of a compass pass for the centre's `how`: at shrink 0 the
    /// four that move the landing most (a drop: aim along, `run`, `back`,
    /// aim side); from shrink 1 those and the launch point's side, `yaw`,
    /// `gain`, `letgo` and, a running jump, `early`. A stand: aim along,
    /// aim side, `drift`, `still`.
    fn axes(&self) -> Vec<Axis> {
        let how = self.full_centre().how.unwrap_or_default();
        if how == How::Stand {
            return vec![Axis::AimAlong, Axis::AimSide, Axis::Drift, Axis::Still];
        }
        let mut v = vec![Axis::AimAlong, Axis::Run, Axis::Back, Axis::AimSide];
        if self.line.shrink > 0 {
            if self.setup.base.from.is_some() {
                v.push(Axis::FromSide);
            }
            v.extend([Axis::Yaw, Axis::Gain, Axis::Letgo]);
            if how == How::Jump {
                v.push(Axis::Early);
            }
        }
        v
    }

    /// The `arms` round's arms: each `how` the site allows, a drop at a
    /// full stick and at `run_off`.
    fn arms(&self) -> Vec<Knobs> {
        let mut v: Vec<Knobs> = Vec::new();
        let run_off =
            learned::quantise(self.setup.run_off.clamp(learned::RUN.0, learned::RUN.1), 2);
        for &h in &self.setup.hows {
            let base = Knobs {
                how: Some(h),
                ..self.setup.base
            };
            if h == How::Stand {
                v.push(base.canonical());
                continue;
            }
            for run in [ARM_RUN, run_off] {
                let k = Knobs {
                    run: Some(run),
                    ..base
                }
                .canonical();
                if !v.contains(&k) {
                    v.push(k);
                }
            }
        }
        v
    }

    /// The route's own choice first among equal arms: its `how`, then the
    /// stick nearest its own.
    fn arm_pref(&self, k: &Knobs) -> (bool, f32) {
        let own = self.setup.hows.first().copied();
        let run = self.setup.base.run.unwrap_or(ARM_RUN);
        (k.how != own, k.run.map_or(0.0, |r| (r - run).abs()))
    }

    /// The shift's moved knobs from the centre's tries this round: the aim
    /// moved [`SHIFT_GAIN`] of the mean miss, held to its range. `None`
    /// when a try bounced, none came down, or the aim would not move.
    fn shifted(&self, centre: &[Option<Try>]) -> Option<Knobs> {
        let ts: Vec<&Try> = centre.iter().flatten().collect();
        if ts.iter().any(|t| t.bounced()) {
            return None;
        }
        let downs: Vec<Down> = ts.iter().filter_map(|t| t.down).collect();
        if downs.is_empty() {
            return None;
        }
        let n = downs.len() as f32;
        let off = [
            downs.iter().map(|d| d.off[0]).sum::<f32>() / n,
            downs.iter().map(|d| d.off[1]).sum::<f32>() / n,
        ];
        let f = self.frame();
        let c = self.full_centre();
        let along = self.get(&f, &c, Axis::AimAlong) + SHIFT_GAIN * dot(off, f.along);
        let side = self.get(&f, &c, Axis::AimSide) + SHIFT_GAIN * dot(off, f.side);
        let k = self.set(
            &f,
            self.set(&f, c, Axis::AimAlong, along),
            Axis::AimSide,
            side,
        );
        (k.canonical() != c.canonical()).then_some(k.canonical())
    }

    /// The compass round's slots: the centre, a step under and a step over,
    /// [`PROBE_TRIES`] each, interleaved; a side the range clips onto the
    /// centre is left out.
    fn compass_slots(&self) -> Vec<(Side, Knobs)> {
        let f = self.frame();
        let c = self.full_centre();
        let Some(a) = self.axis() else {
            return Vec::new();
        };
        let step = a.first_step() * 0.5f32.powi(self.line.shrink.min(SHRINK_END) as i32);
        let step = if a == Axis::Still {
            step.round().max(1.0)
        } else {
            step
        };
        let v = self.get(&f, &c, a);
        let centre = c.canonical();
        let under = self.set(&f, c, a, v - step).canonical();
        let over = self.set(&f, c, a, v + step).canonical();
        let mut out = Vec::new();
        for _ in 0..PROBE_TRIES {
            out.push((Side::Centre, centre));
            if under != centre {
                out.push((Side::Under, under));
            }
            if over != centre {
                out.push((Side::Over, over));
            }
        }
        out
    }

    /// The scatter's draws: each knob even over its range, `how` over what
    /// the site allows.
    fn scatter_slots(&self) -> Vec<Knobs> {
        let f = self.frame();
        let base = self.setup.base;
        (0..SCATTER_TRIES)
            .map(|slot| {
                let i = self.line.tries + slot as u32;
                let d = |axis: u32| draw(self.setup.spot, self.setup.seed, i, axis);
                let hows = &self.setup.hows;
                let how = hows
                    .get(
                        ((d(HOW_AXIS) * hows.len() as f32) as usize)
                            .min(hows.len().saturating_sub(1)),
                    )
                    .copied()
                    .or(base.how);
                let mut k = Knobs { how, ..base };
                for a in Axis::ALL {
                    let (lo, hi) = a.range();
                    k = self.set(&f, k, a, lo + (hi - lo) * d(a.index()));
                }
                k.canonical()
            })
            .collect()
    }

    /// The round's slots, from the results of the slots before.
    fn slots(&self, results: &[Option<Try>]) -> Vec<Knobs> {
        match self.line.stage {
            Stage::Arms => self
                .arms()
                .into_iter()
                .flat_map(|k| std::iter::repeat_n(k, ARM_TRIES))
                .collect(),
            Stage::Shift => {
                let c = self.full_centre().canonical();
                let mut v = vec![c; SHIFT_TRIES];
                if results.len() >= SHIFT_TRIES
                    && let Some(m) = self.shifted(&results[..SHIFT_TRIES])
                {
                    v.extend([m; SHIFT_TRIES]);
                }
                v
            }
            Stage::Compass => self.compass_slots().into_iter().map(|(_, k)| k).collect(),
            Stage::Scatter => self.scatter_slots(),
        }
    }

    /// Matches a round's tries to its slots in order: a scored try finishes
    /// its slot; a void one asks the slot again, [`SLOT_ASKS`] in all.
    fn walk(&self, done: &[Try]) -> Walked {
        let mut results: Vec<Option<Try>> = Vec::new();
        let mut voids = Vec::new();
        let mut asks = 0;
        let mut it = done.iter();
        loop {
            let slots = self.slots(&results);
            let i = results.len();
            let Some(&k) = slots.get(i) else {
                return Walked {
                    results,
                    pending: None,
                    voids,
                };
            };
            match it.next() {
                None => {
                    return Walked {
                        results,
                        pending: Some((i, k)),
                        voids,
                    };
                }
                Some(t) => match t.void {
                    None => {
                        results.push(Some(*t));
                        asks = 0;
                    }
                    Some(why) => {
                        voids.push(why);
                        asks += 1;
                        if asks >= SLOT_ASKS {
                            results.push(None);
                            asks = 0;
                        }
                    }
                },
            }
        }
    }

    /// Adds tries at the centre to its count and mean.
    fn add_centre(&mut self, ts: &[Try]) {
        if ts.is_empty() {
            return;
        }
        let (n, ok, mean) = mean_of(ts, self.setup.need);
        let l = &mut self.line;
        let total = l.score * l.n as f32 + mean * n as f32;
        l.n += n;
        l.ok += ok;
        l.score = total / l.n as f32;
    }

    fn take_centre(&mut self, k: Knobs, ts: &[Try]) {
        let (n, ok, mean) = mean_of(ts, self.setup.need);
        let l = &mut self.line;
        l.knobs = k.canonical();
        l.n = n;
        l.ok = ok;
        l.score = mean;
    }

    fn enter_compass(&mut self, shrink: u32) {
        let l = &mut self.line;
        l.stage = Stage::Compass;
        l.axis = 0;
        l.shrink = shrink;
        l.moved = false;
    }

    /// Ends the round: moves the centre on what it measured, steps the
    /// cursor, and reads the stop rules. A round is folded once every slot
    /// is flown ([`Search::next`] gave `None`); slots not flown count as
    /// given up. A round of only void slots changes nothing but the void
    /// count: three running block the site.
    pub fn fold(&mut self, done: &[Try]) -> Folded {
        let w = self.walk(done);
        let stage = self.line.stage;
        let axis = self.axis();
        let before = self.line.score;
        let scored: Vec<(usize, Try)> = w
            .results
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.map(|t| (i, t)))
            .collect();
        let report = |s: &Search, void: bool, change: Option<String>| Folded {
            stage,
            axis,
            void,
            before,
            after: s.line.score,
            change,
            state: s.line.state,
            tries: s.line.tries,
            hits: s.line.hits,
        };
        if self.line.state != State::Open {
            return report(&*self, false, None);
        }
        if scored.is_empty() {
            self.voids += 1;
            if w.voids.is_empty() {
                self.void_why.push("no_tries");
            }
            self.void_why.extend(w.voids);
            if self.voids >= VOID_ROUNDS {
                if self.line.tries == 0 {
                    self.line.state = State::Blocked;
                    self.line.why = Some(commonest(&self.void_why).to_string());
                } else {
                    // Flown before: the bot that couldn't get there is
                    // where its last try left it (a launch lands far off),
                    // not the site's fault. The count starts again.
                    self.voids = 0;
                    self.void_why.clear();
                }
            }
            return report(&*self, true, None);
        }
        self.voids = 0;
        self.void_why.clear();
        // The slots as the round flew them, before the evidence moves the
        // try index the scatter drew with.
        let slots = self.slots(&w.results);
        let sides = self.compass_slots();
        // The evidence.
        {
            let l = &mut self.line;
            for (_, t) in &scored {
                l.tries += 1;
                l.hits += u32::from(t.bounced());
                l.deaths += u32::from(t.died);
                if let Some(d) = t.down {
                    l.best = l.best.max(d.vz_out);
                }
            }
        }
        let mut change = None;
        let mut dead = false;
        match stage {
            Stage::Arms => {
                let arms = self.arms();
                let mut best: Option<(usize, f32, Vec<Try>)> = None;
                for (ai, arm) in arms.iter().enumerate() {
                    let ts: Vec<Try> = scored
                        .iter()
                        .filter(|(i, _)| i / ARM_TRIES == ai)
                        .map(|(_, t)| *t)
                        .collect();
                    if ts.is_empty() {
                        continue;
                    }
                    let (_, _, mean) = mean_of(&ts, self.setup.need);
                    let better = match &best {
                        None => true,
                        Some((bi, bm, _)) => {
                            let (p, q) = (self.arm_pref(arm), self.arm_pref(&arms[*bi]));
                            mean > *bm
                                || (mean == *bm && (p.0, p.1.to_bits()) < (q.0, q.1.to_bits()))
                        }
                    };
                    if better {
                        best = Some((ai, mean, ts));
                    }
                }
                if let Some((ai, _, ts)) = best {
                    let k = arms[ai];
                    self.take_centre(k, &ts);
                    change = Some(match k.run {
                        Some(r) => format!("how={} run={r:.2}", k.how.map_or("-", How::name)),
                        None => format!("how={}", k.how.map_or("-", How::name)),
                    });
                }
                if self.line.ok > 0 {
                    self.enter_compass(0);
                } else {
                    let l = &mut self.line;
                    l.stage = Stage::Shift;
                    l.axis = 0;
                }
            }
            Stage::Shift => {
                let centre: Vec<Try> = scored
                    .iter()
                    .filter(|(i, _)| *i < SHIFT_TRIES)
                    .map(|(_, t)| *t)
                    .collect();
                let moved: Vec<Try> = scored
                    .iter()
                    .filter(|(i, _)| *i >= SHIFT_TRIES)
                    .map(|(_, t)| *t)
                    .collect();
                self.add_centre(&centre);
                let mut go_on = false;
                if !moved.is_empty() {
                    let (_, _, mean) = mean_of(&moved, self.setup.need);
                    if mean > self.line.score {
                        let f = self.frame();
                        let old = self.full_centre();
                        let k = slots[SHIFT_TRIES];
                        let d = [
                            self.get(&f, &k, Axis::AimAlong) - self.get(&f, &old, Axis::AimAlong),
                            self.get(&f, &k, Axis::AimSide) - self.get(&f, &old, Axis::AimSide),
                        ];
                        self.take_centre(k, &moved);
                        change = Some(format!("aim {:+.2} along {:+.2} side", d[0], d[1]));
                        self.line.axis += 1;
                        go_on = self.line.axis < SHIFT_ROUNDS && self.line.ok == 0;
                    }
                }
                if !go_on {
                    self.enter_compass(0);
                }
            }
            Stage::Compass => {
                let a = axis.unwrap_or(Axis::AimAlong);
                let of = |side: Side| -> Vec<Try> {
                    scored
                        .iter()
                        .filter(|(i, _)| sides.get(*i).is_some_and(|(s, _)| *s == side))
                        .map(|(_, t)| *t)
                        .collect()
                };
                let (centre, under, over) = (of(Side::Centre), of(Side::Under), of(Side::Over));
                self.add_centre(&centre);
                let mut best: Option<(Side, f32, Vec<Try>)> = None;
                for (side, ts) in [(Side::Under, under), (Side::Over, over)] {
                    if ts.is_empty() {
                        continue;
                    }
                    let (_, _, mean) = mean_of(&ts, self.setup.need);
                    if mean > self.line.score + MARGIN
                        && best.as_ref().is_none_or(|(_, bm, _)| mean > *bm)
                    {
                        best = Some((side, mean, ts));
                    }
                }
                let len = self.axes().len();
                if let Some((side, _, ts)) = best {
                    let k = sides.iter().find(|(s, _)| *s == side).map(|(_, k)| *k);
                    if let Some(k) = k {
                        let f = self.frame();
                        let delta = self.get(&f, &k, a) - self.get(&f, &self.full_centre(), a);
                        self.take_centre(k, &ts);
                        self.line.moved = true;
                        change = Some(describe_move(a, delta));
                    }
                }
                let l = &mut self.line;
                l.axis = (l.axis as usize).min(len - 1) as u32 + 1;
                if l.axis as usize >= len {
                    l.axis = 0;
                    if !l.moved {
                        l.shrink += 1;
                    }
                    l.moved = false;
                    if l.shrink >= SHRINK_END {
                        if l.hits > 0 {
                            // It bounces: keep polishing at the finest steps.
                            l.shrink = SHRINK_END - 1;
                        } else if !l.scattered {
                            l.stage = Stage::Scatter;
                            l.shrink = SHRINK_END;
                        } else {
                            l.shrink = SHRINK_END;
                            dead = true;
                        }
                    }
                }
            }
            Stage::Scatter => {
                let mut best: Option<(usize, f32, Try)> = None;
                for (i, t) in &scored {
                    let s = score(t, self.setup.need);
                    if best.as_ref().is_none_or(|(_, bs, _)| s > *bs) {
                        best = Some((*i, s, *t));
                    }
                }
                if let Some((i, s, t)) = best
                    && (s > self.line.score || self.line.n == 0)
                {
                    let k = slots[i];
                    self.take_centre(k, &[t]);
                    change = Some(format!("scatter draw {i}"));
                }
                self.line.scattered = true;
                self.enter_compass(1);
            }
        }
        // The stop rules, the first that holds.
        let budget = self.setup.budget;
        let l = &mut self.line;
        l.why = None;
        if l.n >= LEARNED_N && l.rate() >= LEARNED_RATE {
            l.state = State::Learned;
        } else if dead {
            l.state = State::Dead;
            l.why = Some("no_bounce".into());
        } else if l.tries >= budget {
            if l.hits == 0 {
                l.state = State::Dead;
                l.why = Some("budget".into());
            } else {
                l.state = State::Partial;
            }
        } else {
            l.state = State::Open;
        }
        self.line = self.line.canonical();
        report(&*self, false, change)
    }
}

fn describe_move(a: Axis, d: f32) -> String {
    match a {
        Axis::AimAlong => format!("aim {d:+.2} along"),
        Axis::AimSide => format!("aim {d:+.2} side"),
        Axis::FromSide => format!("launch {d:+.2} side"),
        Axis::Yaw => format!("yaw {d:+.1}"),
        Axis::Still => format!("still {:+}", d.round() as i32),
        _ => format!("{} {d:+.2}", a.name()),
    }
}

/// The commonest reason, the first by name among equals.
fn commonest(whys: &[&'static str]) -> &'static str {
    let mut best: Option<(&'static str, usize)> = None;
    for &w in whys {
        let n = whys.iter().filter(|x| **x == w).count();
        if best.is_none_or(|(bw, bn)| n > bn || (n == bn && w < bw)) {
            best = Some((w, n));
        }
    }
    best.map_or("void", |(w, _)| w)
}

/// `dead` and `blocked` lines back to `open` (`h2 sim train --reopen`).
pub fn reopen(entries: &mut [Entry]) {
    for e in entries {
        if matches!(e.state, State::Dead | State::Blocked) {
            e.state = State::Open;
            e.why = None;
        }
    }
}

// ---- the board: a training run's sites and bots ----

/// A training run: its shard of the map's sites, the budget and the seed
/// (a sim match's `[train]` table).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Train {
    pub shard: u32,
    pub shards: u32,
    /// Scored tries a site.
    pub budget: u32,
    pub seed: u64,
}

/// Whether the site `spot` belongs to `shard` of `shards`: its id modulo
/// `shards`. One shard (or none) owns every site.
pub fn in_shard(spot: u64, shard: u32, shards: u32) -> bool {
    shards <= 1 || spot % u64::from(shards) == u64::from(shard)
}

/// A bounce site as the board takes it: what [`Setup`] needs besides the
/// batch's numbers. A V's arm comes twice: from the mirror side, and once
/// from its own as the control ([`SiteIn::control`]).
#[derive(Clone, Debug, PartialEq)]
pub struct SiteIn {
    pub at: Vec3,
    pub hows: Vec<How>,
    pub base: Knobs,
    pub run_off: f32,
    pub need: f32,
    /// The arm's control: flown from the arm's own side ([`Setup::control`]).
    pub control: bool,
}

/// One site on the board.
#[derive(Clone, Debug, PartialEq)]
pub struct Held {
    pub search: Search,
    /// The bot flying its round.
    pub claim: Option<u64>,
    /// Rounds flown this run.
    pub rounds: u32,
}

/// A training run's sites: the open ones its bots pick from, a round at a
/// time.
#[derive(Clone, Debug, PartialEq)]
pub struct Board {
    map: String,
    train: Train,
    sites: Vec<Held>,
}

impl Board {
    /// The sites of `train`'s shard, each from its line of `learned` when
    /// it has one. A line goes to the site standing nearest it: a groove
    /// V's bottom line and its arms, 0.05 apart, each keep their own.
    pub fn new(map: &str, train: Train, sites: Vec<SiteIn>, learned: &[Entry]) -> Board {
        let ats: Vec<(Vec3, bool)> = sites.iter().map(|s| (s.at, s.control)).collect();
        let mut held: Vec<Held> = Vec::new();
        for s in sites {
            let spot = if s.control {
                learned::control_id(map, s.at)
            } else {
                spot_id(map, s.at)
            };
            if !in_shard(spot, train.shard, train.shards)
                || held.iter().any(|h| h.search.setup.spot == spot)
            {
                continue;
            }
            let setup = Setup {
                at: s.at,
                spot,
                seed: train.seed,
                budget: train.budget,
                hows: s.hows,
                base: s.base,
                run_off: s.run_off,
                need: s.need,
                control: s.control,
            };
            let line = if s.control {
                learned::control_for(learned, s.at)
            } else {
                learned::entry_for(learned, s.at)
            };
            let line = line.filter(|l| {
                let d = |p: Vec3| (p[0] - l.at[0]).hypot(p[1] - l.at[1]);
                let mine = d(s.at);
                !ats.iter().any(|&(o, c)| c == s.control && d(o) < mine)
            });
            held.push(Held {
                search: Search::new(setup, line),
                claim: None,
                rounds: 0,
            });
        }
        Board {
            map: map.to_string(),
            train,
            sites: held,
        }
    }

    pub fn map(&self) -> &str {
        &self.map
    }

    pub fn train(&self) -> Train {
        self.train
    }

    pub fn sites(&self) -> &[Held] {
        &self.sites
    }

    /// The site `bot` holds, if any.
    pub fn claimed(&self, bot: u64) -> Option<usize> {
        self.sites.iter().position(|h| h.claim == Some(bot))
    }

    /// The site `bot` (standing at `at`) flies next: the one it holds, else
    /// the open, unclaimed site with the fewest rounds flown this run, then
    /// the fewest tries, then the nearest, then the lowest id. It holds it
    /// until [`Board::fold`] or [`Board::release`].
    pub fn pick(&mut self, bot: u64, at: Vec3) -> Option<usize> {
        if let Some(i) = self.claimed(bot) {
            return Some(i);
        }
        let d2 = |p: Vec3| (p[0] - at[0]).powi(2) + (p[1] - at[1]).powi(2) + (p[2] - at[2]).powi(2);
        let i = self
            .sites
            .iter()
            .enumerate()
            .filter(|(_, h)| h.claim.is_none() && h.search.state() == State::Open)
            .min_by(|(_, a), (_, b)| {
                a.rounds
                    .cmp(&b.rounds)
                    .then(a.search.entry().tries.cmp(&b.search.entry().tries))
                    .then(d2(a.search.setup.at).total_cmp(&d2(b.search.setup.at)))
                    .then(a.search.setup.spot.cmp(&b.search.setup.spot))
            })
            .map(|(i, _)| i)?;
        self.sites[i].claim = Some(bot);
        Some(i)
    }

    /// `bot` lets its site go without a round (the round flies again from
    /// the same state).
    pub fn release(&mut self, bot: u64) {
        for h in &mut self.sites {
            if h.claim == Some(bot) {
                h.claim = None;
            }
        }
    }

    /// Site `i`'s next slot ([`Search::next`]).
    pub fn next(&self, i: usize, done: &[Try]) -> Option<Knobs> {
        self.sites.get(i)?.search.next(done)
    }

    /// Ends site `i`'s round ([`Search::fold`]) and lets it go. `None` for
    /// no such site.
    pub fn fold(&mut self, i: usize, done: &[Try]) -> Option<Folded> {
        let h = self.sites.get_mut(i)?;
        let f = h.search.fold(done);
        h.claim = None;
        h.rounds += 1;
        Some(f)
    }

    /// The log line of site `i`'s round: `site (x, y, z) round 7 ...`.
    pub fn describe(&self, i: usize, f: &Folded) -> String {
        let Some(h) = self.sites.get(i) else {
            return f.describe();
        };
        let p = h.search.setup.at;
        format!(
            "site ({:.2}, {:.2}, {:.2}){} round {} {}",
            p[0],
            p[1],
            p[2],
            if h.search.setup.control { " control" } else { "" },
            h.rounds,
            f.describe()
        )
    }

    /// Every site's line.
    pub fn entries(&self) -> Vec<Entry> {
        self.sites
            .iter()
            .map(|h| h.search.entry().clone())
            .collect()
    }

    /// Every site of the shard is closed (or not open: `partial` waits
    /// for a higher budget).
    pub fn done(&self) -> bool {
        self.sites.iter().all(|h| h.search.state() != State::Open)
    }
}

// ---- the process's training run ----

static TRAIN: Mutex<Option<Train>> = Mutex::new(None);
static BOARD: Shared = Mutex::new(None);

/// Where a training run's board is kept: the process's ([`shared`]), or a
/// test's own (`mind::practice`, as its claims board is).
pub type Shared = Mutex<Option<Board>>;

/// The process's board, built at the first pick (`mind::practice`).
pub fn shared() -> &'static Shared {
    &BOARD
}

/// Sets the training run (`None`: none). The board is dropped, to be built
/// at the first pick ([`install`]).
pub fn set(t: Option<Train>) {
    *TRAIN.lock().unwrap_or_else(|e| e.into_inner()) = t;
    *BOARD.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// The training run, when one is set.
pub fn current() -> Option<Train> {
    *TRAIN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Installs the board built from the map's sites and file. Ignored with no
/// training run set.
pub fn install(board: Board) {
    if current().is_some() {
        *BOARD.lock().unwrap_or_else(|e| e.into_inner()) = Some(board);
    }
}

/// Runs `f` on the board, when one is built.
pub fn with_board<R>(f: impl FnOnce(&mut Board) -> R) -> Option<R> {
    BOARD
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .map(f)
}

/// Whether the training run is over: a run is set, its board is built,
/// and every site of its shard is closed. False until the board is built.
pub fn done() -> bool {
    current().is_some() && with_board(|b| b.done()).unwrap_or(false)
}

#[cfg(test)]
mod tests;
