//! A batch's plan: which spots, and each trial's start, from
//! `superbounce.verify`'s params and the scan (docs/game/superbounce.md,
//! "Verifying spots"). Pure: the tests drive it with made-up spots.

use agent_proto::superbounce::{VerifyParams, VerifyPlanned, VerifyPoint, VerifySpot};
use superbounce::{Kind, Spot, Status, Vec3};

/// With no `spots` or `points`: this many of the scan's, best first.
pub const MAX_SPOTS: usize = 12;
/// The most trials a batch may plan.
pub const MAX_TRIALS: usize = 400;
/// Crouch: the approach turns (degrees from straight in, against the way
/// out) and the distances outside the lip a trial starts at.
pub const TURNS: [f32; 3] = [0.0, 25.0, -25.0];
pub const DISTANCES: [f32; 2] = [0.3, 0.6];
/// Drop: offsets across from the spot's target line, the seam (toward
/// the drift), centred on it, and along it from its middle, and the
/// heights fallen from.
pub const ACROSS: [f32; 5] = [-0.1, -0.05, 0.0, 0.05, 0.1];
pub const ALONG: [f32; 1] = [0.0];
pub const HEIGHTS: [f32; 3] = [1.5, 2.5, 3.5];
/// Tick defaults ([`VerifyParams`]'s fields of those names).
pub const SETTLE_TICKS: u32 = 20;
pub const HOLD_TICKS: u32 = 15;
pub const OUT_TICKS: u32 = 30;
pub const FALL_TICKS: u32 = 120;
pub const ROUTE_TICKS: u32 = 3600;
pub const REPEAT: u32 = 3;
/// Tick caps: a param over one is refused.
pub const MAX_SETTLE: u32 = 120;
pub const MAX_HOLD: u32 = 120;
pub const MAX_OUT: u32 = 300;
pub const MAX_FALL: u32 = 600;
pub const MAX_ROUTE: u32 = 36_000;
pub const MAX_REPEAT: u32 = 20;
/// An explicit drop point's defaults: slot width, touchdown speed needed,
/// and the length of the midline drawn through `stand`.
pub const POINT_WIDTH: f32 = 0.2;
pub const POINT_NEED: f32 = 4.3;
pub const POINT_EDGE: f32 = 0.3;
/// A route's default target: this far straight up from the strip's stand,
/// so only a launch gets there.
pub const ROUTE_RISE: f32 = 6.0;
/// A teleport puts the origin this far over the planned point, so the feet
/// do not start in the floor. A guess; the settle drops it back.
pub const TELEPORT_LIFT: f32 = 0.05;
/// Crouch: how far past `stand` (along the way in) the walk in aims, as
/// the bots' walking prime does (`bots::glitches::prime::PUSH_IN`).
pub const PUSH_IN: f32 = bots::glitches::prime::PUSH_IN;

/// What a batch tries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrialKind {
    /// A forced crouch: crouch outside the lip, walk in, let go under it,
    /// walk out; does the word turn -3?
    Crouch,
    /// A fall onto a strip with the word set: does it launch?
    Drop,
    /// A bot's whole bounce route.
    Route,
}

impl TrialKind {
    pub fn parse(s: &str) -> Option<TrialKind> {
        match s {
            "crouch" => Some(TrialKind::Crouch),
            "drop" => Some(TrialKind::Drop),
            "route" => Some(TrialKind::Route),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TrialKind::Crouch => "crouch",
            TrialKind::Drop => "drop",
            TrialKind::Route => "route",
        }
    }

    /// The kind of spot it tries.
    pub fn spot_kind(self) -> Kind {
        match self {
            TrialKind::Crouch => Kind::Crouch,
            TrialKind::Drop | TrialKind::Route => Kind::Bounce,
        }
    }
}

/// A drop's word before the landing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prime {
    /// Written -3.
    Set,
    /// Written 0: the control.
    None,
    /// Left as the game has it.
    Keep,
}

