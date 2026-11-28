//! The superbounce game type (docs/game/superbounce.md, "The superbounce
//! game type"): a HUD that says whether the local player's level pair is
//! primed, lines over the map for the spots a primed landing launches
//! from, and the `superbounce.state` agent method.
//!
//! **When it is on.** Only when the rules script the active variant's
//! extension block names requires `hud.superbounce` (through
//! [`crate::variant_ext::Decision::superbounce`] and
//! [`crate::rules::header::script_asks_superbounce`]); nothing else turns
//! it on, and `superbounce.set` can only force it off. The built-in "Superbounce" variant's
//! record names the Superbounce script from the list's load
//! (`crate::community_variants::write_blocks`), so a game on it, or on a
//! copy of it, turns the mode on through the script.
//!
//! **Each tick** (a tick observer, only while on) it reads the first local
//! player's unit, its position, whether it is airborne, its vertical
//! velocity and aim, and the word of its one pair against the level
//! (`live_telemetry::havok_agents::fixed_pair`, pair `+0x0c`; -3 is
//! primed), asks for the loaded map's spots (a background scan, once per
//! map and structure BSP, cached), finds the nearest bounce spot and
//! publishes all of it as one [`State`]. The HUD ([`crate::superbounce_hud`],
//! through [`hud_state`]), the world lines ([`gpu::Lines`])
//! and the agent method ([`reply`]) read that state.
//!
//! **The charge, the next thing to do and the landings**
//! (docs/game/superbounce.md, "Charge, crouch spots and landings") are on
//! the same state: [`State::charge`] and [`State::verb`] from the
//! [`superbounce_charge`](crate::superbounce_charge) tracker, which also
//! files [`State::last_landing`] (and [`State::off_spot`], the bounces
//! no marked strip accounts for) and picks [`State::nav_target`];
//! [`State::landing`] from the assist's prediction
//! ([`crate::superbounce_assist::predict`]). The standing-capsule test
//! behind `under` is [`crate::superbounce_capsule`], over the collision the
//! scan reads.
//!
//! **The medals** (docs/game/superbounce.md, "Medals") are on it too:
//! every player's chain of bounces ([`crate::superbounce_chain`]), stepped
//! each tick for every listed player, bots included, published as
//! [`State::medals`]. What they see of each player's physics goes to the
//! running rules script as `on touchdown`, `on launch` and `on grounded`,
//! and each bounce they file as `on bounce`
//! ([`crate::rules::feature::bounces`]).
//!
//! **The Pogo Stick's riders** (docs/game/pogo-stick.md) are stepped from
//! here too, with the same players and awards ([`crate::pogo_ride`]).

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use guest_layout::GuestMemory;
use superbounce::{Kind, Spot, Vec3};

use crate::superbounce_capsule::Room;
use crate::superbounce_chain::{self as chain, Medals};
pub use crate::superbounce_charge::{
    Charge, ChargePhase, Course, Landing, LandingResult, Lost, LostReason, NavKind, NavTarget,
    OffSpot, Predicted, Verb,
};
use crate::superbounce_charge::{Tick, Tracker};
use crate::superbounce_second::{RANKED, Ranked};
use crate::superbounce_targets::Marks;

/// The feature's name.
pub const FEATURE: &str = "superbounce";
/// The pair word of a primed player, as a signed number.
pub const PRIMED_WORD: i32 = -3;
/// Metres in a world unit (10 feet).
pub const METRES_PER_UNIT: f32 = 3.048;
/// Where an object's origin is in its body (`+0x64`): the point a
/// teleport places and the frame every spot's `stand` and `edge` heights
/// are in (the floor plus 0.01). Not the bounding sphere's centre at
/// `+0x30`, which a biped's bounds update keeps about 0.4 units over the
/// origin and which moves with its pose and facing.
pub const OBJECT_POSITION: u32 = guest_layout::Object::POSITION.offset;
/// Where the bounding sphere's centre is in an object's body (`+0x30`):
/// the frame the approach guide's depth rule was measured in
/// (`superbounce_guide::depth`). The mode reads it only for the lift.
pub const OBJECT_CENTRE: u32 = guest_layout::Object::BOUND_CENTER.offset;
/// The most spot lines drawn: the nearest ones.
pub const MAX_LINES: usize = 40;
/// A bounce line's brightness while the player is not primed.
pub const DIM: f32 = 0.35;
/// Bounce strips that carry their launch label: the nearest few.
pub const BOUNCE_LABELS: usize = 3;
/// World units (from the camera) where a line starts to fade, and where
/// it is gone.
pub const FADE_NEAR: f32 = 25.0;
pub const FADE_FAR: f32 = 45.0;

// ---- pure rules ----

/// Whether a level pair word is the primed one (-3).
pub fn primed(word: u32) -> bool {
    word as i32 == PRIMED_WORD
}

// ---- the published state ----

/// How the spot scan stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ScanState {
    /// Nothing asked for: no map yet.
    #[default]
    Idle,
    Scanning,
    Ready,
    Failed(String),
}

/// The bounce spot nearest the player.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Near {
    /// World units, to the spot's edge.
    pub distance: f32,
    pub launch: f32,
    pub fall: f32,
    pub edge: [Vec3; 2],
}

/// Everything the HUD, the lines and the agent method show, as of the last
/// tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub active: bool,
    /// The level pair's word; `None` without a pair (between lives).
    pub word: Option<u32>,
    /// The body's origin (object `+0x64`, [`OBJECT_POSITION`]), the frame
    /// of every spot's heights; not the bounding sphere's centre.
    pub position: Option<Vec3>,
    /// The stick the player holds, in world axes (the unit's throttle at
    /// `+0x1b0` turned by the aim's yaw, `superbounce_flight::held`);
    /// `None` without a unit or a yaw.
    pub stick: Option<[f32; 2]>,
    pub nearest: Option<Near>,
    pub scan: ScanState,
    pub map: Option<String>,
    pub bounce: usize,
    pub crouch: usize,
    /// The biped is off the ground (biped flags bit 0, as `world.object`'s
    /// `physics.airborne` reads it); false without a biped.
    pub airborne: bool,
    /// Vertical velocity, world units a second, up positive.
    pub vz: Option<f32>,
    /// The unit's aim as a yaw, radians from +X toward +Y. The first-person
    /// camera looks along it.
    pub yaw: Option<f32>,
    /// The body's velocity, world units a second.
    pub velocity: Option<Vec3>,
    /// The unit's aim, a unit vector in world space.
    pub aim: Option<Vec3>,
    /// The game tick the state was read on.
    pub tick: Option<u32>,
    /// The seam targets' navpoints and the tag
    /// ([`crate::superbounce_targets`]).
    pub marks: Marks,
    /// The kinds of seam the player has just switched the targets to, for
    /// the readout's line (`superbounce_targets::Switch::switched`).
    pub switched: Option<crate::superbounce_options::TargetMode>,
    /// In the flight after a bounce, the seams it can reach, the best
    /// first ([`crate::superbounce_second`]); none otherwise, and with
    /// the variant's `second_bounce` option off.
    pub second: [Option<Ranked>; RANKED],
    /// The charge meter: the phase of the recipe and its fill; `None` is
    /// hidden (not primed, and not crouched by a crouch spot).
    pub charge: Option<Charge>,
    /// The recipe's step for the HUD's prompt line, while `under` or
    /// `held`.
    pub verb: Option<Verb>,
    /// The navpoint's target: the nearest crouch spot while not primed;
    /// while primed the chosen strip (`superbounce_charge::nav_target`),
    /// and while falling onto a strip that strip
    /// (`superbounce_charge::nav_to_landing`). `None` without a position
    /// or a spot of that kind.
    pub nav_target: Option<NavTarget>,
    /// Where the navpoint is drawn: the takeoff while the player's floor
    /// is too low for the chosen strip ([`navpoint`]), else the target's
    /// spot (`nav_target`'s `at`).
    pub navpoint: Option<Vec3>,
    /// Where to leave from for the chosen strip, while primed and on the
    /// ground ([`takeoff_for`]); `None` when none is found.
    pub takeoff: Option<crate::superbounce_guide::Takeoff>,
    /// The landing the assist predicts, while airborne and falling toward
    /// a strip.
    pub landing: Option<Predicted>,
    /// The bounce rule's verdict, drop from the apex and nearest edge on
    /// this primed airborne tick; independent of the guide's display option.
    pub phase: Option<crate::superbounce_guide::Phase>,
    /// The approach guide on the seam the player is going for
    /// ([`crate::superbounce_guide`]); `None` when it is not shown, and
    /// with the variant's `approach_guide` option off.
    pub guide: Option<crate::superbounce_guide::Guide>,
    /// The last landing within 0.5 of a strip, or primed bounce anywhere,
    /// filed when its launch window closes; kept until the next one or the
    /// game's end.
    pub last_landing: Option<Landing>,
    /// The first local player's last [`OFF_SPOT_MAX`] bounces no marked
    /// strip accounts for, oldest first, this game; each one's `edge` is
    /// the nearest scanned bounce spot's within 0.5, marked or not.
    pub off_spot: Vec<OffSpot>,
    /// Ticks the primed body has been still (speed under 0.03 units/s);
    /// 0 while not primed. The primed fill is 1 less this over 38.
    pub still_ticks: u32,
    /// Whether the standing capsule fits at the player's origin, by the
    /// capsule test against the scanned collision
    /// ([`crate::superbounce_capsule`]); `None` before the scan is ready.
    pub standing_fits: Option<bool>,
    /// Every player's medals this game: the records and the last awards
    /// ([`crate::superbounce_chain`]).
    pub medals: chain::View,
}

impl State {
    const EMPTY: State = State {
        active: false,
        word: None,
        position: None,
        stick: None,
        nearest: None,
        scan: ScanState::Idle,
        map: None,
        bounce: 0,
        crouch: 0,
        airborne: false,
        vz: None,
        yaw: None,
        velocity: None,
        aim: None,
        tick: None,
        marks: Marks::NONE,
        switched: None,
        second: [None; RANKED],
        charge: None,
        verb: None,
        nav_target: None,
        navpoint: None,
        takeoff: None,
        landing: None,
        phase: None,
        guide: None,
        last_landing: None,
        off_spot: Vec::new(),
        still_ticks: 0,
        standing_fits: None,
        medals: chain::View::EMPTY,
    };

    pub fn primed(&self) -> bool {
        self.word.is_some_and(primed)
    }

    /// Where the nearest spot is from where the player looks: radians
    /// clockwise from straight ahead, in (-pi, pi] (a spot to the right is
    /// positive, as an arrow on screen turns). None without a position, an
    /// aim or a spot, or standing on it.
    pub fn bearing(&self) -> Option<f32> {
        bearing(self.position?, self.yaw?, self.nearest?.edge)
    }
}

