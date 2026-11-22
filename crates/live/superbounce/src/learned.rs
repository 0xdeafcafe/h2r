//! What the bots' bounce search learned of each bounce site: a map's
//! learned file (docs/game/bounce-search.md, "The file: `<map>.learned`").
//!
//! The search (`bots::learn`) keeps one line a bounce site: the best knobs
//! it has found ([`Knobs`]), the evidence for them, and where the search
//! stands, so a batch resumes from the file. A knob a line leaves out is
//! the route's own value. The file is text, one site a line:
//!
//! ```text
//! # superbounce learned v1
//! bounce learned -10.620 3.880 -3.190 how=runoff fx=-8.330 fy=2.540 fz=-0.760 ax=-10.540 ay=4.090 yaw=0.0 back=1.20 run=1.00 gain=1.20 letgo=0.030 tries=64 hits=19 n=12 ok=8 score=1.214 best=20.90 deaths=1 stage=compass axis=2 shrink=1 moved=0 scattered=0
//! ```
//!
//! [`to_text`] writes the keys in a fixed order and the lines sorted by
//! `x`, `y`, `z`, so equal entries give equal bytes. [`parse`] refuses a
//! file of another version, an unknown key, state, stage or `how`, a knob
//! the line's `how` does not use, a key given twice, and a value out of its
//! range, naming the line: a mistyped knob is never skipped quietly.
//! [`merge`] keeps the better of two lines about one site whole, by an
//! order that is total ([`rank`]), so merging in any order gives the same
//! file.

use std::cmp::Ordering;

use crate::geom::segment_xy;
use crate::verified::{MATCH, MATCH_Z};
use crate::{Kind, Spot, Vec3};

/// The file format's version; [`parse`] refuses any other. It rises
/// whenever the format or what the route makes of a knob changes.
pub const VERSION: u32 = 1;
/// The first line, before the version number.
pub const HEADER_PREFIX: &str = "# superbounce learned v";

/// The first line of a file of this version.
pub fn header() -> String {
    format!("{HEADER_PREFIX}{VERSION}")
}

/// How far (world units) the search moves the launch point to the side of
/// the route's own.
pub const FROM_SIDE: f32 = 0.5;
/// How far (world units) the search moves the aim along the launch-to-aim
/// line from the route's own.
pub const AIM_ALONG: f32 = 0.4;
/// How far (world units) the search moves the aim to the side.
pub const AIM_SIDE: f32 = 0.3;
/// The heading's turn off the launch-to-aim line, degrees.
pub const YAW: (f32, f32) = (-20.0, 20.0);
/// How far behind the launch point the run starts, world units.
pub const BACK: (f32, f32) = (0.2, 3.0);
/// The run's stick.
pub const RUN: (f32, f32) = (0.2, 1.0);
/// How far short of the launch point a running jump is pressed, world
/// units.
pub const EARLY: (f32, f32) = (-0.1, 0.4);
/// The air stick's gain.
pub const GAIN: (f32, f32) = (0.0, 3.0);
/// How near the aim the air stick lets go, world units.
pub const LETGO: (f32, f32) = (0.0, 0.15);
/// A standing jump's air stick toward the drift.
pub const DRIFT: (f32, f32) = (0.0, 1.0);
/// Ticks stood still before a standing jump.
pub const STILL: (u32, u32) = (0, 30);
/// A line's mean score at the centre lies in this range (a deadly try
/// scores down to -1, a bounce up to 1.5).
pub const SCORE: (f32, f32) = (-1.0, 1.5);
/// The most `axis` a line may hold (the longest compass pass).
pub const MAX_AXIS: u32 = 9;
/// The most `shrink` a line may hold.
pub const MAX_SHRINK: u32 = 3;

/// How a drop or a stand leaves the floor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum How {
    /// Run off the ledge.
    #[default]
    Runoff,
    /// A running jump off the ledge.
    Jump,
    /// A standing jump on the patch.
    Stand,
}