impl Prime {
    pub fn parse(s: &str) -> Option<Prime> {
        match s {
            "set" => Some(Prime::Set),
            "none" => Some(Prime::None),
            "keep" => Some(Prime::Keep),
            _ => None,
        }
    }
}

/// A batch's settings, defaults filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub kind: TrialKind,
    pub prime: Prime,
    pub drift_stick: f32,
    pub settle: u32,
    pub hold: u32,
    pub out: u32,
    pub fall: u32,
    pub route: u32,
    pub mode: bots::bounce_route::Mode,
    pub trace: Option<usize>,
    /// Write the verified file at the end.
    pub save: bool,
    /// The spots are explicit points.
    pub points: bool,
}

/// One spot of the batch: the scan's (by its index in [`candidates`]) or
/// an explicit point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub index: Option<usize>,
    pub spot: Spot,
}

impl Target {
    pub fn view(&self) -> VerifySpot {
        VerifySpot {
            index: self.index,
            kind: kind_name(self.spot.kind).into(),
            stand: self.spot.stand,
            status: self.spot.status.name().into(),
            score: self.spot.score,
        }
    }

    /// `#3`, or `#p0` for an explicit point (by its place in the batch).
    pub fn label(&self, at: usize) -> String {
        match self.index {
            Some(i) => format!("#{i}"),
            None => format!("#p{at}"),
        }
    }
}

/// One trial.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trial {
    /// Index into the batch's targets.
    pub spot: usize,
    /// Crouch: the start, outside the lip (teleported to). Drop: the drop
    /// origin, `height` over `point`. Route: the route's target.
    pub start: Vec3,
    /// Crouch: where the walk in aims, just past `stand`. Drop: the rest
    /// point on the strip a drop is lifted from. Route: the strip's stand.
    pub point: Vec3,
    /// The facing, radians about +Z from +X: crouch, the way in; drop, the
    /// drift.
    pub yaw: f32,
    /// Crouch: the unit direction walked in (X and Y).
    pub way_in: [f32; 2],
    /// Crouch: the unit way out.
    pub way_out: [f32; 2],
    pub distance: Option<f32>,
    /// Crouch: X and Y from the spot's start; drop: across and along.
    pub offset: [f32; 2],
    pub height: Option<f32>,
    /// Drop: the drift direction (unit X and Y).
    pub drift: [f32; 2],
}

impl Trial {
    pub fn view(&self) -> VerifyPlanned {
        VerifyPlanned {
            spot: self.spot,
            start: self.start,
            yaw: (self.distance.is_some() || self.height.is_some()).then(|| degrees(self.yaw)),
            distance: self.distance,
            offset: self.offset,
            height: self.height,
        }
    }
}

/// A batch's plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub config: Config,
    pub targets: Vec<Target>,
    pub trials: Vec<Trial>,
}

pub fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Bounce => "bounce",
        Kind::Crouch => "crouch",
    }
}

pub fn degrees(rad: f32) -> f32 {
    let d = rad.to_degrees();
    (d * 10.0).round() / 10.0
}

fn unit(v: [f32; 2]) -> Option<[f32; 2]> {
    let l = v[0].hypot(v[1]);
    (l.is_finite() && l > 1e-4).then(|| [v[0] / l, v[1] / l])
}

fn turn(d: [f32; 2], deg: f32) -> [f32; 2] {
    let (s, c) = deg.to_radians().sin_cos();
    [d[0] * c - d[1] * s, d[0] * s + d[1] * c]
}

/// The scan's spots of `kind`, best (highest score) first: the order the
/// batch's `spots` indices count in.
pub fn candidates(scanned: &[Spot], kind: Kind) -> Vec<Spot> {
    let mut v: Vec<Spot> = scanned.iter().filter(|s| s.kind == kind).copied().collect();
    v.sort_by(|a, b| b.score.total_cmp(&a.score));
    v
}