/// [`State::bearing`] for a player at `at` aiming along `yaw`, to the
/// nearest point of `edge`, in the horizontal plane.
pub fn bearing(at: Vec3, yaw: f32, edge: [Vec3; 2]) -> Option<f32> {
    use std::f32::consts::{PI, TAU};
    let [a, b] = edge;
    let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
    let len2 = ex * ex + ey * ey;
    let t = if len2 > 0.0 {
        (((at[0] - a[0]) * ex + (at[1] - a[1]) * ey) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (dx, dy) = (a[0] + t * ex - at[0], a[1] + t * ey - at[1]);
    if dx.hypot(dy) < 0.05 || !yaw.is_finite() {
        return None;
    }
    // World yaw turns from +X toward +Y, which is to the left (Z up).
    let left = dy.atan2(dx) - yaw;
    let mut cw = (-left).rem_euclid(TAU);
    if cw > PI {
        cw -= TAU;
    }
    Some(cw)
}

static STATE: Mutex<State> = Mutex::new(State::EMPTY);
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The state as of the last tick.
pub fn state() -> State {
    lock(&STATE).clone()
}

/// Whether the mode was on at the last tick (one atomic load).
pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// The state for the HUD: `None` while the mode is off (one atomic load
/// then). The window asks each present.
pub fn hud_state() -> Option<State> {
    if !active() {
        return None;
    }
    let s = state();
    s.active.then_some(s)
}

/// `superbounce.state`'s result for `s`.
pub fn reply(s: &State) -> agent_proto::superbounce::State {
    use agent_proto::superbounce as wire;
    let (status, error) = match &s.scan {
        ScanState::Idle => ("idle", None),
        ScanState::Scanning => ("scanning", None),
        ScanState::Ready => ("ready", None),
        ScanState::Failed(why) => ("failed", Some(why.clone())),
    };
    let v3 = |v: Vec3| v.map(f64::from);
    wire::State {
        active: s.active,
        primed: s.primed(),
        word: s.word.map(|w| format!("{w:08x}")),
        word_signed: s.word.map(|w| w as i32),
        position: s.position.map(v3),
        nearest: s.nearest.map(|n| wire::Nearest {
            distance: f64::from(n.distance),
            launch: f64::from(n.launch),
            fall: f64::from(n.fall),
            edge: n.edge.map(v3),
        }),
        scan: wire::Scan {
            status: status.into(),
            map: s.map.clone(),
            error,
            bounce: s.bounce,
            crouch: s.crouch,
        },
        airborne: s.position.map(|_| s.airborne),
        vz: s.vz.map(f64::from),
        yaw: s.yaw.map(f64::from),
        velocity: s.velocity.map(v3),
        options: options_reply(crate::superbounce_options::current()),
        targets: wire::Targets {
            mode: crate::superbounce_targets::mode()
                .unwrap_or(crate::superbounce_options::current().targets)
                .name()
                .into(),
            count: crate::superbounce_targets::current().len(),
            tag: s.marks.tagged.map(v3),
            marked: s.marks.targets.iter().flatten().map(|m| v3(m.at)).collect(),
            second: s
                .second
                .iter()
                .flatten()
                .map(|r| wire::SecondBounce {
                    at: v3(r.at),
                    time: f64::from(r.time),
                    steer: f64::from(r.steer[0].hypot(r.steer[1])),
                    reach: f64::from(r.reach),
                    speed: f64::from(r.speed),
                    need: f64::from(r.need),
                    cost: f64::from(r.cost),
                    caught: r.caught,
                })
                .collect(),
        },
        charge: s.charge.map(|c| wire::Charge {
            phase: match c.phase {
                ChargePhase::Near => wire::ChargePhase::Near,
                ChargePhase::Under => wire::ChargePhase::Under,
                ChargePhase::Held => wire::ChargePhase::Held,
                ChargePhase::Primed => wire::ChargePhase::Primed,
                ChargePhase::Lost => wire::ChargePhase::Lost,
            },
            fill: f64::from(c.fill),
            still_ticks: s.still_ticks,
        }),
        verb: s.verb.map(|v| match v {
            Verb::LetGoOfCrouch => wire::Verb::LetGoOfCrouch,
            Verb::WalkOutToStand => wire::Verb::WalkOutToStand,
            Verb::KeepMoving => wire::Verb::KeepMoving,
            Verb::ChargeAgain(_) => wire::Verb::ChargeAgain,
        }),
        lost_reason: lost(s).map(|l| match l.reason {
            LostReason::StoodStill => wire::LostReason::StoodStill,
            LostReason::Crouched => wire::LostReason::Crouched,
            LostReason::Unknown => wire::LostReason::Unknown,
        }),
        nav_target: s.nav_target.map(|n| wire::NavTarget {
            at: v3(n.at),
            kind: match n.kind {
                NavKind::Crouch => wire::NavKind::Crouch,
                NavKind::Strip => wire::NavKind::Strip,
            },
            distance: f64::from(n.distance),
            fall: f64::from(n.fall),
        }),
        takeoff: s.takeoff.map(|t| v3(t.at)),
        landing: s.landing.map(|p| wire::Predicted {
            at: v3(p.at),
            verdict: match p.verdict {
                Course::OnSlot => wire::Course::OnSlot,
                Course::Fixable => wire::Course::Fixable,
                Course::Miss => wire::Course::Miss,
            },
            speed: f64::from(p.speed),
            need: f64::from(p.need),
            miss: f64::from(p.miss()),
            locked: p.locked(),
        }),
        guide: s.guide.as_ref().map(guide_reply),
        last_landing: s.last_landing.map(|l| wire::Landing {
            result: match l.result {
                LandingResult::Bounce => wire::LandingResult::Bounce,
                LandingResult::NotPrimed => wire::LandingResult::NotPrimed,
                LandingResult::Missed => wire::LandingResult::Missed,
                LandingResult::TooSlow => wire::LandingResult::TooSlow,
                LandingResult::NoBounce => wire::LandingResult::NoBounce,
                LandingResult::Through => wire::LandingResult::Through,
            },
            tick: l.tick,
            at: v3(l.at),
        }),
        off_spot: s
            .off_spot
            .iter()
            .map(|o| wire::OffSpot {
                tick: o.tick,
                at: v3(o.at),
                edge: o.edge.map(|e| e.map(v3)),
                speed_in: f64::from(o.speed_in),
                launch: f64::from(o.launch),
            })
            .collect(),
        standing_fits: s.standing_fits,
        medals: medals_reply(&s.medals),
    }
}

/// `superbounce.state`'s `options` for `o`.
pub fn options_reply(o: crate::superbounce_options::Options) -> agent_proto::superbounce::Options {
    agent_proto::superbounce::Options {
        targets: o.targets.name().into(),
        velocity: o.velocity,
        alignment: o.alignment,
        miss_info: o.miss_info,
        medals: o.medals,
        second_bounce: o.second_bounce,
        approach_guide: o.approach_guide,
        charge_segments: o.charge_segments,
    }
}

/// `superbounce.state`'s `guide` for `g`.
pub fn guide_reply(g: &crate::superbounce_guide::Guide) -> agent_proto::superbounce::Guide {
    use hud_kit_core::views::superbounce_guide::Call;
    let v3 = |v: Vec3| v.map(f64::from);
    agent_proto::superbounce::Guide {
        seam: v3(g.seam.point),
        call: match g.call {
            Call::OffLine => "off_line",
            Call::LinedUp => "lined_up",
            Call::TooShallow => "too_shallow",
            Call::TooHigh => "too_high",
            Call::OffEdge => "off_edge",
            Call::NoDrift => "no_drift",
            Call::Crouching => "crouching",
            Call::WillBounce => "will_bounce",
        }
        .into(),
        jump: g.jump,
        impact: g.pipper().map(v3),
        row: g.impact.map(|i| v3(i.row)),
        ticks: g.impact.map(|i| i.ticks),
        speed: g.impact.map(|i| f64::from(i.speed)),
        caught: g.caught(),
        deep: g.depth.map(|d| d.deep()),
        depth: g.depth.map(|d| f64::from(d.margin)),
        past: g.depth.map(|d| f64::from(d.past)),
        across: g.across.map(f64::from),
        chance: g.chance.map(|c| f64::from(c.value)),
        chance_capped: g.chance.is_some_and(|c| c.capped),
        jump_in: g.jump_in,
        cue: g.cue.map(crate::superbounce_guide::Cue::text),
        takeoff: g.takeoff.map(|t| agent_proto::superbounce::TakeoffLine {
            at: v3(t.at),
            heading: t.heading.map(f64::from),
            half: f64::from(t.half),
            speed: f64::from(t.speed),
            // Not counting to a jump's line: `Climb up`, `Walk to line`
            // or `Walk off`.
            walk: !g.cue.is_some_and(crate::superbounce_guide::Cue::counting),
        }),
        strength: f64::from(g.strength),
    }
}

/// The seam marks a player is given (docs/game/superbounce.md,
/// "Targets"): the target seams while primed only, as a seam is nothing
/// to go for before then; the tag either way.
pub fn primed_marks(marks: Marks, primed: bool) -> Marks {
    if primed {
        marks
    } else {
        Marks {
            targets: Marks::NONE.targets,
            ..marks
        }
    }
}

/// The takeoff for the strip `spot` for a player whose feet are at
/// `player` (docs/game/superbounce.md, "The takeoff"), with `floor(xy,
/// top)` the map's floor (`superbounce_capsule::Room::floor_at`): placed
/// (`superbounce_guide::place`, flown at `superbounce_guide::CENTRE_LIFT`)
/// from `run`, a ghost or verified run's lip
/// (`superbounce_ghost::lip_for`), when it has one on a real floor; else
/// from the map's lip search (`superbounce_world::lip`).
pub fn takeoff_for(
    spot: &Spot,
    player: Vec3,
    run: Option<crate::superbounce_guide::Lip>,
    floor: impl Fn([f32; 2], f32) -> Option<f32>,
) -> Option<crate::superbounce_guide::Takeoff> {
    use crate::superbounce_guide::{CENTRE_LIFT, Seam, place};
    let seam = Seam::of(spot, [spot.stand[0], spot.stand[1]]);
    let need = crate::superbounce_charge::need_of(spot);
    // A fixed lift: the takeoff is a place on the floor, and must not
    // move with the pose.
    run.and_then(|l| place(&seam, need, &l, CENTRE_LIFT, &floor))
        .or_else(|| {
            crate::superbounce_world::lip(spot.stand, spot.fall, spot.drift, player, &floor)
                .and_then(|l| place(&seam, need, &l, CENTRE_LIFT, &floor))
        })
}

/// Where the navpoint goes for `nav`: on a strip the player's floor is
/// too low for (`high_enough` false), its `takeoff`, the place to climb
/// to; else the target's own spot.
pub fn navpoint(
    nav: Option<NavTarget>,
    takeoff: Option<&crate::superbounce_guide::Takeoff>,
    high_enough: bool,
) -> Option<Vec3> {
    let n = nav?;
    match takeoff {
        Some(t) if n.kind == NavKind::Strip && !high_enough => Some(t.at),
        _ => Some(n.at),
    }
}

/// `superbounce.state`'s `medals` for `m`.
pub fn medals_reply(m: &chain::View) -> agent_proto::superbounce::Medals {
    use agent_proto::superbounce as wire;
    let ids = |medals: &[hud_kit_core::views::superbounce::Medal]| {
        medals.iter().map(|m| m.id().to_owned()).collect()
    };
    wire::Medals {
        players: m
            .players
            .iter()
            .map(|(slot, r)| wire::MedalPlayer {
                player: *slot,
                chain: r.chain,
                live: r.live,
                counts: r
                    .counts
                    .iter()
                    .map(|(m, n)| (m.id().to_owned(), *n))
                    .collect(),
            })
            .collect(),
        recent: m
            .recent
            .iter()
            .map(|a| wire::MedalAward {
                player: a.slot,
                tick: a.tick,
                chain: a.chain,
                medals: ids(&a.medals),
                strips: a.strips,
            })
            .collect(),
    }
}

/// The last loss, while the charge shows `lost` or the verb asks for a
/// new charge.
pub fn lost(s: &State) -> Option<Lost> {
    s.charge.and_then(|c| c.lost).or(match s.verb {
        Some(Verb::ChargeAgain(l)) => Some(l),
        _ => None,
    })
}

/// `superbounce.state` takes no parameters: null or `{}`.
pub fn no_params(params: &serde_json::Value) -> Result<(), &'static str> {
    if params.is_null() || params.as_object().is_some_and(|f| f.is_empty()) {
        Ok(())
    } else {
        Err("superbounce.state takes no parameters")
    }
}

/// `superbounce.state` as the agent answers it.
pub fn state_reply() -> agent_proto::superbounce::State {
    reply(&state())
}

// ---- the scan ----

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ScanKey {
    map: String,
    /// The loaded map file's header checksum, so a different file of the
    /// same name (an editor or Map Pack override) is scanned again.
    checksum: u32,
    bsp: usize,
}

/// One scan's result: the spots, and the collision they were found in
/// (for the standing-capsule test).
struct Scanned {
    spots: Vec<Spot>,
    room: Room,
}

#[derive(Default)]
struct Scans {
    current: Option<ScanKey>,
    /// The current key's spots with the map's verified file applied
    /// (`superbounce::verified`), failed ones too: what the verifier
    /// indexes.
    scanned: Arc<Vec<Spot>>,
    /// The ones of them that are marked ([`marked`]): what is drawn and
    /// steered onto.
    spots: Arc<Vec<Spot>>,
    /// The bots' spots generation the verified file was applied at.
    verified_at: u64,
    /// The current key's collision; empty until its scan is ready.
    room: Arc<Room>,
    status: ScanState,
    cache: HashMap<ScanKey, (Arc<Vec<Spot>>, Arc<Room>)>,
    /// Scans running now, so leaving a map and coming back does not start a
    /// second one.
    inflight: HashSet<ScanKey>,
}

fn scans() -> &'static Mutex<Scans> {
    static S: OnceLock<Mutex<Scans>> = OnceLock::new();
    S.get_or_init(Mutex::default)
}