impl How {
    pub const ALL: [How; 3] = [How::Runoff, How::Jump, How::Stand];

    pub fn name(self) -> &'static str {
        match self {
            How::Runoff => "runoff",
            How::Jump => "jump",
            How::Stand => "stand",
        }
    }

    pub fn parse(s: &str) -> Option<How> {
        How::ALL.into_iter().find(|h| h.name() == s)
    }

    /// A drop: a run-off or a running jump.
    pub fn is_drop(self) -> bool {
        self != How::Stand
    }
}

/// Where a site's search stands (docs/game/bounce-search.md, the stop
/// rules).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum State {
    /// Still searching.
    #[default]
    Open,
    /// The centre bounces often enough.
    Learned,
    /// The budget ran out after some bounce.
    Partial,
    /// No try ever bounced.
    Dead,
    /// Three rounds running said nothing about the knobs.
    Blocked,
}

impl State {
    pub const ALL: [State; 5] = [
        State::Open,
        State::Learned,
        State::Partial,
        State::Dead,
        State::Blocked,
    ];

    pub fn name(self) -> &'static str {
        match self {
            State::Open => "open",
            State::Learned => "learned",
            State::Partial => "partial",
            State::Dead => "dead",
            State::Blocked => "blocked",
        }
    }

    pub fn parse(s: &str) -> Option<State> {
        State::ALL.into_iter().find(|v| v.name() == s)
    }

    /// A batch flies it no more: `learned`, `dead` or `blocked`.
    pub fn closed(self) -> bool {
        matches!(self, State::Learned | State::Dead | State::Blocked)
    }

    /// Its knobs are played: `open`, `partial` or `learned`.
    pub fn plays(self) -> bool {
        matches!(self, State::Open | State::Partial | State::Learned)
    }
}

/// The search's stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Stage {
    #[default]
    Arms,
    Shift,
    Compass,
    Scatter,
}

impl Stage {
    pub const ALL: [Stage; 4] = [Stage::Arms, Stage::Shift, Stage::Compass, Stage::Scatter];

    pub fn name(self) -> &'static str {
        match self {
            Stage::Arms => "arms",
            Stage::Shift => "shift",
            Stage::Compass => "compass",
            Stage::Scatter => "scatter",
        }
    }

    pub fn parse(s: &str) -> Option<Stage> {
        Stage::ALL.into_iter().find(|v| v.name() == s)
    }
}

/// A knob, for [`uses`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Knob {
    From,
    Aim,
    Yaw,
    Back,
    Run,
    Early,
    Gain,
    Letgo,
    Drift,
    Still,
}

/// Whether a line of `how` uses `knob`. With no `how`, every knob.
/// A drop uses the launch point, the aim, `yaw`, `back`, `run`, `gain` and
/// `letgo`, a running jump `early` too; a stand the aim, `drift` and
/// `still`.
pub fn uses(how: Option<How>, knob: Knob) -> bool {
    match how {
        None => true,
        Some(How::Stand) => matches!(knob, Knob::Aim | Knob::Drift | Knob::Still),
        Some(h) => match knob {
            Knob::From
            | Knob::Aim
            | Knob::Yaw
            | Knob::Back
            | Knob::Run
            | Knob::Gain
            | Knob::Letgo => true,
            Knob::Early => h == How::Jump,
            Knob::Drift | Knob::Still => false,
        },
    }
}

/// One bounce site's knobs. `None` is the route's own value
/// (`bots::bounce_route::flown`); `Knobs::default()` plays exactly as the
/// route does.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Knobs {
    pub how: Option<How>,
    /// The launch point (`fx fy fz`).
    pub from: Option<Vec3>,
    /// The aim (`ax ay`).
    pub aim: Option<[f32; 2]>,
    /// Degrees off the launch-to-aim line.
    pub yaw: Option<f32>,
    pub back: Option<f32>,
    pub run: Option<f32>,
    pub early: Option<f32>,
    pub gain: Option<f32>,
    pub letgo: Option<f32>,
    pub drift: Option<f32>,
    pub still: Option<u32>,
}