/// An explicit point as a spot of `kind`.
pub fn point_spot(p: &VerifyPoint, kind: Kind) -> Result<Spot, String> {
    fn finite(v: &[f32]) -> bool {
        v.iter().all(|c| c.is_finite())
    }
    if !finite(&p.stand[..]) {
        return Err("points: stand must be three finite numbers".into());
    }
    match kind {
        Kind::Crouch => {
            let out = match p.out {
                Some(o) => unit(o).ok_or("points: out must be a non-zero direction")?,
                None => [1.0, 0.0],
            };
            Ok(Spot {
                kind,
                edge: [p.stand, p.stand],
                stand: p.stand,
                out,
                score: 0.0,
                ..Spot::default()
            })
        }
        Kind::Bounce => {
            let drift = match p.drift {
                Some(d) => unit(d).ok_or("points: drift must be a non-zero direction")?,
                None => [0.0, 1.0],
            };
            let edge = match p.edge {
                Some(e) if e.iter().all(|v| finite(&v[..])) => e,
                Some(_) => return Err("points: edge must be finite".into()),
                None => {
                    // Along the strip: across the drift.
                    let along = [drift[1], -drift[0]];
                    let h = POINT_EDGE / 2.0;
                    let s = p.stand;
                    [
                        [s[0] - along[0] * h, s[1] - along[1] * h, s[2]],
                        [s[0] + along[0] * h, s[1] + along[1] * h, s[2]],
                    ]
                }
            };
            let width = p.width.unwrap_or(POINT_WIDTH);
            let need = p.need.unwrap_or(POINT_NEED);
            if !(width.is_finite() && width > 0.0 && need.is_finite() && need >= 0.0) {
                return Err("points: width must be over 0 and need at least 0".into());
            }
            Ok(Spot {
                kind,
                edge,
                stand: p.stand,
                drift,
                width,
                need,
                score: 0.0,
                ..Spot::default()
            })
        }
    }
}

fn list(
    v: &Option<Vec<f32>>,
    default: &[f32],
    name: &str,
    lo: f32,
    hi: f32,
) -> Result<Vec<f32>, String> {
    let v = v.clone().unwrap_or_else(|| default.to_vec());
    if v.is_empty() {
        return Err(format!("{name}: at least one value"));
    }
    if let Some(bad) = v
        .iter()
        .find(|x| !(x.is_finite() && **x >= lo && **x <= hi))
    {
        return Err(format!("{name}: {bad} is outside {lo}..{hi}"));
    }
    Ok(v)
}

fn ticks(v: Option<u32>, default: u32, name: &str, max: u32) -> Result<u32, String> {
    match v.unwrap_or(default) {
        0 => Err(format!("{name}: at least 1")),
        t if t > max => Err(format!("{name}: at most {max}")),
        t => Ok(t),
    }
}