/// The spots of the structure BSP `bsp` of an open map, and its collision
/// for the capsule test (`superbounce::scan_map` is `scan` over
/// `map_polys`). Isolated so that the scan taking a raw resource source is
/// a one-line change here.
#[cfg_attr(not(feature = "gpu"), allow(dead_code))]
fn scan_one<R: Read + Seek>(
    map: &mut map::Map<R>,
    raw: &mut dyn tags::geometry::RawSource,
    bsp: usize,
) -> Result<Scanned, String> {
    let _ = &raw;
    // The starting locations and the measured priming places, as `h2
    // superbounce scan` and the bots' scan use them.
    let ctx = superbounce::map_context(map).map_err(|e| e.to_string())?;
    let polys = superbounce::map_polys(map, bsp).map_err(|e| e.to_string())?;
    Ok(Scanned {
        spots: superbounce::scan_in(&polys, &ctx),
        room: Room::new(&polys),
    })
}

#[cfg(feature = "gpu")]
fn scan(key: &ScanKey) -> Result<Scanned, String> {
    let (mut map, files) = crate::native_map::open(&key.map).map_err(|e| e.to_string())?;
    if map.header().checksum != key.checksum {
        return Err("the mounted map file is not the loaded one".into());
    }
    let mut raw = crate::native_world_glue::Raw(files);
    scan_one(&mut map, &mut raw, key.bsp)
}

#[cfg(not(feature = "gpu"))]
fn scan(_: &ScanKey) -> Result<Scanned, String> {
    Err("this build has no map access".into())
}

/// What a tick sees of the scan: the key's spots, starting the scan the
/// first time the key is seen.
struct ScanView {
    spots: Arc<Vec<Spot>>,
    room: Arc<Room>,
    status: ScanState,
}

/// `raw` with the map's verified file and then its learned file applied,
/// and the speed model filled in: every spot, and the ones [`marked`]. A
/// `learned` or `partial` line of the sim's training moves its spot's aim
/// (`stand`) onto the line's (`superbounce::learned::apply`): the sim's
/// measured answer outranks the scan's aim. A proven seam's `stand` is
/// then the landing the game run measured, the verified entry's working
/// point (`superbounce::verified::landings`, `land`): a game run's
/// landing outranks both. Everything that points at a seam reads
/// `stand`: the navpoints, the tag, the line-up, the second bounce and
/// the assist. The edge stays the collision
/// edge, and a line proves nothing. Each strip with no
/// measured `need` or `fall` takes the one threshold every seam shares and
/// the drop that reaches it (`superbounce::speed::apply`).
fn with_verified(map: &str, raw: &[Spot]) -> (Arc<Vec<Spot>>, Arc<Vec<Spot>>) {
    let mut all = raw.to_vec();
    let verified = bots::spots::verified_for(map);
    superbounce::verified::apply(&mut all, &verified);
    // Worked out on the scan's stands, as the entries are matched.
    let landings = superbounce::verified::landings(&all, &verified);
    superbounce::learned::apply(&mut all, &bots::spots::learned_for(map));
    superbounce::verified::land(&mut all, &landings);
    superbounce::speed::apply(&mut all);
    let shown = marked(&all);
    (Arc::new(all), Arc::new(shown))
}

/// Whether a game run has shown `spot` works: its `verified` flag, from
/// the map's verified file or a measured place.
pub fn verified(spot: &Spot) -> bool {
    spot.status == superbounce::Status::Proven
}

/// The spots that are marked, pointed at and steered onto, of `all` (the
/// scan with the verified file applied): never a failed one.
///
/// - Crouch spots: every one not failed.
/// - Strips, once a game run has proved any strip of the map
///   ([`verified`]): only the proved ones.
/// - Strips before that: the scan's two-joint slots. A single crease, V
///   or busy point is a candidate (`Spot::candidate`) for the sim's
///   training only, so it is left out until a game run proves it.
///
/// A proven V's arm is marked only for a player on the side it is
/// approached from: [`for_player`] narrows these each tick.
pub fn marked(all: &[Spot]) -> Vec<Spot> {
    let proved = all.iter().any(|s| s.kind == Kind::Bounce && verified(s));
    all.iter()
        .filter(|s| s.status != superbounce::Status::Failed)
        .filter(|s| match s.kind {
            Kind::Crouch => true,
            Kind::Bounce if proved => verified(s),
            Kind::Bounce => !s.candidate(),
        })
        .copied()
        .collect()
}

/// The marked spots as the player standing at `at` is given them: a
/// proven V's arm only from the far side of its bisector, so the arm
/// marked is the one across from the player, the mirror of their approach
/// (`superbounce::facing`; docs/game/superbounce.md, "Vs"). Nothing else
/// changes; with no V arm among them, `spots` itself.
pub fn for_player(spots: &Arc<Vec<Spot>>, at: Option<Vec3>) -> Arc<Vec<Spot>> {
    if spots.iter().any(|s| s.arm.is_some()) {
        Arc::new(superbounce::facing(spots, at))
    } else {
        Arc::clone(spots)
    }
}

/// The loaded map's name, once the mode has asked for its scan.
pub(crate) fn current_map() -> Option<String> {
    lock(scans()).current.as_ref().map(|k| k.map.clone())
}

/// Applies the bots' files to the mode's scan of `map` now, when `map`
/// is the current one and its scan is ready (`superbounce.reload`;
/// otherwise the tick does it on the generation change). Whether it did.
pub(crate) fn refresh(map: &str) -> bool {
    let mut s = lock(scans());
    let Some(key) = s
        .current
        .clone()
        .filter(|k| k.map.eq_ignore_ascii_case(map))
    else {
        return false;
    };
    let Some((raw, _)) = s.cache.get(&key).cloned() else {
        return false;
    };
    let (all, shown) = with_verified(&key.map, &raw);
    s.scanned = all;
    s.spots = shown;
    s.verified_at = bots::spots::generation();
    true
}

/// `map`'s scan as the bots keep it with its verified and learned files
/// applied, and the ones the mode marks.
pub(crate) fn applied(map: &str) -> (Arc<Vec<Spot>>, Arc<Vec<Spot>>) {
    with_verified(map, &bots::spots::scanned(map))
}

fn request_scan(key: ScanKey) -> ScanView {
    let mut s = lock(scans());
    // A verified file written (or read) since: apply it again.
    let generation = bots::spots::generation();
    if s.current.as_ref() == Some(&key)
        && s.verified_at != generation
        && let Some((raw, _)) = s.cache.get(&key).cloned()
    {
        let (all, shown) = with_verified(&key.map, &raw);
        s.scanned = all;
        s.spots = shown;
        s.verified_at = generation;
    }
    if s.current.as_ref() != Some(&key) {
        s.current = Some(key.clone());
        if let Some((spots, room)) = s.cache.get(&key).cloned() {
            let (all, shown) = with_verified(&key.map, &spots);
            s.scanned = all;
            s.spots = shown;
            s.verified_at = generation;
            s.room = room;
            s.status = ScanState::Ready;
        } else if s.inflight.contains(&key) {
            s.spots = Arc::default();
            s.scanned = Arc::default();
            s.room = Arc::default();
            s.status = ScanState::Scanning;
        } else {
            s.spots = Arc::default();
            s.scanned = Arc::default();
            s.room = Arc::default();
            s.status = ScanState::Scanning;
            s.inflight.insert(key.clone());
            let k = key.clone();
            let spawned = std::thread::Builder::new()
                .name("superbounce-scan".into())
                .spawn(move || {
                    let found = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan(&k)))
                        .unwrap_or_else(|_| Err("the scan panicked".into()));
                    finish_scan(k, found);
                });
            if let Err(e) = spawned {
                s.inflight.remove(&key);
                s.status = ScanState::Failed(format!("no scan thread: {e}"));
            }
        }
    }
    ScanView {
        spots: Arc::clone(&s.spots),
        room: Arc::clone(&s.room),
        status: s.status.clone(),
    }
}

fn finish_scan(key: ScanKey, found: Result<Scanned, String>) {
    let mut s = lock(scans());
    s.inflight.remove(&key);
    let here = s.current.as_ref() == Some(&key);
    match found {
        Ok(Scanned { spots, room }) => {
            eprintln!(
                "superbounce: {} spots on {} (BSP {})",
                spots.len(),
                key.map,
                key.bsp
            );
            // The bots' spots come from the nav worker's scan or its saved
            // file; a run with neither (sim mode starts no worker, and a
            // file from another scanner version is not read) would leave
            // the practising bots with no spot to go to. This scan is the
            // same scanner's, so it serves them until the worker's arrives.
            if !bots::spots::has_scan(&key.map) {
                eprintln!(
                    "superbounce: {} spots on {} given to the bots (no nav scan)",
                    spots.len(),
                    key.map
                );
                bots::spots::publish(&key.map, spots.clone());
            }
            let (spots, room) = (Arc::new(spots), Arc::new(room));
            s.cache
                .insert(key.clone(), (Arc::clone(&spots), Arc::clone(&room)));
            if here {
                let (all, shown) = with_verified(&key.map, &spots);
                s.scanned = all;
                s.spots = shown;
                s.verified_at = bots::spots::generation();
                s.room = room;
                s.status = ScanState::Ready;
            }
        }
        Err(why) => {
            eprintln!(
                "superbounce: no spots on {} (BSP {}): {why}",
                key.map, key.bsp
            );
            if here {
                s.status = ScanState::Failed(why);
            }
        }
    }
}

/// The spots of the current scan that are marked ([`marked`]): the map's
/// verified file applied, the failed spots and the unproved creases left
/// out, and a V's arm only for a player across from it ([`for_player`],
/// at the last tick's position).
pub fn current_spots() -> Arc<Vec<Spot>> {
    let at = lock(&STATE).position;
    let spots = Arc::clone(&lock(scans()).spots);
    for_player(&spots, at)
}

/// Every spot of the current scan with its verified status, failed ones
/// too, and the map they are for: what `superbounce.verify` indexes.
pub fn scanned_spots() -> (Option<String>, Arc<Vec<Spot>>) {
    let s = lock(scans());
    (
        s.current.as_ref().map(|k| k.map.clone()),
        Arc::clone(&s.scanned),
    )
}

/// The collision of the current scan, for the standing-capsule test;
/// empty until the scan is ready.
pub fn current_room() -> Arc<Room> {
    Arc::clone(&lock(scans()).room)
}

/// Whether the standing capsule fits at `origin` on the loaded map, while
/// the mode is on and its scan is ready (`world.object`'s
/// `physics.standing_fits`); `None` otherwise.
pub fn standing_fits_at(origin: Vec3) -> Option<bool> {
    if !active() {
        return None;
    }
    let room = current_room();
    (!room.is_empty()).then(|| room.standing_fits(origin))
}

/// `world.object`'s `physics.standing_fits`, from [`standing_fits_at`].
pub struct StandingFitsNow;

impl agent::StandingFits for StandingFitsNow {
    fn standing_fits(&mut self, origin: [f64; 3]) -> Option<bool> {
        standing_fits_at(origin.map(|v| v as f32))
    }
}

// ---- reading the game ----