/// `v` to `places` decimals as the file prints it, with no negative zero.
pub fn quantise(v: f32, places: usize) -> f32 {
    format!("{v:.places$}").parse::<f32>().unwrap_or(v) + 0.0
}

fn clamp(v: f32, r: (f32, f32)) -> f32 {
    v.clamp(r.0, r.1)
}

impl Knobs {
    /// Whether no knob is set: the route's own everywhere.
    pub fn is_default(&self) -> bool {
        *self == Knobs::default()
    }

    /// These knobs, each unset one taken from `base`.
    pub fn or(self, base: Knobs) -> Knobs {
        Knobs {
            how: self.how.or(base.how),
            from: self.from.or(base.from),
            aim: self.aim.or(base.aim),
            yaw: self.yaw.or(base.yaw),
            back: self.back.or(base.back),
            run: self.run.or(base.run),
            early: self.early.or(base.early),
            gain: self.gain.or(base.gain),
            letgo: self.letgo.or(base.letgo),
            drift: self.drift.or(base.drift),
            still: self.still.or(base.still),
        }
    }

    /// Only the knobs this vector's `how` uses ([`uses`]).
    pub fn for_how(self) -> Knobs {
        let u = |k| uses(self.how, k);
        Knobs {
            how: self.how,
            from: self.from.filter(|_| u(Knob::From)),
            aim: self.aim.filter(|_| u(Knob::Aim)),
            yaw: self.yaw.filter(|_| u(Knob::Yaw)),
            back: self.back.filter(|_| u(Knob::Back)),
            run: self.run.filter(|_| u(Knob::Run)),
            early: self.early.filter(|_| u(Knob::Early)),
            gain: self.gain.filter(|_| u(Knob::Gain)),
            letgo: self.letgo.filter(|_| u(Knob::Letgo)),
            drift: self.drift.filter(|_| u(Knob::Drift)),
            still: self.still.filter(|_| u(Knob::Still)),
        }
    }

    /// Each set knob held to its range (the points are not held: their
    /// range is round the route's own, which the file does not know).
    pub fn clamped(self) -> Knobs {
        Knobs {
            yaw: self.yaw.map(|v| clamp(v, YAW)),
            back: self.back.map(|v| clamp(v, BACK)),
            run: self.run.map(|v| clamp(v, RUN)),
            early: self.early.map(|v| clamp(v, EARLY)),
            gain: self.gain.map(|v| clamp(v, GAIN)),
            letgo: self.letgo.map(|v| clamp(v, LETGO)),
            drift: self.drift.map(|v| clamp(v, DRIFT)),
            still: self.still.map(|v| v.clamp(STILL.0, STILL.1)),
            ..self
        }
    }

    /// Each set knob as the file prints it: what reading the line back
    /// gives.
    pub fn rounded(self) -> Knobs {
        let p3 = |v: f32| quantise(v, 3);
        Knobs {
            how: self.how,
            from: self.from.map(|f| [p3(f[0]), p3(f[1]), p3(f[2])]),
            aim: self.aim.map(|a| [p3(a[0]), p3(a[1])]),
            yaw: self.yaw.map(|v| quantise(v, 1)),
            back: self.back.map(|v| quantise(v, 2)),
            run: self.run.map(|v| quantise(v, 2)),
            early: self.early.map(p3),
            gain: self.gain.map(|v| quantise(v, 2)),
            letgo: self.letgo.map(p3),
            drift: self.drift.map(|v| quantise(v, 2)),
            still: self.still,
        }
    }

    /// The knobs as the file would hold them: the `how`'s own, in range,
    /// rounded.
    pub fn canonical(self) -> Knobs {
        self.for_how().clamped().rounded()
    }
}