/// The batch `p` asks for, over the scan's spots `scanned`.
pub fn plan(p: &VerifyParams, scanned: &[Spot]) -> Result<Plan, String> {
    let kind = TrialKind::parse(&p.kind)
        .ok_or_else(|| format!("kind {:?}: crouch, drop or route", p.kind))?;
    let prime = match p.prime.as_deref() {
        None => Prime::Set,
        Some(s) => Prime::parse(s).ok_or_else(|| format!("prime {s:?}: set, none or keep"))?,
    };
    let mode = match p.mode.as_deref() {
        None => bots::bounce_route::Mode::Fun,
        Some(m) => {
            bots::bounce_route::Mode::parse(m).ok_or_else(|| format!("mode {m:?}: help or fun"))?
        }
    };
    let drift_stick = p.drift_stick.unwrap_or(0.0);
    if !(drift_stick.is_finite() && (0.0..=1.0).contains(&drift_stick)) {
        return Err("drift_stick: 0..1".into());
    }
    if let Some(to) = p.to
        && !to.iter().all(|v| v.is_finite())
    {
        return Err("to: three finite numbers".into());
    }
    let repeat = ticks(p.repeat, REPEAT, "repeat", MAX_REPEAT)?;
    let config = Config {
        kind,
        prime,
        drift_stick,
        settle: ticks(p.settle_ticks, SETTLE_TICKS, "settle_ticks", MAX_SETTLE)?,
        hold: ticks(p.hold_ticks, HOLD_TICKS, "hold_ticks", MAX_HOLD)?,
        out: ticks(p.out_ticks, OUT_TICKS, "out_ticks", MAX_OUT)?,
        fall: ticks(p.fall_ticks, FALL_TICKS, "fall_ticks", MAX_FALL)?,
        route: ticks(p.route_ticks, ROUTE_TICKS, "route_ticks", MAX_ROUTE)?,
        mode,
        trace: p.trace,
        save: p.save.unwrap_or(true) && p.points.is_none() && kind != TrialKind::Route,
        points: p.points.is_some(),
    };
    let targets = targets(p, scanned, kind)?;
    let mut trials = Vec::new();
    match kind {
        TrialKind::Crouch => {
            let turns = list(&p.turns, &TURNS, "turns", -90.0, 90.0)?;
            let distances = list(&p.distances, &DISTANCES, "distances", 0.05, 3.0)?;
            let offsets = p.offsets.clone().unwrap_or_else(|| vec![[0.0, 0.0]]);
            if offsets.is_empty()
                || !offsets
                    .iter()
                    .flatten()
                    .all(|v| v.is_finite() && v.abs() <= 1.0)
            {
                return Err("offsets: at least one, each within 1 unit".into());
            }
            for (i, t) in targets.iter().enumerate() {
                for &turn_by in &turns {
                    for &d in &distances {
                        for &o in &offsets {
                            trials.push(crouch_trial(i, &t.spot, turn_by, d, o));
                        }
                    }
                }
            }
        }
        TrialKind::Drop => {
            let across = list(&p.across, &ACROSS, "across", -1.0, 1.0)?;
            let along = list(&p.along, &ALONG, "along", -3.0, 3.0)?;
            let heights = list(&p.heights, &HEIGHTS, "heights", 0.2, 20.0)?;
            for (i, t) in targets.iter().enumerate() {
                for &h in &heights {
                    for &a in &along {
                        for &c in &across {
                            trials.push(drop_trial(i, &t.spot, c, a, h));
                        }
                    }
                }
            }
        }
        TrialKind::Route => {
            for (i, t) in targets.iter().enumerate() {
                let s = t.spot.stand;
                let to = p.to.unwrap_or([s[0], s[1], s[2] + ROUTE_RISE]);
                for _ in 0..repeat {
                    trials.push(Trial {
                        spot: i,
                        start: to,
                        point: s,
                        yaw: 0.0,
                        way_in: [0.0; 2],
                        way_out: [0.0; 2],
                        distance: None,
                        offset: [0.0; 2],
                        height: None,
                        drift: t.spot.drift,
                    });
                }
            }
        }
    }
    if trials.len() > MAX_TRIALS {
        return Err(format!(
            "{} trials: at most {MAX_TRIALS} a batch; ask for fewer spots or a smaller grid",
            trials.len()
        ));
    }
    if let Some(t) = config.trace
        && t >= trials.len()
    {
        return Err(format!("trace {t}: the batch has {} trials", trials.len()));
    }
    Ok(Plan {
        config,
        targets,
        trials,
    })
}