/// The agent's override (`superbounce.set`): [`FOLLOW`] or [`FORCE_OFF`].
/// There is no force-on: only a script that requires `hud.superbounce`
/// turns the mode on.
static OVERRIDE: AtomicU8 = AtomicU8::new(FOLLOW);
const FOLLOW: u8 = 0;
const FORCE_OFF: u8 = 2;

/// `superbounce.set`: `Some(false)` forces the mode off, `None` follows
/// the game again; `Some(true)` is refused. Returns the override's name.
/// The tick picks it up at its next check (within [`RECHECK`] ticks).
pub fn set_override(on: Option<bool>) -> Result<&'static str, &'static str> {
    let (v, name) = match on {
        Some(true) => {
            return Err(
                "the mode turns on only from a game whose script requires hud.superbounce; \
                 superbounce.set can only force it off",
            );
        }
        Some(false) => (FORCE_OFF, "off"),
        None => (FOLLOW, "game"),
    };
    OVERRIDE.store(v, Ordering::Relaxed);
    Ok(name)
}

/// Whether the mode is on in the game running now.
fn on_now(m: &dyn GuestMemory) -> bool {
    if OVERRIDE.load(Ordering::Relaxed) == FORCE_OFF {
        return false;
    }
    crate::variant_ext::in_game(&crate::layout_glue::ReadGm(m)).superbounce()
}

/// The local player's word, position and motion.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Live {
    word: Option<u32>,
    /// The origin ([`OBJECT_POSITION`]).
    position: Option<Vec3>,
    /// The bounding sphere's centre over the origin
    /// ([`superbounce_guide::CENTRE_LIFT`](crate::superbounce_guide::CENTRE_LIFT)
    /// when either is not read).
    lift: f32,
    /// The held stick in world axes ([`State::stick`]).
    stick: Option<[f32; 2]>,
    airborne: bool,
    vz: Option<f32>,
    yaw: Option<f32>,
    /// The aim, a unit vector.
    aim: Option<Vec3>,
    /// The flashlight action is down.
    flashlight: bool,
    /// World units a second.
    velocity: Option<Vec3>,
    /// The crouch request, unit flags bit 23.
    request: bool,
    /// The game tick.
    tick: Option<u32>,
}

/// The seam the local player faces, tick to tick.
static FACING: Mutex<crate::superbounce_seams::Facing> =
    Mutex::new(crate::superbounce_seams::Facing::new());