/// What the search made of one bounce site: one line of the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub state: State,
    /// The site's `stand`, as the scan gave it.
    pub at: Vec3,
    /// The centre: the best knobs found.
    pub knobs: Knobs,
    /// Scored tries, and of them how many bounced.
    pub tries: u32,
    pub hits: u32,
    /// Tries flown at exactly the centre, of them how many bounced, and
    /// their mean score.
    pub n: u32,
    pub ok: u32,
    pub score: f32,
    /// The best launch seen (units/s up); 0 for none.
    pub best: f32,
    /// Tries the bot died in.
    pub deaths: u32,
    /// A `dead` or `blocked` line's reason.
    pub why: Option<String>,
    /// A control line (`control=1`): a V's arm flown from its own side,
    /// not the mirror side its spot is approached from
    /// (docs/game/bounce-search.md, "Vs: the mirror and its control"). It
    /// is a site of its own beside the arm's line at the same `at`, and
    /// evidence only: [`entry_for`], [`knobs_for`] and [`apply`] never
    /// take it.
    pub control: bool,
    /// The cursor: the stage, the next axis of the pass (in `shift`, the
    /// shift rounds flown), how often the steps have halved, whether this
    /// pass has moved the centre, whether the scatter has run.
    pub stage: Stage,
    pub axis: u32,
    pub shrink: u32,
    pub moved: bool,
    pub scattered: bool,
}

impl Entry {
    /// Whether this line is about the bounce site standing at `stand`.
    pub fn is_of(&self, stand: Vec3) -> bool {
        near(self.at, stand)
    }

    /// The line as [`parse`] reads it back.
    pub fn canonical(&self) -> Entry {
        parse_line(&line(self)).unwrap_or_else(|_| self.clone())
    }

    /// `ok / n`: the bounce rate at the centre (0 with no tries).
    pub fn rate(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            f64::from(self.ok) / f64::from(self.n)
        }
    }
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a[0] - b[0]).hypot(a[1] - b[1]) <= MATCH && (a[2] - b[2]).abs() <= MATCH_Z
}

/// The site's id: FNV-1a 64 of `"<map> <x> <y> <z>"`, the site's `stand`
/// printed as the file prints it. It shards a batch and seeds its draws.
pub fn spot_id(map: &str, stand: Vec3) -> u64 {
    fnv(&format!("{map} {}", key(stand)))
}

/// A control site's id ([`Entry::control`]): [`spot_id`]'s text with
/// ` control` after it, so the arm and its control are two sites.
pub fn control_id(map: &str, stand: Vec3) -> u64 {
    fnv(&format!("{map} {} control", key(stand)))
}