fn targets(p: &VerifyParams, scanned: &[Spot], kind: TrialKind) -> Result<Vec<Target>, String> {
    let spot_kind = kind.spot_kind();
    if let Some(points) = &p.points {
        if p.spots.is_some() {
            return Err("spots and points: one or the other".into());
        }
        if points.is_empty() {
            return Err("points: at least one".into());
        }
        return points
            .iter()
            .map(|pt| point_spot(pt, spot_kind).map(|spot| Target { index: None, spot }))
            .collect();
    }
    let all = candidates(scanned, spot_kind);
    if all.is_empty() {
        return Err(format!(
            "the scan has no {} spots on this map",
            kind_name(spot_kind)
        ));
    }
    match &p.spots {
        Some(ix) => {
            if ix.is_empty() {
                return Err("spots: at least one index".into());
            }
            ix.iter()
                .map(|&i| {
                    all.get(i)
                        .map(|s| Target {
                            index: Some(i),
                            spot: *s,
                        })
                        .ok_or_else(|| {
                            format!(
                                "spots: no {} spot {i} (the scan has {}; dry_run lists them)",
                                kind_name(spot_kind),
                                all.len()
                            )
                        })
                })
                .collect()
        }
        None => {
            let n = p.max_spots.unwrap_or(MAX_SPOTS).max(1);
            Ok(all
                .iter()
                .enumerate()
                .filter(|(_, s)| s.status != Status::Failed || p.max_spots.is_some())
                .take(n)
                .map(|(i, s)| Target {
                    index: Some(i),
                    spot: *s,
                })
                .collect())
        }
    }
}

/// A crouch trial: the way in turned `turn_by` degrees off straight in
/// (against the way out), the start `distance` outside along it, moved by
/// `offset`.
pub fn crouch_trial(spot: usize, s: &Spot, turn_by: f32, distance: f32, offset: [f32; 2]) -> Trial {
    let out = unit(s.out).unwrap_or([1.0, 0.0]);
    let way_in = turn([-out[0], -out[1]], turn_by);
    let st = s.stand;
    let start = [
        st[0] - way_in[0] * distance + offset[0],
        st[1] - way_in[1] * distance + offset[1],
        st[2] + TELEPORT_LIFT,
    ];
    let point = [
        st[0] + way_in[0] * PUSH_IN,
        st[1] + way_in[1] * PUSH_IN,
        st[2],
    ];
    Trial {
        spot,
        start,
        point,
        yaw: way_in[1].atan2(way_in[0]),
        way_in,
        way_out: out,
        distance: Some(distance),
        offset,
        height: None,
        drift: [0.0; 2],
    }
}

/// A drop trial: the point `across` the spot's target line toward the drift
/// and `along` it from its middle, fallen onto from `height` over the
/// strip's stand height.
pub fn drop_trial(spot: usize, s: &Spot, across: f32, along: f32, height: f32) -> Trial {
    let [a, b] = s.edge;
    let mid = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
    let along_dir = unit([b[0] - a[0], b[1] - a[1]]).unwrap_or([1.0, 0.0]);
    let drift = unit(s.drift).unwrap_or([-along_dir[1], along_dir[0]]);
    let xy = [
        mid[0] + along_dir[0] * along + drift[0] * across,
        mid[1] + along_dir[1] * along + drift[1] * across,
    ];
    let z = s.stand[2];
    Trial {
        spot,
        start: [xy[0], xy[1], z + height],
        point: [xy[0], xy[1], z + TELEPORT_LIFT],
        yaw: drift[1].atan2(drift[0]),
        way_in: [0.0; 2],
        way_out: [0.0; 2],
        distance: None,
        offset: [across, along],
        height: Some(height),
        drift,
    }
}

/// Ticks a crouch trial's walk in may take at most, and its crouch first.
pub const IN_TICKS: u32 = 60;
pub const CROUCH_FIRST_MAX: u32 = 20;
pub const RELEASE_TICKS: u32 = 6;
/// Ticks a teleport is waited for, and the landing read after contact.
pub const TELEPORT_TICKS: u32 = 30;
pub const AFTER_TICKS: u32 = 3;

/// A guess at the batch's length in game ticks: each trial at its most.
pub fn estimate(plan: &Plan) -> u64 {
    let c = &plan.config;
    let each = match c.kind {
        TrialKind::Crouch => {
            2 + c.settle + CROUCH_FIRST_MAX + IN_TICKS + c.hold + RELEASE_TICKS + c.out
        }
        TrialKind::Drop => 4 + c.settle + c.fall + AFTER_TICKS,
        TrialKind::Route => c.route,
    };
    u64::from(each) * plan.trials.len() as u64
}