/// The chosen strip with the faced seam: while `nav` is on a strip (the
/// player is primed), the seam of `shown` whose landing point is `faced`
/// is the chosen strip. Otherwise, and with no position, `nav` as it is.
pub fn faced_target(
    nav: Option<NavTarget>,
    faced: Option<Vec3>,
    shown: &[crate::superbounce_targets::Target],
    at: Option<Vec3>,
) -> Option<NavTarget> {
    let strip = nav.is_none_or(|n| n.kind == NavKind::Strip);
    let seam = faced
        .filter(|_| strip)
        .and_then(|f| shown.iter().find(|t| t.at() == f));
    match (seam, at) {
        (Some(t), Some(at)) => {
            let d = [t.at()[0] - at[0], t.at()[1] - at[1], t.at()[2] - at[2]];
            Some(NavTarget {
                at: t.at(),
                kind: NavKind::Strip,
                distance: (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt(),
                fall: t.spot.fall,
            })
        }
        _ => nav,
    }
}

/// The game tick, from the game-time record.
fn game_tick(m: &dyn GuestMemory) -> Option<u32> {
    use live_telemetry::layout::{GAME_TIME_PTR, TIME_TICK};
    let time = m.u32(GAME_TIME_PTR).ok().filter(|&t| t != 0)?;
    m.u32(time.wrapping_add(TIME_TICK)).ok()
}

fn read_live(m: &dyn GuestMemory) -> Live {
    let tick = game_tick(m);
    let Ok(units) = crate::skulls_custom::local_units(m) else {
        return Live {
            tick,
            ..Live::default()
        };
    };
    let Some(u) = units.first() else {
        return Live {
            tick,
            ..Live::default()
        };
    };
    let read3 = |offset: u32| {
        let at = u.body.wrapping_add(offset);
        let f = |i: u32| m.u32(at.wrapping_add(4 * i)).ok().map(f32::from_bits);
        Some([f(0)?, f(1)?, f(2)?])
    };
    let position = read3(OBJECT_POSITION);
    let lift = match (position, read3(OBJECT_CENTRE)) {
        (Some(o), Some(c)) if (c[2] - o[2]).is_finite() => c[2] - o[2],
        _ => crate::superbounce_guide::CENTRE_LIFT,
    };
    let word = live_telemetry::havok_agents::fixed_pair_word(m, u.unit)
        .ok()
        .flatten();
    use live_telemetry::layout::{
        BIPED_FLAGS, BODY_VELOCITY, UNIT_AIM, UNIT_FLAG_CROUCH, UNIT_FLAGS,
    };
    let f = |at: u32| {
        m.u32(u.body.wrapping_add(at))
            .ok()
            .map(f32::from_bits)
            .filter(|v| v.is_finite())
    };
    let vz = f(BODY_VELOCITY + 8);
    let velocity = (|| Some([f(BODY_VELOCITY)?, f(BODY_VELOCITY + 4)?, vz?]))();
    let request = m
        .u32(u.body.wrapping_add(UNIT_FLAGS))
        .is_ok_and(|flags| flags & UNIT_FLAG_CROUCH != 0);
    let yaw = match (f(UNIT_AIM), f(UNIT_AIM + 4)) {
        (Some(x), Some(y)) if x != 0.0 || y != 0.0 => Some(y.atan2(x)),
        _ => None,
    };
    let aim = (|| Some([f(UNIT_AIM)?, f(UNIT_AIM + 4)?, f(UNIT_AIM + 8)?]))()
        .filter(|a: &Vec3| (a[0] * a[0] + a[1] * a[1] + a[2] * a[2] - 1.0).abs() < 0.1);
    // The unit's throttle (forward, left; `cheats::flight::UNIT_THROTTLE`)
    // in world axes.
    let throttle = guest_layout::Unit::THROTTLE.offset;
    let stick = match (f(throttle), f(throttle + 4), yaw) {
        (Some(forward), Some(left), Some(yaw)) => {
            Some(crate::superbounce_flight::held([forward, left], yaw))
        }
        _ => None,
    };
    let flashlight = m
        .u32(
            u.body
                .wrapping_add(crate::superbounce_view::UNIT_CONTROL_FLAGS),
        )
        .is_ok_and(|flags| flags & crate::superbounce_view::CONTROL_FLASHLIGHT != 0);
    // Bit 0 of the biped flags, as `world.object` reads `physics.airborne`.
    let airborne = u.kind == guest_layout::view::objects::BIPED
        && m.u16(u.body.wrapping_add(BIPED_FLAGS))
            .is_ok_and(|flags| flags & 1 != 0);
    Live {
        word,
        position,
        lift,
        stick,
        airborne,
        vz,
        yaw,
        aim,
        flashlight,
        velocity,
        request,
        tick,
    }
}

/// The loaded map's cache header checksum (`+0x2d0` in the header the name
/// is at `+0x198` of).
const CACHE_CHECKSUM: u32 = live_telemetry::layout::CACHE_MAP_NAME - 0x198 + 0x2d0;

/// The scan key of the loaded map and its active structure BSP.
fn scan_key(m: &dyn GuestMemory) -> Option<ScanKey> {
    use crate::skulls_custom::{GAME_GLOBALS, GLOBALS_ACTIVE_BSP};
    let map = crate::health::loaded_map_name(m)
        .ok()
        .filter(|n| !n.is_empty())?;
    let g = m.u32(GAME_GLOBALS).ok().filter(|&g| g != 0)?;
    let bsp = m.u16(g.wrapping_add(GLOBALS_ACTIVE_BSP)).ok()? as i16;
    Some(ScanKey {
        map,
        checksum: m.u32(CACHE_CHECKSUM).ok()?,
        bsp: usize::try_from(bsp).ok()?,
    })
}

/// How often (in ticks) a mode that is off, or on, is decided again, so a
/// guest's late host block is picked up. A second at 30 Hz.
const RECHECK: u32 = 30;
static TICKS: AtomicU32 = AtomicU32::new(0);
/// The charge and landing state kept from tick to tick.
static TRACKER: Mutex<Tracker> = Mutex::new(Tracker::new());
/// Every player's chain and medals this game.
static MEDALS: Mutex<Medals> = Mutex::new(Medals::empty());
/// The first local player's off-spot bounces this game, oldest first.
static OFF_SPOT: Mutex<Vec<OffSpot>> = Mutex::new(Vec::new());
/// Off-spot bounces kept for `superbounce.state`; the oldest go first.
pub const OFF_SPOT_MAX: usize = 16;

/// Keeps `o` on the list, its `edge` looked up in the whole scan (`all`,
/// marked or not) when no marked strip was near.
fn keep_off_spot(mut o: OffSpot, all: &[Spot]) -> Vec<OffSpot> {
    if o.edge.is_none() {
        o.edge = crate::superbounce_charge::landing_spot(all, o.at).map(|i| all[i].edge);
    }
    let mut kept = lock(&OFF_SPOT);
    kept.push(o);
    let over = kept.len().saturating_sub(OFF_SPOT_MAX);
    kept.drain(..over);
    kept.clone()
}
/// The first local player's slot and user at the last tick, for
/// `superbounce.medal_test`.
static LOCAL: Mutex<Option<(u8, u8)>> = Mutex::new(None);

/// Whether the awards are kept for sim mode's bounce practice
/// (docs/runtime/sim.md, "Bounce practice"). Off but in sim mode.
static COLLECT: AtomicBool = AtomicBool::new(false);
/// The awards kept since the sim last took them, oldest first.
static COLLECTED: Mutex<Vec<chain::Award>> = Mutex::new(Vec::new());
/// Awards kept for the sim at most between takes; the oldest go first.
pub const COLLECT_MAX: usize = 4096;

/// Keeps every award (a bounce of a chain, or a Bounced) for
/// [`take_awards`], or stops and drops what was kept.
pub fn collect_awards(on: bool) {
    COLLECT.store(on, Ordering::Relaxed);
    if !on {
        lock(&COLLECTED).clear();
    }
}

/// The awards made since the last take, oldest first.
pub fn take_awards() -> Vec<chain::Award> {
    std::mem::take(&mut *lock(&COLLECTED))
}

fn keep_awards(awards: &[chain::Award]) {
    if awards.is_empty() || !COLLECT.load(Ordering::Relaxed) {
        return;
    }
    let mut kept = lock(&COLLECTED);
    kept.extend(awards.iter().cloned());
    let over = kept.len().saturating_sub(COLLECT_MAX);
    kept.drain(..over);
}

fn reset() {
    ACTIVE.store(false, Ordering::Relaxed);
    bots::set_practice(false);
    *lock(&STATE) = State::EMPTY;
    *lock(&TRACKER) = Tracker::new();
    *lock(&MEDALS) = Medals::empty();
    lock(&OFF_SPOT).clear();
    *lock(&LOCAL) = None;
    crate::superbounce_assist::set_marked(None);
    crate::superbounce_view::reset();
    crate::pogo_ride::reset();
    crate::pogo_sound::reset();
    crate::superbounce_ghost::reset();
    crate::superbounce_targets::reset();
    crate::superbounce_options::set(crate::superbounce_options::Options::DEFAULT);
}

/// `superbounce.medal_test`: files a bounce at chain count `chain` (a
/// Monster with `monster`, or a Bounced with `bounced`) for the first
/// local player, as the chain's rule gives it. The HUD line and `medals` show it as a real one's; nothing
/// is sent or recorded. Refused while the mode is off or there is no
/// local player.
pub fn medal_test(
    p: &agent_proto::superbounce::MedalTestParams,
) -> Result<agent_proto::superbounce::MedalTest, String> {
    if !active() {
        return Err("the superbounce mode is off (superbounce.set {\"on\":true})".into());
    }
    let chain = match (p.chain, p.bounced) {
        (Some(0), _) => return Err("chain is 1 or more".into()),
        (Some(n), false) => n,
        (None, true) if p.monster => return Err("monster goes with chain, not bounced".into()),
        (None, true) => 0,
        (Some(_), true) => return Err("give chain or bounced, not both".into()),
        (None, false) => return Err("give chain (1 and up) or bounced".into()),
    };
    let Some((slot, user)) = *lock(&LOCAL) else {
        return Err("no local player with a unit".into());
    };
    let tick = TICKS.load(Ordering::Relaxed);
    let a = lock(&MEDALS).test(slot, Some(user), tick, chain, p.bounced, p.monster);
    let view = lock(&MEDALS).view();
    lock(&STATE).medals = view;
    Ok(agent_proto::superbounce::MedalTest {
        player: slot,
        chain: a.chain,
        medals: a.medals.iter().map(|m| m.id().to_owned()).collect(),
        line: chain::highest(&a.medals).map(|m| m.line()),
    })
}

/// Decides on or off from the game now. Going off clears the state.
fn decide(o: &hooks::Obs) {
    let on = {
        let guest = o.guest();
        let on = on_now(&*guest);
        // The HUD's pieces, as the variant's script options give them
        // (docs/game/superbounce.md, "The HUD's options").
        crate::superbounce_options::set(if on {
            crate::superbounce_options::read(&*guest)
        } else {
            crate::superbounce_options::Options::DEFAULT
        });
        on
    };
    // The bots practise bounces instead of fighting while it is on
    // (docs/game/bots.md, "Practising bounces").
    bots::set_practice(on);
    if ACTIVE.swap(on, Ordering::Relaxed) && !on {
        *lock(&STATE) = State::EMPTY;
        *lock(&TRACKER) = Tracker::new();
        *lock(&MEDALS) = Medals::empty();
        lock(&OFF_SPOT).clear();
        *lock(&LOCAL) = None;
        crate::pogo_ride::reset();
        crate::superbounce_ghost::reset();
        crate::superbounce_targets::reset();
        *lock(&FACING) = crate::superbounce_seams::Facing::new();
    }
}

fn on_tick(o: &hooks::Obs) {
    // Off costs a counter and a flag: the decision is made at the game's
    // start and again every RECHECK ticks.
    if TICKS.fetch_add(1, Ordering::Relaxed) % RECHECK == 0 {
        decide(o);
    }
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let (live, key, players) = {
        let guest = o.guest();
        let m: &dyn GuestMemory = &*guest;
        (read_live(m), scan_key(m), chain::read_players(m))
    };
    let map = key.as_ref().map(|k| k.map.clone());
    let view = key.map(request_scan);
    let (every, room, scan) = match view {
        Some(v) => (v.spots, v.room, v.status),
        None => (Arc::default(), Arc::default(), ScanState::Idle),
    };
    // What the player is given: a proven V's arm only from across its
    // bisector. The medals judge every player's landings on all of them.
    let spots = for_player(&every, live.position);
    let nearest = live.position.and_then(|p| {
        superbounce::nearest(&spots, p).map(|(s, d)| Near {
            distance: d,
            launch: s.launch,
            fall: s.fall,
            edge: s.edge,
        })
    });
    let count = |k: Kind| spots.iter().filter(|s| s.kind == k).count();
    let standing_fits = live
        .position
        .filter(|_| !room.is_empty())
        .map(|p| room.standing_fits(p));
    let step = lock(&TRACKER).step(
        &Tick {
            tick: live.tick,
            word: live.word,
            position: live.position,
            velocity: live.velocity,
            airborne: live.airborne,
            request: live.request,
            crouch_held: crate::superbounce_assist::crouch_held(),
            standing_fits,
        },
        &spots,
    );
    // A bounce off every marked strip: kept for harvesting, its edge from
    // the whole scan.
    let off_spot = match step.off_spot {
        Some(o) => keep_off_spot(o, &scanned_spots().1),
        None => lock(&OFF_SPOT).clone(),
    };
    // The stick the player holds; where it was not read, the one that
    // keeps the body going (superbounce_flight::keep).
    let stick_of = |v: Vec3| {
        live.stick
            .unwrap_or_else(|| crate::superbounce_flight::keep([v[0], v[1]]))
    };
    let landing = match (live.position, live.velocity) {
        (Some(position), Some(velocity)) => crate::superbounce_assist::predict_on(
            &spots,
            &crate::superbounce_assist::Body {
                position,
                velocity,
                yaw: live.yaw.unwrap_or(0.0),
                airborne: live.airborne,
                stick: stick_of(velocity),
            },
            // A strip under a floor is not one the fall lands on.
            &crate::superbounce_assist::Ground {
                floor: &|xy: [f32; 2], top: f32| room.floor_at(xy, top),
                ..crate::superbounce_assist::Ground::open()
            },
        ),
        _ => None,
    };
    // Falling onto a strip, that strip is the target: the navpoint, the
    // world's marks and the lock agree. A crouch while primed keeps the
    // tracker's crouch spot.
    let nav_target = match (&landing, step.nav_target, live.position) {
        (Some(l), Some(n), Some(p)) if n.kind == NavKind::Strip => {
            Some(crate::superbounce_charge::nav_to_landing(l, p))
        }
        _ => step.nav_target,
    };
    // The seam targets and the tag (docs/game/superbounce.md, "Targets"):
    // a tagged seam the mode marks is the chosen strip; the mode's own
    // navpoint is drawn on the ground only, and no target's goes on it.
    let tag = crate::superbounce_targets::step(&crate::superbounce_targets::Input {
        map: map.as_deref(),
        scanned: &scanned_spots().1,
        mode: crate::superbounce_options::current().targets,
        position: live.position,
        aim: live.aim,
        down: live.flashlight,
    });
    let nav_target = crate::superbounce_targets::chosen(
        nav_target,
        landing.is_some(),
        tag,
        &spots,
        live.position,
    );
    let is_primed = live.word.is_some_and(primed);
    // The seams the targets' mode shows: the faced seam is one of them,
    // and each has its line on the map (superbounce_seams).
    let shown = crate::superbounce_targets::current();
    // Primed, with no tag and no strip being fallen onto, the chosen
    // strip is the seam the player faces (docs/game/superbounce.md, "The
    // faced seam"), so the navpoint, the takeoff, the guide and the
    // assist follow where the player is going.
    let faced = match live.position {
        Some(p) if is_primed => {
            lock(&FACING).step(&shown, p, live.yaw, live.velocity, live.airborne)
        }
        _ => {
            *lock(&FACING) = crate::superbounce_seams::Facing::new();
            None
        }
    };
    let nav_target = faced_target(
        nav_target,
        faced.filter(|_| landing.is_none() && tag.is_none()),
        &shown,
        live.position,
    );
    let mut marks = primed_marks(
        crate::superbounce_targets::marks(
            live.position,
            tag,
            nav_target.filter(|_| !live.airborne).map(|n| n.at),
        ),
        is_primed,
    );
    // The set's seams are lines on their edges, not navpoints: the
    // chosen strip's and the tag's are the only ones. The flight after a
    // bounce puts its ranked seams on them (superbounce_second::over).
    marks.targets = [None; crate::superbounce_targets::NEAREST];
    // The assist steers onto the strip that is marked
    // (superbounce_assist::choose).
    crate::superbounce_assist::set_marked(
        nav_target
            .filter(|n| n.kind == NavKind::Strip)
            .map(|n| n.at),
    );
    // The chosen strip's takeoff, from its run's lip or the map's
    // (docs/game/superbounce.md, "The takeoff"), and the navpoint on it
    // while the player is too low for the strip.
    let strip = nav_target
        .filter(|n| n.kind == NavKind::Strip)
        .and_then(|n| {
            spots
                .iter()
                .chain(shown.iter().map(|t| &t.spot))
                .find(|s| s.kind == Kind::Bounce && s.stand == n.at)
        });
    let takeoff = match (strip, live.position) {
        (Some(s), Some(p)) if is_primed && !live.airborne => takeoff_for(
            s,
            p,
            map.as_deref()
                .and_then(|m| crate::superbounce_ghost::lip_for(m, s.stand)),
            |xy, top| room.floor_at(xy, top),
        ),
        _ => None,
    };
    let navpoint = navpoint(
        nav_target,
        takeoff.as_ref(),
        match (strip, live.position) {
            (Some(s), Some(p)) => crate::superbounce_charge::high_enough(p[2], s),
            _ => true,
        },
    );
    *lock(&LOCAL) = players
        .iter()
        .filter(|p| p.tick.is_some())
        .filter_map(|p| Some((p.slot, p.user?)))
        .min_by_key(|(_, user)| *user);
    let tick = live.tick.unwrap_or_else(|| TICKS.load(Ordering::Relaxed));
    let (awards, medals, physics, bodies) = {
        let mut m = lock(&MEDALS);
        let awards = m.step(tick, &players, &every);
        (awards, m.view(), m.physics().to_vec(), m.bodies().to_vec())
    };
    keep_awards(&awards);
    // In the flight after a bounce, the seams it can reach, the best
    // first, on the target navpoints (docs/game/superbounce.md, "The
    // second bounce").
    let second = match (live.position, live.velocity) {
        (Some(position), Some(velocity))
            if crate::superbounce_options::current().second_bounce
                && crate::superbounce_second::window(
                    live.airborne,
                    (*lock(&LOCAL)).map(|(slot, _)| slot),
                    &medals,
                ) =>
        {
            crate::superbounce_second::rank(
                &crate::superbounce_targets::current(),
                &crate::superbounce_assist::Body {
                    position,
                    velocity,
                    yaw: live.yaw.unwrap_or(0.0),
                    airborne: live.airborne,
                    stick: stick_of(velocity),
                },
                &crate::superbounce_assist::Ground {
                    floor: &|xy: [f32; 2], top: f32| room.floor_at(xy, top),
                    ..crate::superbounce_assist::Ground::open()
                },
            )
        }
        _ => [None; RANKED],
    };
    let marks = crate::superbounce_second::over(marks, &second);
    // The bounce rule's airborne phase and the approach guide, on the
    // seam the player is going for (docs/game/superbounce.md, "The
    // approach guide"), its jump cue settled against the last tick's.
    let (last_guide, last_apex) = {
        let previous = lock(&STATE);
        (previous.guide, previous.phase.map(|p| p.apex))
    };
    let guide_body = match (live.position, live.velocity) {
        (Some(position), Some(velocity)) if is_primed => Some(crate::superbounce_guide::Body {
            position,
            velocity,
            airborne: live.airborne,
            primed: is_primed,
            lift: live.lift,
            stick: stick_of(velocity),
        }),
        _ => None,
    };
    let phase = guide_body.filter(|b| b.airborne).and_then(|body| {
        let floors = room.floors_near(
            [body.position[0], body.position[1]],
            crate::superbounce_guide::FLOORS_REACH,
        );
        crate::superbounce_guide::phase(
            &body,
            &crate::superbounce_guide::Air {
                floors: &floors,
                apex: last_apex,
                crouching: live.request,
            },
        )
    });
    let guide = match guide_body {
        Some(body) if crate::superbounce_options::current().approach_guide => {
            let seams: Vec<Vec3> = marks
                .tagged
                .into_iter()
                .chain(marks.tag.map(|m| m.at))
                .chain(marks.targets.iter().flatten().map(|m| m.at))
                .collect();
            crate::superbounce_guide::target(
                body.position,
                is_primed,
                landing.map(|l| l.stand),
                nav_target
                    .filter(|n| n.kind == NavKind::Strip)
                    .map(|n| n.at),
                &seams,
            )
            .and_then(|at| {
                let shown = crate::superbounce_targets::current();
                let spot = crate::superbounce_guide::spot_at(
                    spots.iter().chain(shown.iter().map(|t| &t.spot)),
                    at,
                )?;
                crate::superbounce_guide::guide_with_phase(
                    &body,
                    spot,
                    // The takeoff is the chosen strip's: on another seam
                    // there is none.
                    takeoff.filter(|_| strip.is_some_and(|s| s.stand == spot.stand)),
                    phase,
                )
                .map(|g| g.settled(last_guide.as_ref()))
            })
        }
        _ => None,
    };
    // The bots' and the verifier's bounces kept as ghost runs, and the
    // ghost the player asks for (docs/game/superbounce.md, "Ghost runs").
    {
        let guest = o.guest();
        let m: &dyn GuestMemory = &*guest;
        // Only the strip the player is going for may show its run.
        let chosen = nav_target
            .filter(|n| n.kind == NavKind::Strip)
            .map(|n| n.at);
        crate::superbounce_ghost::tick(m, tick, &players, &awards, map.as_deref(), &spots, chosen);
        // A rules script hears the tick's touchdowns, launches and ends
        // of runs, then each bounce on the tick it is filed.
        crate::rules::feature::bounces(m, &bodies, &physics, &awards);
        // The Pogo Stick's riders, on the chain's verdicts
        // (docs/game/pogo-stick.md, "Phases and tricks").
        // The local players' pistons are heard (pogo_sound.rs).
        let cues = crate::pogo_ride::on_tick(m, map.as_deref(), &players, &awards);
        crate::pogo_sound::play(&cues);
    }
    // The film and Live get each bounce and Bounced, as the kills.
    if !awards.is_empty()
        && let Some(t) = lock(&TELEMETRY).clone()
    {
        let guest = o.guest();
        chain::record(&*t, &guest, &awards);
    }
    // The Pogo Stick's chase camera (docs/game/pogo-stick.md, "The
    // camera"): on while the local player holds a stick, its boom as
    // long as the level leaves room for.
    {
        use superbounce::view::LineOfSight;
        let sight = room.view();
        crate::pogo_camera::on_tick(
            crate::pogo_ride::local().is_some(),
            live.position,
            live.aim,
            &|from, to| sight.clear(from, to),
        );
    }
    *lock(&STATE) = State {
        active: true,
        word: live.word,
        position: live.position,
        stick: live.stick,
        nearest,
        scan,
        map,
        bounce: count(Kind::Bounce),
        crouch: count(Kind::Crouch),
        airborne: live.airborne,
        vz: live.vz,
        yaw: live.yaw,
        velocity: live.velocity,
        aim: live.aim,
        tick: live.tick,
        marks,
        switched: crate::superbounce_targets::switched(),
        second,
        charge: step.charge,
        verb: step.verb,
        nav_target,
        navpoint,
        takeoff,
        landing,
        phase,
        guide,
        last_landing: step.last_landing,
        off_spot,
        still_ticks: step.still_ticks,
        standing_fits,
        medals,
    };
    // The bounce view on the flashlight and the tracker.
    crate::superbounce_view::on_tick(o, &spots, &room);
}

// ---- the feature ----

/// The tick observer that publishes the state.
pub struct SuperbounceMode;

impl hooks::Feature for SuperbounceMode {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Superbounce game type: HUD and spot lines")
            .doc("docs/game/superbounce.md")
            // A tick observer and start and end observers only: a recorded
            // film's hook digest does not include it.
            .presentation_only()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // State is rebuilt from guest RAM every tick and the scan cache holds
        // only map-derived spots, so a snapshot needs nothing from us. The
        // charge tracker (still ticks, the lost second, the last landing)
        // is presentation only: after a load it restarts from the next tick.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        b.game().on_start("superbounce: a new game", |o| {
            reset();
            TICKS.store(1, Ordering::Relaxed);
            decide(o);
        });
        b.game().on_end("superbounce: the game ended", |_| reset());
        b.frame().on_tick("superbounce: state", on_tick);
        Ok(())
    }
}