fn fnv(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `v` printed to `places` decimals, with no negative zero.
fn num(v: f32, places: usize) -> String {
    format!("{:.places$}", v + 0.0)
        .parse::<f32>()
        .map(|q| format!("{:.places$}", q + 0.0))
        .unwrap_or_else(|_| format!("{v:.places$}"))
}

/// A site's position as the file prints it.
fn key(at: Vec3) -> String {
    format!("{} {} {}", num(at[0], 3), num(at[1], 3), num(at[2], 3))
}

/// What two lines are matched by when merged: the position, and whether
/// it is a control ([`Entry::control`]).
fn site_key(e: &Entry) -> (String, bool) {
    (key(e.at), e.control)
}

/// One line of the file, with no newline.
pub fn line(e: &Entry) -> String {
    let mut o = format!("bounce {} {}", e.state.name(), key(e.at));
    let k = e.knobs.for_how();
    let mut kv = |key: &str, v: String| {
        o.push(' ');
        o.push_str(key);
        o.push('=');
        o.push_str(&v);
    };
    if e.control {
        kv("control", "1".into());
    }
    if let Some(h) = k.how {
        kv("how", h.name().into());
    }
    if let Some(f) = k.from {
        kv("fx", num(f[0], 3));
        kv("fy", num(f[1], 3));
        kv("fz", num(f[2], 3));
    }
    if let Some(a) = k.aim {
        kv("ax", num(a[0], 3));
        kv("ay", num(a[1], 3));
    }
    for (name, v, p) in [
        ("yaw", k.yaw, 1),
        ("back", k.back, 2),
        ("run", k.run, 2),
        ("early", k.early, 3),
        ("gain", k.gain, 2),
        ("letgo", k.letgo, 3),
        ("drift", k.drift, 2),
    ] {
        if let Some(v) = v {
            kv(name, num(v, p));
        }
    }
    if let Some(s) = k.still {
        kv("still", s.to_string());
    }
    kv("tries", e.tries.to_string());
    kv("hits", e.hits.to_string());
    kv("n", e.n.to_string());
    kv("ok", e.ok.to_string());
    kv("score", num(e.score, 3));
    if e.best > 0.0 {
        kv("best", num(e.best, 2));
    }
    if e.deaths > 0 {
        kv("deaths", e.deaths.to_string());
    }
    if let Some(w) = &e.why {
        kv("why", w.clone());
    }
    kv("stage", e.stage.name().into());
    kv("axis", e.axis.to_string());
    kv("shrink", e.shrink.to_string());
    kv("moved", u8::from(e.moved).to_string());
    kv("scattered", u8::from(e.scattered).to_string());
    o
}

/// The order lines are written in: by `x`, `y`, `z`, then by text.
fn order(a: &Entry, b: &Entry) -> Ordering {
    a.at[0]
        .total_cmp(&b.at[0])
        .then(a.at[1].total_cmp(&b.at[1]))
        .then(a.at[2].total_cmp(&b.at[2]))
        .then_with(|| line(a).cmp(&line(b)))
}

/// The text of a learned file: [`header`], then one line an entry, sorted
/// by `x`, `y`, `z`.
pub fn to_text(entries: &[Entry]) -> String {
    let mut sorted: Vec<&Entry> = entries.iter().collect();
    sorted.sort_by(|a, b| order(a, b));
    let mut o = header();
    o.push('\n');
    for e in sorted {
        o.push_str(&line(e));
        o.push('\n');
    }
    o
}

/// The entries of a learned file. The first line that is not blank must be
/// [`header`]; an empty text is an empty file. An error names the line
/// (from 1) and what is wrong with it; a refused file is read as empty by
/// its callers, which log the error.
pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty());
    let Some((n, first)) = lines.next() else {
        return Ok(Vec::new());
    };
    let Some(v) = first.strip_prefix(HEADER_PREFIX) else {
        return Err(format!(
            "line {n}: not a learned file (want {:?})",
            header()
        ));
    };
    match v.trim().parse::<u32>() {
        Ok(VERSION) => {}
        Ok(other) => {
            return Err(format!(
                "line {n}: a learned file of version {other}; this build reads version {VERSION}"
            ));
        }
        Err(_) => return Err(format!("line {n}: version {v:?} is not a number")),
    }
    let mut out = Vec::new();
    for (n, l) in lines {
        if l.starts_with('#') {
            continue;
        }
        out.push(parse_line(l).map_err(|e| format!("line {n}: {e}"))?);
    }
    Ok(out)
}

fn number(s: &str, what: &str) -> Result<f32, String> {
    s.parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .map(|v| v + 0.0)
        .ok_or_else(|| format!("{what} {s:?} is not a number"))
}

fn in_range(k: &str, v: f32, r: (f32, f32)) -> Result<f32, String> {
    if v < r.0 || v > r.1 {
        Err(format!("{k}={v} is out of its range {} to {}", r.0, r.1))
    } else {
        Ok(v)
    }
}

fn parse_line(l: &str) -> Result<Entry, String> {
    let mut t = l.split_whitespace();
    match t.next() {
        Some("bounce") => {}
        other => return Err(format!("{other:?}: want bounce")),
    }
    let state = match t.next() {
        Some(s) => State::parse(s)
            .ok_or_else(|| format!("state {s:?}: open, learned, partial, dead or blocked"))?,
        None => return Err("no state".into()),
    };
    let mut xyz = |what: &str| -> Result<f32, String> {
        number(t.next().ok_or_else(|| format!("no {what}"))?, what)
    };
    let at = [xyz("x")?, xyz("y")?, xyz("z")?];
    let mut e = Entry {
        state,
        at,
        ..Entry::default()
    };
    let mut seen: Vec<&str> = Vec::new();
    let (mut fx, mut fy, mut fz, mut ax, mut ay) = (None, None, None, None, None);
    for kv in t {
        let Some((k, v)) = kv.split_once('=') else {
            return Err(format!("{kv:?} is not key=value"));
        };
        if seen.contains(&k) {
            return Err(format!("{k} given twice"));
        }
        seen.push(k);
        let f = || number(v, k).map_err(|_| format!("{k}={v:?} is not a number"));
        let n = || {
            v.parse::<u32>()
                .map_err(|_| format!("{k}={v:?} is not a count"))
        };
        let flag = || match v {
            "0" => Ok(false),
            "1" => Ok(true),
            _ => Err(format!("{k}={v:?}: 0 or 1")),
        };
        match k {
            "how" => {
                e.knobs.how =
                    Some(How::parse(v).ok_or_else(|| format!("how {v:?}: runoff, jump or stand"))?)
            }
            "fx" => fx = Some(f()?),
            "fy" => fy = Some(f()?),
            "fz" => fz = Some(f()?),
            "ax" => ax = Some(f()?),
            "ay" => ay = Some(f()?),
            "yaw" => e.knobs.yaw = Some(in_range(k, f()?, YAW)?),
            "back" => e.knobs.back = Some(in_range(k, f()?, BACK)?),
            "run" => e.knobs.run = Some(in_range(k, f()?, RUN)?),
            "early" => e.knobs.early = Some(in_range(k, f()?, EARLY)?),
            "gain" => e.knobs.gain = Some(in_range(k, f()?, GAIN)?),
            "letgo" => e.knobs.letgo = Some(in_range(k, f()?, LETGO)?),
            "drift" => e.knobs.drift = Some(in_range(k, f()?, DRIFT)?),
            "still" => {
                let s = n()?;
                if s > STILL.1 {
                    return Err(format!("still={s} is out of its range 0 to {}", STILL.1));
                }
                e.knobs.still = Some(s);
            }
            "tries" => e.tries = n()?,
            "hits" => e.hits = n()?,
            "n" => e.n = n()?,
            "ok" => e.ok = n()?,
            "score" => e.score = in_range(k, f()?, SCORE)?,
            "best" => {
                let b = f()?;
                if b < 0.0 {
                    return Err(format!("best={v} is under 0"));
                }
                e.best = b;
            }
            "deaths" => e.deaths = n()?,
            "why" => {
                let ok = !v.is_empty()
                    && v.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
                if !ok {
                    return Err(format!("why={v:?}: lower-case letters, digits and _"));
                }
                e.why = Some(v.to_string());
            }
            "stage" => {
                e.stage = Stage::parse(v)
                    .ok_or_else(|| format!("stage {v:?}: arms, shift, compass or scatter"))?
            }
            "axis" => {
                e.axis = n()?;
                if e.axis >= MAX_AXIS {
                    return Err(format!("axis={} is not under {MAX_AXIS}", e.axis));
                }
            }
            "shrink" => {
                e.shrink = n()?;
                if e.shrink > MAX_SHRINK {
                    return Err(format!("shrink={} is over {MAX_SHRINK}", e.shrink));
                }
            }
            "control" => e.control = flag()?,
            "moved" => e.moved = flag()?,
            "scattered" => e.scattered = flag()?,
            _ => return Err(format!("unknown key {k:?}")),
        }
    }
    e.knobs.from = match (fx, fy, fz) {
        (Some(x), Some(y), Some(z)) => Some([x, y, z]),
        (None, None, None) => None,
        _ => return Err("fx, fy and fz go together".into()),
    };
    e.knobs.aim = match (ax, ay) {
        (Some(x), Some(y)) => Some([x, y]),
        (None, None) => None,
        _ => return Err("ax and ay go together".into()),
    };
    let k = e.knobs;
    for (knob, set, name) in [
        (Knob::From, k.from.is_some(), "fx"),
        (Knob::Aim, k.aim.is_some(), "ax"),
        (Knob::Yaw, k.yaw.is_some(), "yaw"),
        (Knob::Back, k.back.is_some(), "back"),
        (Knob::Run, k.run.is_some(), "run"),
        (Knob::Early, k.early.is_some(), "early"),
        (Knob::Gain, k.gain.is_some(), "gain"),
        (Knob::Letgo, k.letgo.is_some(), "letgo"),
        (Knob::Drift, k.drift.is_some(), "drift"),
        (Knob::Still, k.still.is_some(), "still"),
    ] {
        if set && !uses(k.how, knob) {
            let how = k.how.map_or("none", How::name);
            return Err(format!("{name} is not a knob of how={how}"));
        }
    }
    if e.hits > e.tries {
        return Err(format!("hits={} is over tries={}", e.hits, e.tries));
    }
    if e.ok > e.n {
        return Err(format!("ok={} is over n={}", e.ok, e.n));
    }
    if e.why.is_some() && !matches!(e.state, State::Dead | State::Blocked) {
        return Err(format!("why= on a {} line", e.state.name()));
    }
    Ok(e)
}