/// Live telemetry, which records the medals' events
/// (`superbounce_chain::record`); `None` without Live.
static TELEMETRY: Mutex<Option<Arc<live_telemetry::Telemetry>>> = Mutex::new(None);

/// Adds the feature to `registry`, recording the bounces to `telemetry`.
pub fn install(
    registry: &mut hooks::Registry,
    telemetry: Option<Arc<live_telemetry::Telemetry>>,
) -> anyhow::Result<()> {
    *lock(&TELEMETRY) = telemetry;
    registry
        .add(&SuperbounceMode)
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("superbounce: {e}"))
}

/// Registers the world-line source before boot (windowed hosts).
#[cfg(feature = "gpu")]
pub use gpu::register;

#[cfg(feature = "gpu")]
mod gpu {
    use std::time::Instant;

    use super::*;
    use crate::superbounce_prompts::Callout;
    use crate::superbounce_world::{LockMarks, lock_callouts};
    use crate::world_lines::{ImageMarks, ImageValidity, WorldLine, WorldLineSource};

    /// Draws the spot lines while the mode is on.
    pub struct Lines;

    pub fn register(host: &mut crate::Host) {
        host.add_world_line_source(Arc::new(Lines));
        host.add_world_line_source(Arc::new(crate::superbounce_view::Source));
        crate::superbounce_ghost::register(host);
        // The approach guide's marks are the HUD kit's.
        crate::hud_kit_world::register(host);
    }

    impl WorldLineSource for Lines {
        fn active(&self) -> bool {
            super::active()
        }

        fn capture(&self, guest: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            let s = state();
            let spots = current_spots();
            let callouts = crate::superbounce_world::frame(guest, &s, &spots);
            // The lock-on eases by the wall clock between this tick's
            // capture and the next (docs/game/superbounce.md, "Lock-on").
            // Fullscreen the approach guide's box is on the seam in its
            // place.
            let full = matches!(crate::hud_kit::mode(guest), Ok(hud_kit_core::Mode::Full));
            let lock = LockMarks::capture(guest, &s, full, Instant::now());
            (!callouts.is_empty() || lock.is_some())
                .then(|| Arc::new(Sample { callouts, lock }) as Arc<dyn ImageMarks>)
        }
    }

    #[derive(Debug)]
    struct Sample {
        callouts: Vec<Callout>,
        lock: Option<LockMarks>,
    }

    fn draw(callouts: &[Callout], visitor: &mut dyn FnMut(WorldLine<'_>)) {
        for c in callouts {
            visitor(WorldLine {
                edges: &c.edges,
                anchor: c.anchor,
                color: c.color,
                dashed: false,
                label: c.label.as_deref(),
                style: c.style,
            });
        }
    }

    impl ImageMarks for Sample {
        /// The marks, and the lock-on where its ease ends.
        fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>)) {
            draw(&self.callouts, visitor);
            if let Some(l) = &self.lock {
                draw(&lock_callouts(&l.ease.to, &l.look), visitor);
            }
        }