/// The line about the site standing at `stand`, the nearest when several
/// match. Never a control line ([`control_for`]).
pub fn entry_for(entries: &[Entry], stand: Vec3) -> Option<&Entry> {
    nearest_line(entries, stand, false)
}

/// The control line ([`Entry::control`]) about the arm standing at
/// `stand`, the nearest when several match.
pub fn control_for(entries: &[Entry], stand: Vec3) -> Option<&Entry> {
    nearest_line(entries, stand, true)
}

fn nearest_line(entries: &[Entry], stand: Vec3, control: bool) -> Option<&Entry> {
    entries
        .iter()
        .filter(|e| e.control == control && e.is_of(stand))
        .min_by(|a, b| {
            let d = |e: &Entry| (e.at[0] - stand[0]).hypot(e.at[1] - stand[1]);
            d(a).total_cmp(&d(b)).then_with(|| order(a, b))
        })
}

/// The map's learned lines applied to scanned `spots`: the sim's measured
/// aim outranks the scan's. A bounce spot whose line is `learned` or
/// `partial` with a hit (some try bounced) and has an aim (`ax ay`) has
/// its aim moved onto it: `stand` to the aim (its height kept). `edge`
/// stays the collision edge it is: the aim is where on and across it to
/// land, the edge what the catch is measured from. Its status is left alone: a line never proves a site
/// (the training writes the word; only a primed game run proves,
/// [`crate::verified`]). Other lines and crouch spots are left as they
/// are.
///
/// A line belongs to one spot: of the bounce spots it is about
/// ([`Entry::is_of`]), the one whose target line passes nearest its aim
/// (with no aim, the one standing nearest it); between spots on one edge
/// (a groove V's bottom line and its arms), the one standing nearest the
/// line's `at`. Ascension's slot (target
/// y 4.000) and its crease candidate (y 3.800) are both within reach of
/// the line at y 3.88; its aim, y 3.985, is on the slot's seam, so the
/// slot takes it and the crease stays where it is. Control lines are
/// never applied.
pub fn apply(spots: &mut [Spot], entries: &[Entry]) {
    if entries.is_empty() {
        return;
    }
    let before: Vec<Spot> = spots.to_vec();
    let owner = |e: &Entry, s: &Spot| {
        let at = (e.at[0] - s.stand[0]).hypot(e.at[1] - s.stand[1]);
        match e.knobs.aim {
            Some(aim) => (segment_xy(s.edge, aim), at),
            None => (at, 0.0),
        }
    };
    for (i, s) in spots.iter_mut().enumerate() {
        if s.kind != Kind::Bounce {
            continue;
        }
        let Some(e) = entry_for(entries, s.stand) else {
            continue;
        };
        if !matches!(e.state, State::Learned | State::Partial) || e.hits == 0 {
            continue;
        }
        let mine = owner(e, &before[i]);
        let taken = before.iter().enumerate().any(|(k, o)| {
            k != i && o.kind == Kind::Bounce && e.is_of(o.stand) && owner(e, o) < mine
        });
        if taken {
            continue;
        }
        if let Some(aim) = e.knobs.aim {
            retarget(s, aim);
        }
    }
}