        /// The marks, and the lock-on as it is at the present's time.
        fn visit(
            &self,
            now: &dyn Fn() -> Instant,
            visitor: &mut dyn FnMut(WorldLine<'_>),
        ) -> Option<ImageValidity> {
            let validity = self.validity(now)?;
            draw(&self.callouts, visitor);
            if let Some(l) = &self.lock {
                draw(&l.callouts(now()), visitor);
            }
            Some(validity)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The faced seam of the targets' set is the chosen strip while the
    /// navpoint is a strip's; a crouch spot's navpoint, no faced seam and
    /// a seam that left the set leave it as it is.
    #[test]
    fn the_faced_seam_is_the_chosen_strip() {
        use crate::superbounce_targets::{Class, Target};
        let seam = |x: f32, fall: f32| Target {
            spot: Spot {
                kind: Kind::Bounce,
                edge: [[x, -1.0, 0.0], [x, 1.0, 0.0]],
                stand: [x, 0.0, 0.01],
                fall,
                ..Spot::default()
            },
            class: Class::Slot,
        };
        let shown = [seam(4.0, 0.0), seam(12.0, 2.43)];
        let nav = |at: Vec3, kind| NavTarget {
            at,
            kind,
            distance: 4.0,
            fall: 0.0,
        };
        let me = Some([0.0, 0.0, 0.01]);
        let near = Some(nav(shown[0].at(), NavKind::Strip));
        // Facing the far seam: it is the strip, with its own drop.
        let got = faced_target(near, Some(shown[1].at()), &shown, me).unwrap();
        assert_eq!(
            (got.at, got.kind, got.fall),
            (shown[1].at(), NavKind::Strip, 2.43)
        );
        assert!((got.distance - 12.0).abs() < 1e-5);
        // A seam the mode marks nothing of still gets a strip.
        assert_eq!(
            faced_target(None, Some(shown[1].at()), &shown, me).map(|n| n.at),
            Some(shown[1].at())
        );
        // No faced seam, one not in the set, no position, and a crouch
        // spot's navpoint: as it was.
        assert_eq!(faced_target(near, None, &shown, me), near);
        assert_eq!(faced_target(near, Some([99.0, 0.0, 0.0]), &shown, me), near);
        assert_eq!(faced_target(near, Some(shown[1].at()), &shown, None), near);
        let crouch = Some(nav([1.0, 0.0, 0.0], NavKind::Crouch));
        assert_eq!(
            faced_target(crouch, Some(shown[1].at()), &shown, me),
            crouch
        );
    }

    /// The mode reads the origin a teleport places, the frame of every
    /// spot's heights: read from the bounding sphere's centre, every
    /// height over a seam was about 0.4 too big (fall times 2 to 3 ticks
    /// long, touchdowns too fast, landings by a strip missed by the 0.5
    /// radius). The centre is read for the lift alone.
    #[test]
    fn the_mode_reads_the_origin_a_teleport_places() {
        use guest_layout::Object;
        assert_eq!(OBJECT_POSITION, Object::POSITION.offset);
        assert_eq!(OBJECT_POSITION, 0x64);
        assert_ne!(OBJECT_POSITION, Object::BOUND_CENTER.offset);
        assert_eq!(OBJECT_POSITION, crate::telemetry::layout::BODY_POSITION);
        assert_eq!(OBJECT_CENTRE, 0x30);
        assert_eq!(guest_layout::Unit::THROTTLE.offset, 0x1b0);
        assert_eq!(State::EMPTY.stick, None);
    }

    #[test]
    fn the_agent_override_names_itself_and_can_be_undone() {
        assert!(set_override(Some(true)).is_err(), "no force-on");
        assert_eq!(OVERRIDE.load(Ordering::Relaxed), FOLLOW);
        assert_eq!(set_override(Some(false)), Ok("off"));
        assert_eq!(OVERRIDE.load(Ordering::Relaxed), FORCE_OFF);
        assert_eq!(set_override(None), Ok("game"));
        assert_eq!(OVERRIDE.load(Ordering::Relaxed), FOLLOW);
    }

    #[test]
    fn the_word_is_primed_at_minus_three_only() {
        assert!(primed(0xffff_fffd));
        assert!(primed((-3i32) as u32));
        for w in [0, 1, 0xffff_fffe, 0xffff_fffc, 3] {
            assert!(!primed(w), "{w:#x}");
        }
        let s = State {
            word: Some(0xffff_fffd),
            ..State::EMPTY
        };
        assert!(s.primed());
        assert!(!State::EMPTY.primed());
    }

    fn spot(kind: Kind, x: f32, shape: superbounce::Shape, status: superbounce::Status) -> Spot {
        Spot {
            kind,
            stand: [x, 0.0, 0.0],
            edge: [[x, -0.5, 0.0], [x, 0.5, 0.0]],
            shape,
            status,
            ..Spot::default()
        }
    }

    #[test]
    fn creases_are_hidden_until_a_run_proves_a_strip_and_then_only_proved_strips_are_marked() {
        use superbounce::{Shape, Status};
        let xs = |spots: &[Spot]| -> Vec<f32> { spots.iter().map(|s| s.stand[0]).collect() };
        // No strip proved: the slots, never a crease; crouch spots stay;
        // failed ones go.
        let scan = [
            spot(Kind::Bounce, 1.0, Shape::Slot, Status::Likely),
            spot(Kind::Bounce, 2.0, Shape::Crease, Status::Likely),
            spot(Kind::Bounce, 3.0, Shape::Slot, Status::Failed),
            spot(Kind::Crouch, 4.0, Shape::Slot, Status::Likely),
            spot(Kind::Crouch, 5.0, Shape::Slot, Status::Proven),
            spot(Kind::Crouch, 6.0, Shape::Slot, Status::Failed),
        ];
        assert_eq!(xs(&marked(&scan)), [1.0, 4.0, 5.0]);
        assert!(!verified(&scan[0]) && verified(&scan[4]));
        // The crease is the sim's candidate, not play's.
        assert!(scan[1].candidate() && !scan[0].candidate());
        // A proved crouch spot proves no strip: the slots still show.
        // Once a run proves a strip, only proved strips are marked, a
        // crease among them; an unproved slot is not.
        let mut run = scan;
        run[1].status = Status::Proven;
        assert_eq!(xs(&marked(&run)), [2.0, 4.0, 5.0]);
        run[0].status = Status::Proven;
        assert_eq!(xs(&marked(&run)), [1.0, 2.0, 4.0, 5.0]);
        assert!(marked(&[]).is_empty());
    }

    #[test]
    fn with_a_proven_arm_a_player_on_one_side_is_marked_the_arm_across() {
        use superbounce::{Arm, Shape, Status};
        // A V on its tip at the origin, opening along +x: arm A at +y,
        // approached from -y; arm B at -y, approached from +y.
        let arm = |y: f32, status: Status| Spot {
            kind: Kind::Bounce,
            edge: [[0.0, 0.0, 0.0], [4.0, y, 0.0]],
            stand: [2.0, y / 2.0, 0.01],
            shape: Shape::Vee,
            status,
            arm: Some(Arm {
                pivot: [0.0, 0.0, 0.0],
                bisector: [1.0, 0.0],
                from: [0.0, -y.signum()],
            }),
            ..Spot::default()
        };
        let tip = Spot {
            edge: [[0.0; 3]; 2],
            stand: [0.0, 0.0, 0.01],
            arm: None,
            ..arm(1.0, Status::Likely)
        };
        // Nothing proven: nothing of the V is marked, from either side.
        let scan = vec![arm(1.0, Status::Likely), arm(-1.0, Status::Likely), tip];
        let none = Arc::new(marked(&scan));
        assert!(for_player(&none, Some([1.0, 1.0, 0.0])).is_empty());
        assert!(for_player(&none, Some([1.0, -1.0, 0.0])).is_empty());
        // Both arms proven: the player on side A (+y) is marked arm B
        // (-y), and the other way round.
        let proven = Arc::new(marked(&[
            arm(1.0, Status::Proven),
            arm(-1.0, Status::Proven),
            tip,
        ]));
        assert_eq!(proven.len(), 2);
        let on_a = for_player(&proven, Some([1.0, 0.5, 0.0]));
        assert_eq!(on_a.len(), 1, "{on_a:?}");
        assert_eq!(on_a[0].edge[1][1], -1.0);
        let on_b = for_player(&proven, Some([1.0, -0.5, 0.0]));
        assert_eq!(on_b.len(), 1, "{on_b:?}");
        assert_eq!(on_b[0].edge[1][1], 1.0);
        // Only arm A proven: a player on A's own side is marked none of
        // the V; across from it, A.
        let one = Arc::new(marked(&[
            arm(1.0, Status::Proven),
            arm(-1.0, Status::Likely),
            tip,
        ]));
        assert!(for_player(&one, Some([1.0, 0.5, 0.0])).is_empty());
        assert_eq!(for_player(&one, Some([1.0, -0.5, 0.0])).len(), 1);
        // No V arm among the spots: the same list.
        let plain = Arc::new(vec![tip]);
        assert!(Arc::ptr_eq(&for_player(&plain, Some([0.0; 3])), &plain));
    }

    fn near(distance: f32) -> Near {
        Near {
            distance,
            launch: 17.85,
            fall: 0.0,
            edge: [[0.0; 3]; 2],
        }
    }

    #[test]
    fn the_bearing_turns_clockwise_from_where_the_player_looks() {
        use std::f32::consts::{FRAC_PI_2, PI};
        let edge = |x: f32, y: f32| [[x, y - 1.0, 0.0], [x, y + 1.0, 0.0]];
        let close = |a: Option<f32>, b: f32| a.is_some_and(|a| (a - b).abs() < 1e-4);
        // Facing +X: a spot ahead, to the right (-Y), behind.
        assert!(close(bearing([0.0; 3], 0.0, edge(5.0, 0.0)), 0.0));
        let right = [[0.0, -5.0, 0.0], [0.0, -5.0, 0.0]];
        assert!(close(bearing([0.0; 3], 0.0, right), FRAC_PI_2));
        let behind = bearing([0.0; 3], 0.0, edge(-5.0, 0.0)).unwrap();
        assert!((behind.abs() - PI).abs() < 1e-4);
        // Facing +Y (yaw pi/2), a spot on +X is to the right.
        assert!(close(
            bearing([0.0; 3], FRAC_PI_2, edge(5.0, 0.0)),
            FRAC_PI_2
        ));
        // The edge's nearest point, not its middle; height is ignored.
        let long = [[2.0, 0.0, 9.0], [2.0, 50.0, 9.0]];
        assert!(close(bearing([0.0; 3], 0.0, long), 0.0));
        // On the spot, or without an aim, there is no arrow.
        assert_eq!(bearing([5.0, 0.0, 0.0], 0.0, edge(5.0, 0.0)), None);
        assert_eq!(bearing([0.0; 3], f32::NAN, edge(5.0, 0.0)), None);
        let s = State {
            position: Some([-5.0, 0.0, 0.0]),
            yaw: Some(0.0),
            nearest: Some(near(5.0)),
            ..State::EMPTY
        };
        assert!(close(s.bearing(), 0.0));
        assert_eq!(State { yaw: None, ..s }.bearing(), None);
    }

    #[test]
    fn the_state_method_takes_no_parameters() {
        use serde_json::json;
        assert!(no_params(&json!(null)).is_ok());
        assert!(no_params(&json!({})).is_ok());
        assert!(no_params(&json!({"x": 1})).is_err());
        assert!(no_params(&json!([1])).is_err());
    }

    #[test]
    fn the_agent_reply_carries_the_word_both_ways() {
        let s = State {
            active: true,
            word: Some(0xffff_fffd),
            position: Some([1.0, 2.0, 3.0]),
            nearest: Some(near(2.0)),
            scan: ScanState::Failed("x".into()),
            map: Some("ascension".into()),
            bounce: 3,
            crouch: 1,
            airborne: true,
            vz: Some(12.5),
            yaw: Some(0.5),
            ..State::EMPTY
        };
        let r = reply(&s);
        assert!(r.active && r.primed);
        assert_eq!(r.word.as_deref(), Some("fffffffd"));
        assert_eq!(r.word_signed, Some(-3));
        assert_eq!(r.position, Some([1.0, 2.0, 3.0]));
        assert_eq!(r.scan.status, "failed");
        assert_eq!(r.scan.error.as_deref(), Some("x"));
        assert_eq!((r.scan.bounce, r.scan.crouch), (3, 1));
        assert_eq!(r.airborne, Some(true));
        assert_eq!((r.vz, r.yaw), (Some(12.5), Some(0.5)));
        let idle = reply(&State::EMPTY);
        assert_eq!((idle.airborne, idle.vz, idle.yaw), (None, None, None));
        assert_eq!(idle.scan.status, "idle");
        assert!(!idle.active && !idle.primed && idle.word.is_none());
        assert!(idle.charge.is_none() && idle.nav_target.is_none());
        assert!(idle.landing.is_none() && idle.last_landing.is_none());
        assert!(idle.verb.is_none() && idle.standing_fits.is_none());
    }

    #[test]
    fn the_agent_reply_carries_the_charge_the_target_and_the_landings() {
        use agent_proto::superbounce as wire;
        let s = State {
            active: true,
            charge: Some(Charge {
                phase: ChargePhase::Primed,
                fill: 0.5,
                lost: None,
            }),
            still_ticks: 19,
            verb: Some(Verb::WalkOutToStand),
            nav_target: Some(NavTarget {
                at: [1.0, 2.0, 3.0],
                kind: NavKind::Strip,
                distance: 4.0,
                fall: 1.0,
            }),
            takeoff: Some(crate::superbounce_guide::Takeoff {
                at: [1.0, 0.5, 4.0],
                heading: [0.0, 1.0],
                half: 0.05,
                speed: 0.8,
                leave: crate::superbounce_guide::Leave::Drop,
                checked: true,
            }),
            landing: Some(Predicted {
                at: [1.0, 2.0, -3.0],
                aim: [1.0, 2.0],
                drift: [0.0; 2],
                across: 0.0,
                verdict: Course::Fixable,
                speed: 5.5,
                need: 4.0,
                edge: [[0.0, 2.0, -3.0], [2.0, 2.0, -3.0]],
                stand: [1.0, 2.0, -3.0],
                fall: 1.0,
                steer: [0.0, 1.0],
            }),
            last_landing: Some(Landing {
                result: LandingResult::TooSlow,
                tick: 6753,
                at: [0.5, 0.5, -3.0],
            }),
            off_spot: vec![OffSpot {
                tick: 7100,
                at: [2.0, 3.0, 4.0],
                edge: Some([[2.0, 2.5, 4.0], [2.0, 3.5, 4.0]]),
                speed_in: 6.0,
                launch: 4.5,
            }],
            standing_fits: Some(false),
            ..State::EMPTY
        };
        let r = reply(&s);
        let c = r.charge.unwrap();
        assert_eq!(
            (c.phase, c.fill, c.still_ticks),
            (wire::ChargePhase::Primed, 0.5, 19)
        );
        assert_eq!(r.verb, Some(wire::Verb::WalkOutToStand));
        let n = r.nav_target.unwrap();
        assert_eq!(
            (n.at, n.kind, n.distance, n.fall),
            ([1.0, 2.0, 3.0], wire::NavKind::Strip, 4.0, 1.0)
        );
        assert_eq!(r.takeoff, Some([1.0, 0.5, 4.0]));
        let p = r.landing.unwrap();
        assert_eq!(
            (p.verdict, p.speed, p.need, p.miss, p.locked),
            (wire::Course::Fixable, 5.5, 4.0, 0.0, false)
        );
        let l = r.last_landing.unwrap();
        assert_eq!(
            (l.result, l.tick, l.at),
            (wire::LandingResult::TooSlow, 6753, [0.5, 0.5, -3.0])
        );
        let o = &r.off_spot[0];
        assert_eq!(
            (o.tick, o.at, o.edge, o.speed_in, o.launch),
            (
                7100,
                [2.0, 3.0, 4.0],
                Some([[2.0, 2.5, 4.0], [2.0, 3.5, 4.0]]),
                6.0,
                4.5
            )
        );
        assert_eq!(r.standing_fits, Some(false));
        assert_eq!(r.lost_reason, None);
    }

    /// An off-spot bounce by an unmarked scanned seam takes that seam's
    /// edge; one by nothing scanned keeps none; the list keeps the last
    /// `OFF_SPOT_MAX`.
    #[test]
    fn an_off_spot_bounce_takes_the_scanned_edge_and_the_list_is_capped() {
        lock(&OFF_SPOT).clear();
        let seam = Spot {
            kind: Kind::Bounce,
            edge: [[30.0, 4.0, 1.0], [30.0, 6.0, 1.0]],
            stand: [30.0, 5.0, 1.0],
            status: superbounce::Status::Likely,
            ..Spot::default()
        };
        let off = |at: Vec3, tick: u32| OffSpot {
            tick,
            at,
            edge: None,
            speed_in: 6.0,
            launch: 5.0,
        };
        let kept = keep_off_spot(off([30.05, 5.0, 1.1], 1), &[seam]);
        assert_eq!(kept[0].edge, Some(seam.edge));
        let kept = keep_off_spot(off([50.0, 5.0, 1.1], 2), &[seam]);
        assert_eq!(kept[1].edge, None);
        for t in 3..30 {
            keep_off_spot(off([50.0, 5.0, 1.1], t), &[seam]);
        }
        let kept = lock(&OFF_SPOT).clone();
        assert_eq!(kept.len(), OFF_SPOT_MAX);
        assert_eq!(kept.last().unwrap().tick, 29);
        lock(&OFF_SPOT).clear();
    }

    #[test]
    fn the_agent_reply_says_why_the_charge_went_while_lost_and_after() {
        use agent_proto::superbounce as wire;
        let lost = Lost {
            reason: LostReason::StoodStill,
            tick: 7000,
        };
        let s = State {
            active: true,
            charge: Some(Charge {
                phase: ChargePhase::Lost,
                fill: 0.0,
                lost: Some(lost),
            }),
            verb: Some(Verb::ChargeAgain(lost)),
            ..State::EMPTY
        };
        let r = reply(&s);
        assert_eq!(r.lost_reason, Some(wire::LostReason::StoodStill));
        assert_eq!(r.verb, Some(wire::Verb::ChargeAgain));
        // After the lost second the charge is hidden; the verb keeps it.
        let after = State {
            charge: None,
            verb: Some(Verb::ChargeAgain(Lost {
                reason: LostReason::Crouched,
                ..lost
            })),
            ..s.clone()
        };
        assert_eq!(reply(&after).lost_reason, Some(wire::LostReason::Crouched));
        let unknown = State {
            verb: Some(Verb::ChargeAgain(Lost {
                reason: LostReason::Unknown,
                ..lost
            })),
            ..after.clone()
        };
        assert_eq!(reply(&unknown).lost_reason, Some(wire::LostReason::Unknown));
        let warned = State {
            verb: Some(Verb::KeepMoving),
            ..State::EMPTY
        };
        let r = reply(&warned);
        assert_eq!(
            (r.verb, r.lost_reason),
            (Some(wire::Verb::KeepMoving), None)
        );
    }

    #[test]
    fn standing_fits_is_absent_while_the_mode_is_off() {
        ACTIVE.store(false, Ordering::Relaxed);
        assert_eq!(standing_fits_at([0.0; 3]), None);
    }

    #[test]
    fn the_sim_takes_the_awards_only_while_it_collects() {
        let award = |seq, chain| chain::Award {
            seq,
            slot: 2,
            user: None,
            tick: 100 + seq as u32,
            chain,
            medals: chain::bounce_medals(chain).to_vec(),
            spot: None,
            strips: None,
        };
        collect_awards(false);
        keep_awards(&[award(1, 1)]);
        assert!(take_awards().is_empty(), "off keeps nothing");
        collect_awards(true);
        keep_awards(&[award(2, 1), award(3, 2)]);
        let taken = take_awards();
        assert_eq!(taken.iter().map(|a| a.seq).collect::<Vec<_>>(), vec![2, 3]);
        assert!(take_awards().is_empty(), "a take empties the queue");
        let many: Vec<_> = (0..COLLECT_MAX as u64 + 5).map(|i| award(i, 1)).collect();
        keep_awards(&many);
        let taken = take_awards();
        assert_eq!(taken.len(), COLLECT_MAX);
        assert_eq!(taken[0].seq, 5, "the oldest go first");
        collect_awards(false);
    }

    /// The reply carries the velocity, the HUD's options and the seam
    /// targets' marks.
    #[test]
    fn the_agent_reply_has_the_velocity_the_options_and_the_marks() {
        use crate::superbounce_targets::Mark;
        let mark = |x: f32| Mark {
            at: [x, 0.0, 1.0],
            distance: x,
        };
        let s = State {
            active: true,
            velocity: Some([1.0, 2.0, -3.0]),
            marks: Marks {
                tag: None,
                targets: [Some(mark(4.0)), Some(mark(9.0))],
                tagged: Some([4.0, 0.0, 1.0]),
            },
            ..State::EMPTY
        };
        let r = reply(&s);
        assert_eq!(r.velocity, Some([1.0, 2.0, -3.0]));
        assert_eq!(r.targets.tag, Some([4.0, 0.0, 1.0]));
        assert_eq!(r.targets.marked, vec![[4.0, 0.0, 1.0], [9.0, 0.0, 1.0]]);
        let o = options_reply(crate::superbounce_options::Options {
            targets: crate::superbounce_options::TargetMode::All,
            medals: false,
            ..crate::superbounce_options::Options::DEFAULT
        });
        assert_eq!(o.targets, "all");
        assert!(o.velocity && o.alignment && o.miss_info && !o.medals);
        let bare = reply(&State::EMPTY);
        assert_eq!((bare.velocity, bare.targets.tag), (None, None));
        assert!(bare.targets.marked.is_empty());
    }

    /// Not primed, no target seam is marked: there is nothing to land
    /// yet. The tag stays either way.
    #[test]
    fn the_target_seams_are_marked_while_primed_only() {
        use crate::superbounce_targets::Mark;
        let mark = |x: f32| {
            Some(Mark {
                at: [x, 0.0, 0.0],
                distance: x,
            })
        };
        let marks = Marks {
            tag: mark(9.0),
            targets: [mark(3.0), mark(5.0)],
            tagged: Some([9.0, 0.0, 0.0]),
        };
        assert_eq!(primed_marks(marks, true), marks);
        let idle = primed_marks(marks, false);
        assert_eq!(idle.targets, [None, None]);
        assert_eq!((idle.tag, idle.tagged), (marks.tag, marks.tagged));
    }

    /// A ledge `height` over a seam floor at z 0, its lip at y `lip`; the
    /// seam's edge along X at y 4, drifting +Y, its landing point just
    /// before the edge.
    fn ledge_room(height: f32, lip: f32) -> (Room, Spot) {
        let quad = |y0: f32, y1: f32, z: f32| superbounce::Poly {
            verts: vec![[-5.0, y0, z], [5.0, y0, z], [5.0, y1, z], [-5.0, y1, z]],
            normal: [0.0, 0.0, 1.0],
        };
        let room = Room::new(&[quad(-10.0, lip, height), quad(lip, 10.0, 0.0)]);
        let spot = Spot {
            kind: Kind::Bounce,
            edge: [[-1.0, 4.0, 0.0], [1.0, 4.0, 0.0]],
            stand: [0.0, 3.976, 0.0],
            drift: [0.0, 1.0],
            fall: height,
            need: 5.0,
            ..Spot::default()
        };
        (room, spot)
    }

    /// Whether the takeoff `t` sends a body into `spot`'s catch at its
    /// need: a walk-off from the lip, a jump leaving the floor
    /// `PRESS_LEAD` ticks on from the line.
    fn lands(spot: &Spot, t: &crate::superbounce_guide::Takeoff) -> bool {
        use crate::superbounce_guide::{
            CENTRE_LIFT, CLEARANCE, JUMP_VZ, Leave, PRESS_LEAD, Seam, TICK, caught, impact,
        };
        let seam = Seam::of(spot, [spot.stand[0], spot.stand[1]]);
        let h = t.heading;
        let (from, vz) = match t.leave {
            Leave::Drop => (0.0, 0.0),
            Leave::Jump => (t.speed * PRESS_LEAD as f32 * TICK, JUMP_VZ),
        };
        let at = [
            t.at[0] + h[0] * from,
            t.at[1] + h[1] * from,
            t.at[2] + CLEARANCE,
        ];
        let v = [h[0] * t.speed, h[1] * t.speed, vz];
        let stick = crate::superbounce_flight::keep([v[0], v[1]]);
        impact(at, v, stick, CENTRE_LIFT, |xy| {
            seam.contact(xy, CENTRE_LIFT)
        })
        .is_some_and(|i| caught(&seam, &i) && i.speed >= spot.need)
    }

    /// The takeoff is a place on the real floor of the ledge to come from,
    /// found from the strip's floor below it all the same (the review's
    /// frames 67 to 87: the player on the seam was told only `Land, 7 m`),
    /// and a flight from it lands in the catch at the seam's need.
    #[test]
    fn the_takeoff_is_on_the_ledges_floor_and_lands_in_the_catch() {
        let (room, spot) = ledge_room(3.45, 2.95);
        let floor = |xy: [f32; 2], top: f32| room.floor_at(xy, top);
        // The player stands on the seam's floor, under the ledge.
        let t = takeoff_for(&spot, [0.0, 3.5, 0.0], None, floor).unwrap();
        assert_eq!(t.at[2], 3.45);
        assert!(t.at[1] <= 2.95, "{t:?}");
        assert_eq!(t.heading, [0.0, 1.0]);
        assert!(t.checked && lands(&spot, &t), "{t:?}");
        // From the ledge itself, the same takeoff.
        assert_eq!(takeoff_for(&spot, [0.0, 1.0, 3.45], None, floor), Some(t));
    }

    /// A run's lip comes first: from it the takeoff is a jump back along
    /// its floor that lands in the catch. A run's lip that is on no floor
    /// is passed over for the map's.
    #[test]
    fn a_runs_lip_comes_before_the_maps() {
        use crate::superbounce_guide::{Leave, Lip};
        let (room, spot) = ledge_room(3.45, 2.95);
        let floor = |xy: [f32; 2], top: f32| room.floor_at(xy, top);
        let run = Lip {
            at: [0.0, 2.5, 3.45],
            heading: [0.0, 1.0],
            speed: Some(1.2),
            run: Some(Leave::Jump),
        };
        let t = takeoff_for(&spot, [0.0, 3.5, 0.0], Some(run), floor).unwrap();
        assert_eq!((t.leave, t.at[2], t.speed), (Leave::Jump, 3.45, 1.2));
        assert!(t.at[1] < 2.5 && t.checked && lands(&spot, &t), "{t:?}");
        let map = takeoff_for(&spot, [0.0, 3.5, 0.0], None, floor);
        let lost = Lip {
            at: [0.0, 2.5, 10.0],
            ..run
        };
        assert_eq!(takeoff_for(&spot, [0.0, 3.5, 0.0], Some(lost), floor), map);
    }

    /// A ledge no modelled flight from reaches the catch still gives the
    /// place to go: its lip, unchecked, a walk-off from its edge.
    #[test]
    fn a_ledge_too_far_back_is_still_where_to_go() {
        use crate::superbounce_guide::Leave;
        let (room, spot) = ledge_room(3.45, 0.5);
        let t = takeoff_for(&spot, [0.0, 3.5, 0.0], None, |xy, top| {
            room.floor_at(xy, top)
        })
        .unwrap();
        assert_eq!((t.leave, t.checked, t.at[2]), (Leave::Drop, false, 3.45));
        assert!(t.at[1] <= 0.5 && t.at[1] > 0.4, "{t:?}");
        // No ledge high enough anywhere: no takeoff.
        let (room, spot) = ledge_room(1.0, 0.5);
        let deep = Spot { fall: 3.45, ..spot };
        assert_eq!(
            takeoff_for(&deep, [0.0; 3], None, |xy, top| room.floor_at(xy, top)),
            None
        );
    }

    /// The navpoint is on the takeoff while the player's floor is too low
    /// for the chosen strip, and on the target's spot otherwise.
    #[test]
    fn the_navpoint_goes_to_the_takeoff_while_too_low() {
        use crate::superbounce_guide::{Leave, Takeoff};
        let strip = NavTarget {
            at: [0.0, 4.0, 0.0],
            kind: NavKind::Strip,
            distance: 4.0,
            fall: 3.45,
        };
        let t = Takeoff {
            at: [0.0, 2.9, 3.45],
            heading: [0.0, 1.0],
            half: 0.05,
            speed: 0.8,
            leave: Leave::Drop,
            checked: true,
        };
        assert_eq!(navpoint(Some(strip), Some(&t), false), Some(t.at));
        assert_eq!(navpoint(Some(strip), Some(&t), true), Some(strip.at));
        assert_eq!(navpoint(Some(strip), None, false), Some(strip.at));
        let crouch = NavTarget {
            kind: NavKind::Crouch,
            ..strip
        };
        assert_eq!(navpoint(Some(crouch), Some(&t), false), Some(crouch.at));
        assert_eq!(navpoint(None, Some(&t), false), None);
        // The agent's state carries the takeoff's place.
        let s = State {
            takeoff: Some(t),
            ..State::EMPTY
        };
        assert_eq!(
            reply(&s).takeoff,
            Some([0.0, 2.9f32 as f64, 3.45f32 as f64])
        );
    }
}