/// `s`'s aim moved onto `aim`: `stand` there, its height kept, a
/// measured landing ([`Spot::landed`]); the edge is left as the map's
/// collision has it.
fn retarget(s: &mut Spot, aim: [f32; 2]) {
    s.stand = [aim[0], aim[1], s.stand[2]];
    s.landed = true;
}

/// The knobs the route plays at the site standing at `stand`: its line's,
/// when the line is `open`, `partial` or `learned`; else none set (the
/// route's own).
pub fn knobs_for(entries: &[Entry], stand: Vec3) -> Knobs {
    entry_for(entries, stand)
        .filter(|e| e.state.plays())
        .map_or_else(Knobs::default, |e| e.knobs.for_how())
}

/// How `a` compares with `b` as a line about one site: `Greater` when `a`
/// is the better. In order: `learned` over not; some `hits` over none;
/// between two `learned`, the higher `ok / n`; more `tries`; the higher
/// `score`; the line that sorts first as text. Total: two lines are equal
/// only when their text is.
pub fn rank(a: &Entry, b: &Entry) -> Ordering {
    let learned = |e: &Entry| e.state == State::Learned;
    learned(a)
        .cmp(&learned(b))
        .then((a.hits > 0).cmp(&(b.hits > 0)))
        .then_with(|| {
            if learned(a) && learned(b) {
                a.rate().total_cmp(&b.rate())
            } else {
                Ordering::Equal
            }
        })
        .then(a.tries.cmp(&b.tries))
        .then(a.score.total_cmp(&b.score))
        .then_with(|| line(b).cmp(&line(a)))
}

/// `old` with `new` merged in: of the lines about one site (one printed
/// `x y z`, a control apart from the arm's own line), the better ([`rank`]) is kept whole; the lines sorted as
/// [`to_text`] writes them. The same in any order, and merging twice adds
/// nothing.
pub fn merge(old: &[Entry], new: &[Entry]) -> Vec<Entry> {
    let mut best: Vec<((String, bool), Entry)> = Vec::new();
    for e in old.iter().chain(new) {
        let k = site_key(e);
        match best.iter_mut().find(|(bk, _)| *bk == k) {
            Some((_, b)) => {
                if rank(e, b) == Ordering::Greater {
                    *b = e.clone();
                }
            }
            None => best.push((k, e.clone())),
        }
    }
    let mut out: Vec<Entry> = best.into_iter().map(|(_, e)| e).collect();
    out.sort_by(order);
    out
}

/// `entries` with `line` in place of every line about its site (one
/// printed `x y z`, a control apart), sorted: what a run writes after a round (the same
/// search, further on, so it replaces the old line outright).
pub fn replace(entries: &[Entry], line: Entry) -> Vec<Entry> {
    let k = site_key(&line);
    let mut out: Vec<Entry> = entries.iter().filter(|e| site_key(e) != k).cloned().collect();
    out.push(line);
    out.sort_by(order);
    out
}

#[cfg(test)]
mod tests;
