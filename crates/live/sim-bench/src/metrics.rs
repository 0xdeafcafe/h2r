//! The metrics tally and the match report (docs/runtime/sim.md §6).
//!
//! Pure: the boot glue reads the game (a snapshot each tick, the shot, hit
//! and kill hooks, the bots' brains) and feeds plain values in here. Player
//! numbers are the game's player indices (the datum's low 16 bits).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::clock::SpeedMeter;
use crate::config::Detail;

/// The report's format tag.
pub const FORMAT: &str = "h2-sim-report/1";

/// One player as a tick's snapshot shows them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerSample {
    pub index: u8,
    pub name: String,
    pub team: Option<i32>,
    /// The unit they are in, if any (the datum).
    pub unit: Option<u32>,
    /// Their unit is alive.
    pub alive: bool,
    pub in_vehicle: bool,
    /// Grenades carried, when known.
    pub grenades: Option<u32>,
    pub score: i32,
    pub kills: i32,
    pub deaths: i32,
    pub assists: i32,
    pub suicides: i32,
}

/// The game after one tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickSample {
    /// The game's tick count.
    pub tick: u32,
    pub players: Vec<PlayerSample>,
    /// `(team, score)`.
    pub teams: Vec<(i32, i64)>,
}

/// What a bot's brain did in one game tick (the `Metered` wrapper).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BrainTick {
    /// The unstick escalation is past `Moving`.
    pub stuck: bool,
    /// Its repaths so far this life (a running count; the tally takes the
    /// rises).
    pub repaths: u32,
    /// What it is doing (`bots::mind::Doing`, lower case).
    pub doing: &'static str,
    /// What its hand saw of the target it is tracking, if any.
    pub aim: Option<AimSample>,
}

/// A bot's hand on one view of a tracked target (`bots::mind::AimTrace`),
/// with the right stick its pad reported on the same game tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimSample {
    /// The tick of the view. It trails the game's by the brain's
    /// perception delay, and repeats while the brain has no new view.
    pub tick: u32,
    /// Who it tracks.
    pub target: u32,
    /// From the aim to the aim point, yaw then pitch, radians, before the
    /// hand's own error.
    pub error: [f32; 2],
    pub stick: (f32, f32),
}

/// A first error this large (radians, 20 degrees) is a flick.
pub const AIM_FLICK: f32 = 0.35;
/// Settled: within this (radians) for [`AIM_SETTLE_HOLD`] views running.
pub const AIM_SETTLED: f32 = 0.05;
pub const AIM_SETTLE_HOLD: u32 = 6;
/// Overshoot is looked for this many ticks into a flick.
pub const AIM_OVERSHOOT_WINDOW: u32 = 60;
/// Views of one target this many ticks apart or less are one engagement.
pub const AIM_GAP: u32 = 3;
/// A stick this far over (of 1) has a direction, for the reversals.
pub const AIM_STICK_MOVING: f32 = 0.05;
/// Hits on one victim further apart than this (ticks) are separate
/// fights: the time to kill starts again.
pub const TTK_GAP: u32 = 150;

/// How a bot's aim moved over a match (docs/runtime/sim.md §6).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AimReport {
    /// Views of a tracked target.
    pub tracked_ticks: u64,
    /// The mean angle off the aim point while tracking, radians.
    pub mean_error: f64,
    /// Engagements that began at least [`AIM_FLICK`] off.
    pub flicks: u64,
    /// Over the flicks, the mean of how far past the target the aim went
    /// in the first two seconds, as a fraction of the flick.
    pub overshoot_ratio: f64,
    /// Flicks that settled, and the mean ticks they took to.
    pub settled: u64,
    pub settle_ticks: f64,
    /// The mean right-stick travel a tick while tracking, in stick radii.
    pub stick_jitter: f64,
    /// Changes of the stick's yaw direction a second while tracking.
    pub reversals_per_s: f64,
}

#[derive(Clone, Copy, Debug)]
struct Engagement {
    start: u32,
    first: [f32; 2],
    size: f32,
    flick: bool,
    /// The furthest past the target so far, radians.
    past: f32,
    /// Views in a row inside [`AIM_SETTLED`].
    calm: u32,
    settled: bool,
}

/// The aim-motion tally of one player.
#[derive(Clone, Debug, Default)]
struct AimMotion {
    last: Option<AimSample>,
    run: Option<Engagement>,
    tracked: u64,
    error: f64,
    pairs: u64,
    travel: f64,
    reversals: u64,
    /// The stick's last yaw direction in the engagement, 0 before any.
    turning: i8,
    flicks: u64,
    overshoot: f64,
    settled: u64,
    settle_ticks: u64,
}

impl AimMotion {
    fn close(&mut self) {
        if let Some(e) = self.run.take()
            && e.flick
        {
            self.overshoot += f64::from(e.past / e.size);
        }
    }

    fn sample(&mut self, s: AimSample) {
        if !(s.error[0].is_finite() && s.error[1].is_finite()) {
            return;
        }
        // The same view again: the brain did not think this tick.
        if self
            .last
            .is_some_and(|l| l.tick == s.tick && l.target == s.target)
        {
            return;
        }
        let same = self
            .last
            .is_some_and(|l| l.target == s.target && s.tick.wrapping_sub(l.tick) <= AIM_GAP);
        let size = s.error[0].hypot(s.error[1]);
        if !same || self.run.is_none() {
            self.close();
            let flick = size >= AIM_FLICK;
            if flick {
                self.flicks += 1;
            }
            self.run = Some(Engagement {
                start: s.tick,
                first: s.error,
                size,
                flick,
                past: 0.0,
                calm: 0,
                settled: false,
            });
            self.turning = 0;
        } else if let Some(l) = self.last {
            self.pairs += 1;
            self.travel += f64::from((s.stick.0 - l.stick.0).hypot(s.stick.1 - l.stick.1));
        }
        let turning = if s.stick.0 > AIM_STICK_MOVING {
            1
        } else if s.stick.0 < -AIM_STICK_MOVING {
            -1
        } else {
            0
        };
        if turning != 0 {
            if self.turning == -turning {
                self.reversals += 1;
            }
            self.turning = turning;
        }
        self.tracked += 1;
        self.error += f64::from(size);
        if let Some(e) = &mut self.run
            && e.flick
        {
            let age = s.tick.wrapping_sub(e.start);
            if age <= AIM_OVERSHOOT_WINDOW {
                let along = (s.error[0] * e.first[0] + s.error[1] * e.first[1]) / e.size;
                e.past = e.past.max(-along);
            }
            if !e.settled {
                if size < AIM_SETTLED {
                    e.calm += 1;
                    if e.calm == AIM_SETTLE_HOLD {
                        e.settled = true;
                        self.settled += 1;
                        // From the flick's first view to the first of the
                        // calm ones.
                        self.settle_ticks += u64::from(age.saturating_sub(AIM_SETTLE_HOLD - 1));
                    }
                } else {
                    e.calm = 0;
                }
            }
        }
        self.last = Some(s);
    }

    fn report(&self) -> Option<AimReport> {
        if self.tracked == 0 {
            return None;
        }
        let mut done = self.clone();
        done.close();
        let over = |sum: f64, n: u64| if n > 0 { sum / n as f64 } else { 0.0 };
        Some(AimReport {
            tracked_ticks: done.tracked,
            mean_error: over(done.error, done.tracked),
            flicks: done.flicks,
            overshoot_ratio: over(done.overshoot, done.flicks),
            settled: done.settled,
            settle_ticks: over(done.settle_ticks as f64, done.settled),
            stick_jitter: over(done.travel, done.pairs),
            reversals_per_s: over(done.reversals as f64 * 30.0, done.pairs),
        })
    }
}

/// How a death is classified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillKind {
    /// Someone else killed them.
    Kill,
    /// They killed themselves.
    Suicide,
    /// A teammate killed them.
    Betrayal,
    /// Nobody did (a fall, the map).
    World,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KillEvent {
    pub tick: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub killer: Option<u8>,
    pub victim: u8,
    pub kind: KillKind,
}

/// Why a match ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    /// The game's own end (score or time limit).
    GameOver,
    /// The match file's `max_ticks`.
    TickLimit,
    /// The match file's `max_wall_s`.
    WallLimit,
    /// `match.stop`.
    Stopped,
    /// The process stopped before any of the above.
    Aborted,
}

impl End {
    pub fn name(self) -> &'static str {
        match self {
            End::GameOver => "game_over",
            End::TickLimit => "tick_limit",
            End::WallLimit => "wall_limit",
            End::Stopped => "stopped",
            End::Aborted => "aborted",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Winner {
    Player(u8),
    Team(i32),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TeamResult {
    pub team: i32,
    pub score: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchResult {
    pub end: End,
    /// `None`: a tie, or nobody scored.
    pub winner: Option<Winner>,
    #[serde(default)]
    pub teams: Vec<TeamResult>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Wall {
    pub setup_s: f64,
    pub match_s: f64,
    pub process_s: f64,
    pub speedup: f64,
    pub process_speedup: f64,
    pub ms_per_tick: f64,
    /// The process's peak resident set, in MiB (`getrusage`'s `ru_maxrss`
    /// while the report is built; reports before the field carry none).
    #[serde(default)]
    pub peak_rss_mib: u64,
    /// The process's physical footprint on macOS, in MiB
    /// (`proc_pid_rusage`'s `ri_phys_footprint`): everything the process
    /// is charged for, clean pages included. `None` elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footprint_mib: Option<u64>,
    /// NtReadFile bytes supplied as private file mappings since boot.
    #[serde(default)]
    pub mapped_read_bytes: u64,
    /// NtReadFile bytes copied into guest RAM, including partial page edges.
    #[serde(default)]
    pub copied_read_bytes: u64,
    /// Guest RAM pages the match wrote over a shared copy-on-write base
    /// (`state.load` with `"cow": true`): the pages that stopped being
    /// shared. `None` without a base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_written_pages: Option<u64>,
    /// The base's guest RAM pages, `guest_written_pages`'s denominator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_pages: Option<u64>,
    /// Game-thread milliseconds per tick the bot brains cost: the read,
    /// think and conclude phases of every phantom's pad, serial or
    /// batched (a think on a worker is not counted; the wait for it is).
    /// `None` when the run did not measure it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bots_ms_per_tick: Option<f64>,
    /// Game-thread milliseconds per tick the phantoms' action build costs
    /// (`bot_glue::retail_input`): each seat's run through retail's local
    /// action builder in the private view, its six guest calls and the
    /// snapshot below included. Not in `bots_ms_per_tick`. `None` when the
    /// run did not measure it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_build_ms_per_tick: Option<f64>,
    /// The part of `phantom_build_ms_per_tick` spent taking the
    /// copy-on-write snapshots of guest RAM the build runs in
    /// (`SecondExecutor::snapshot`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_snapshot_ms_per_tick: Option<f64>,
    /// Those snapshots per tick: one a tick with phantoms, more when a
    /// failed build or a reset starts a seat from live RAM again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_snapshots_per_tick: Option<f64>,
    /// How the snapshots copied guest RAM (`runtime::CopyKind`): `remap`
    /// (a copy-on-write mapping of the same pages), `copy` (every byte),
    /// or `mixed`. `None` with no snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_snapshot_copy: Option<String>,
    /// Check mode (`H2_PHANTOM_BUILD_CHECK=1`): the seats whose action
    /// build on the alias path and on the view path differed in outcome,
    /// action bytes or history. `None` when no seat was checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_build_mismatches: Option<u64>,
    /// Nanoseconds per tick in the build's setup writes (the view's
    /// globals, the pad and the seat's history).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_setup_ns_per_tick: Option<f64>,
    /// Nanoseconds per tick in each of the six guest calls, by role
    /// ([`PHANTOM_CALLS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_call_ns_per_tick: Option<BTreeMap<String, f64>>,
    /// Guest accesses per tick of each call (`CallOutcome::accesses`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_call_accesses_per_tick: Option<BTreeMap<String, f64>>,
    /// Nanoseconds per tick in `written` plus `revert` between seats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_revert_ns_per_tick: Option<f64>,
    /// Nanoseconds per tick reading the action and history back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_read_ns_per_tick: Option<f64>,
    /// Pages a seat wrote, as a mean over the seats that were reverted
    /// (a tick's last seat is not).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_pages_written_per_seat: Option<f64>,
    /// Minor page faults per tick in `Builder::sample`: the process's
    /// `getrusage` `ru_minflt` delta, so other threads' faults count too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_minor_faults_per_tick: Option<f64>,
    /// The census and lean-report fields ([`WallLean`]).
    #[serde(flatten)]
    pub lean: WallLean,
}

/// The six guest calls of a build, in call order: `0x1872d0`,
/// `0x190c34`, `0x1557c0`, `0x124c00`, `0x217c40`, `0x186ab0`.
pub const PHANTOM_CALLS: [&str; 6] = [
    "bind",
    "look_rates",
    "camera",
    "input",
    "remap",
    "builder",
];

/// The phantoms' action build on the game thread, as running totals
/// (`bot_glue::retail_input`): [`Report::set_phantom_costs`] takes the
/// difference over the match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhantomCosts {
    /// Nanoseconds in the build, snapshots included.
    pub build_ns: u64,
    /// Nanoseconds in the snapshots.
    pub snapshot_ns: u64,
    /// Snapshots that remapped guest RAM.
    pub remapped: u64,
    /// Snapshots that copied its bytes.
    pub copied: u64,
    /// Seats built on both paths in check mode.
    pub checked: u64,
    /// Of those, the seats whose outcome, action or history differed.
    pub mismatches: u64,
    /// Nanoseconds in the setup writes.
    pub setup_ns: u64,
    /// Nanoseconds in each guest call ([`PHANTOM_CALLS`] order).
    pub call_ns: [u64; 6],
    /// Guest accesses of each call.
    pub call_accesses: [u64; 6],
    /// Nanoseconds in `written` plus `revert`.
    pub revert_ns: u64,
    /// Nanoseconds reading the action and history back.
    pub read_ns: u64,
    /// Pages written, summed over the reverted seats.
    pub pages_written: u64,
    /// Seats whose pages were counted.
    pub pages_seats: u64,
    /// Minor faults in `Builder::sample`.
    pub minor_faults: u64,
}

fn sub6(a: [u64; 6], b: [u64; 6]) -> [u64; 6] {
    std::array::from_fn(|i| a[i].saturating_sub(b[i]))
}

impl PhantomCosts {
    /// What was added since `start`.
    pub fn since(self, start: PhantomCosts) -> PhantomCosts {
        PhantomCosts {
            build_ns: self.build_ns.saturating_sub(start.build_ns),
            snapshot_ns: self.snapshot_ns.saturating_sub(start.snapshot_ns),
            remapped: self.remapped.saturating_sub(start.remapped),
            copied: self.copied.saturating_sub(start.copied),
            checked: self.checked.saturating_sub(start.checked),
            mismatches: self.mismatches.saturating_sub(start.mismatches),
            setup_ns: self.setup_ns.saturating_sub(start.setup_ns),
            call_ns: sub6(self.call_ns, start.call_ns),
            call_accesses: sub6(self.call_accesses, start.call_accesses),
            revert_ns: self.revert_ns.saturating_sub(start.revert_ns),
            read_ns: self.read_ns.saturating_sub(start.read_ns),
            pages_written: self.pages_written.saturating_sub(start.pages_written),
            pages_seats: self.pages_seats.saturating_sub(start.pages_seats),
            minor_faults: self.minor_faults.saturating_sub(start.minor_faults),
        }
    }

    /// [`Wall::phantom_snapshot_copy`]'s value.
    pub fn copy_kind(self) -> Option<&'static str> {
        match (self.remapped, self.copied) {
            (0, 0) => None,
            (_, 0) => Some("remap"),
            (0, _) => Some("copy"),
            _ => Some("mixed"),
        }
    }
}

/// The guest arrays the census counts (docs/runtime/sim.md §6,
/// `guest_census`), by their data-array names; [`CensusSample::arrays`]
/// is in this order.
pub const CENSUS_ARRAYS: [&str; 5] = ["effect", "particle_system", "particles", "decals", "lights"];

/// One tick's read of the guest's presentation-side state: what a
/// headless match still holds that nobody looks at
/// (docs/perf/sim-headless-skips.md). `None` where the read failed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CensusSample {
    /// The local-user count.
    pub local_users: Option<u32>,
    /// The local players' visibility set is empty.
    pub local_visibility_empty: Option<bool>,
    /// Live elements of each of [`CENSUS_ARRAYS`].
    pub arrays: [Option<u32>; CENSUS_ARRAYS.len()],
}

impl CensusSample {
    /// The sample as `sim.guest_census` answers it: unread fields are
    /// `null`.
    pub fn to_json(&self) -> serde_json::Value {
        let arrays: serde_json::Map<String, serde_json::Value> = CENSUS_ARRAYS
            .iter()
            .zip(self.arrays)
            .map(|(name, count)| ((*name).to_owned(), serde_json::json!(count)))
            .collect();
        serde_json::json!({
            "local_users": self.local_users,
            "local_visibility_empty": self.local_visibility_empty,
            "arrays": arrays,
        })
    }
}

/// A count over the ticks it was read on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CountStat {
    /// Ticks it was read on.
    pub samples: u64,
    pub mean: f64,
    pub peak: u32,
}

/// The local visibility set over the ticks it was read on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct VisibilityStat {
    /// Ticks it was read on.
    pub samples: u64,
    /// Of those, the ticks it was empty.
    pub empty_ticks: u64,
}

/// The report's `guest_census` block: [`CensusSample`]s over the match.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GuestCensus {
    /// Ticks sampled.
    pub ticks: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_users: Option<CountStat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_visibility: Option<VisibilityStat>,
    /// By [`CENSUS_ARRAYS`] name; an array never read is left out.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub arrays: BTreeMap<String, CountStat>,
}

#[derive(Clone, Copy, Debug, Default)]
struct Count {
    samples: u64,
    sum: u64,
    peak: u32,
}

impl Count {
    fn add(&mut self, v: Option<u32>) {
        if let Some(v) = v {
            self.samples += 1;
            self.sum += u64::from(v);
            self.peak = self.peak.max(v);
        }
    }

    fn stat(&self) -> Option<CountStat> {
        (self.samples > 0).then(|| CountStat {
            samples: self.samples,
            mean: self.sum as f64 / self.samples as f64,
            peak: self.peak,
        })
    }
}

#[derive(Clone, Debug, Default)]
struct CensusTally {
    last_tick: Option<u32>,
    ticks: u64,
    local_users: Count,
    visibility: VisibilityStat,
    arrays: [Count; CENSUS_ARRAYS.len()],
}

impl CensusTally {
    fn add(&mut self, tick: u32, s: &CensusSample) {
        if self.last_tick == Some(tick) {
            return;
        }
        self.last_tick = Some(tick);
        self.ticks += 1;
        self.local_users.add(s.local_users);
        if let Some(empty) = s.local_visibility_empty {
            self.visibility.samples += 1;
            self.visibility.empty_ticks += u64::from(empty);
        }
        for (count, v) in self.arrays.iter_mut().zip(s.arrays) {
            count.add(v);
        }
    }

    fn report(&self) -> Option<GuestCensus> {
        (self.ticks > 0).then(|| GuestCensus {
            ticks: self.ticks,
            local_users: self.local_users.stat(),
            local_visibility: (self.visibility.samples > 0).then(|| self.visibility.clone()),
            arrays: CENSUS_ARRAYS
                .iter()
                .zip(&self.arrays)
                .filter_map(|(name, c)| Some(((*name).to_owned(), c.stat()?)))
                .collect(),
        })
    }
}

/// The process's memory, sampled while a report is built: [`Wall`]'s
/// memory fields. Peak RSS everywhere; the footprint where the host
/// has one (macOS); the guest census when a shared base was loaded.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MemoryUsage {
    pub mapped_read_bytes: u64,
    pub copied_read_bytes: u64,
    pub peak_rss_mib: u64,
    pub footprint_mib: Option<u64>,
    pub guest_written_pages: Option<u64>,
    pub guest_pages: Option<u64>,
}

pub use live_proto::game::WeaponAccuracy;

/// Flat RAM window counters over the match (`runtime::WindowStats`), per
/// game tick.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FlatReport {
    /// Faulting guest accesses a fixup sent to the slow path, per tick.
    pub fixups_per_tick: f64,
}

/// One player's line in the report.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<u8>,
    pub index: u8,
    pub name: String,
    /// A bot of ours.
    pub bot: bool,
    /// The batch arm the bot played for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i32>,
    pub score: i32,
    pub kills: i32,
    pub deaths: i32,
    pub assists: i32,
    pub suicides: i32,
    pub betrayals: u32,
    pub shots: u64,
    /// Shots' damage reports on another player's unit.
    pub hits: u64,
    /// `hits / shots` (0 with no shots). A shotgun's pellets, a
    /// grenade's splash and melee count as hits too, so it can pass 1.
    pub accuracy: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub weapon_accuracy: Vec<WeaponAccuracy>,
    /// Phantom brain diagnostics in the same shape as the bots.stats response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combat: Option<serde_json::Value>,
    pub damage_dealt: f64,
    pub damage_taken: f64,
    pub alive_ticks: u64,
    pub vehicle_ticks: u64,
    /// Grenades thrown: the carried count falling within a life.
    pub grenades: u64,
    pub stuck_ticks: u64,
    pub repaths: u64,
    /// Ticks in an objective activity (carry, keep, play).
    pub objective_ticks: u64,
    /// Ticks per activity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub doing: BTreeMap<String, u64>,
    /// Kills with a time to kill: the killer had hurt the victim first.
    #[serde(default)]
    pub ttk_kills: u64,
    /// Over those, the mean ticks from the killer's first hit of the
    /// fight on the victim to the kill.
    #[serde(default)]
    pub ttk_ticks: f64,
    /// How the bot's aim moved while it tracked a target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<AimReport>,
}

/// One post-tick observation. Hex strings preserve all 64 bits in JSON clients.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickHash {
    pub tick: u32,
    pub gameplay: String,
    /// Present when arena verification was enabled and the arena was readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arena: Option<String>,
}

/// The whole report: one JSON file per match.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub format: String,
    /// The match as played (`config::Match::resolved_json`).
    #[serde(rename = "match")]
    pub match_: serde_json::Value,
    pub result: MatchResult,
    pub ticks: u64,
    /// Gameplay digests in guest tick order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub state_hashes_v1: Vec<TickHash>,
    /// The per-component breakdown behind those digests, as deltas
    /// (`h2 sim diff`, [`crate::diff`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_components_v1: Option<crate::diff::StateComponents>,
    /// A training-detail report's rolling digest over every tick
    /// ([`crate::config::Detail`], [`crate::diff::StateDigest`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_digest_v1: Option<crate::diff::StateDigest>,
    pub game_seconds: f64,
    pub wall: Wall,
    pub players: Vec<PlayerReport>,
    pub kills: Vec<KillEvent>,
    /// The bots' nav mesh: `loaded` or `missing`.
    pub nav: String,
    /// Runner dispatch and preemption census when enabled for this match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_per_tick: Option<BTreeMap<String, f64>>,
    /// The flat RAM window's fixups per tick; `None` without a window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flat: Option<FlatReport>,
    /// The guest census over the match (docs/runtime/sim.md §6); `None`
    /// with no tick sampled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_census: Option<GuestCensus>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Report {
    /// Fills the per-tick costs the run measured outside the tally:
    /// `bots_ns` nanoseconds of game-thread bot work and `fixups` flat
    /// window fixups over the match's [`Report::ticks`]. Nothing is set
    /// for a match with no ticks.
    pub fn set_costs(&mut self, bots_ns: u64, fixups: Option<u64>) {
        if self.ticks == 0 {
            return;
        }
        let ticks = self.ticks as f64;
        self.wall.bots_ms_per_tick = Some(bots_ns as f64 / 1e6 / ticks);
        self.flat = fixups.map(|f| FlatReport {
            fixups_per_tick: f as f64 / ticks,
        });
    }

    /// Fills the phantoms' action-build costs over the match's
    /// [`Report::ticks`] (`costs`: the difference of two
    /// [`PhantomCosts`] totals). Nothing is set for a match with no ticks.
    pub fn set_phantom_costs(&mut self, costs: PhantomCosts) {
        if self.ticks == 0 {
            return;
        }
        let ticks = self.ticks as f64;
        self.wall.phantom_build_ms_per_tick = Some(costs.build_ns as f64 / 1e6 / ticks);
        self.wall.phantom_snapshot_ms_per_tick = Some(costs.snapshot_ns as f64 / 1e6 / ticks);
        self.wall.phantom_snapshots_per_tick = Some((costs.remapped + costs.copied) as f64 / ticks);
        self.wall.phantom_snapshot_copy = costs.copy_kind().map(str::to_owned);
        self.wall.phantom_build_mismatches = (costs.checked > 0).then_some(costs.mismatches);
        let per_call = |v: [u64; 6]| {
            PHANTOM_CALLS
                .iter()
                .zip(v)
                .map(|(name, n)| ((*name).to_owned(), n as f64 / ticks))
                .collect::<BTreeMap<_, _>>()
        };
        self.wall.phantom_setup_ns_per_tick = Some(costs.setup_ns as f64 / ticks);
        self.wall.phantom_call_ns_per_tick = Some(per_call(costs.call_ns));
        self.wall.phantom_call_accesses_per_tick = Some(per_call(costs.call_accesses));
        self.wall.phantom_revert_ns_per_tick = Some(costs.revert_ns as f64 / ticks);
        self.wall.phantom_read_ns_per_tick = Some(costs.read_ns as f64 / ticks);
        self.wall.phantom_pages_written_per_seat =
            (costs.pages_seats > 0).then(|| costs.pages_written as f64 / costs.pages_seats as f64);
        self.wall.phantom_minor_faults_per_tick = Some(costs.minor_faults as f64 / ticks);
    }

    pub fn player(&self, index: u8) -> Option<&PlayerReport> {
        self.players.iter().find(|p| p.index == index)
    }
}

/// A bot as the tally knows it: a phantom peer, found in the game by the
/// player it became.
#[derive(Clone, Debug, PartialEq)]
pub struct BotInfo {
    pub seat: u8,
    pub player: Option<u8>,
    pub name: String,
    pub arm: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct Acc {
    shots: u64,
    hits: u64,
    damage_dealt: f64,
    damage_taken: f64,
    alive_ticks: u64,
    vehicle_ticks: u64,
    grenades: u64,
    betrayals: u32,
    stuck_ticks: u64,
    repaths: u64,
    objective_ticks: u64,
    doing: BTreeMap<&'static str, u64>,
    /// `(unit, grenades)` last tick, for the throws.
    held: Option<(u32, u32)>,
    /// The brain's repath count last tick.
    last_repaths: u32,
    ttk_kills: u64,
    ttk_ticks: u64,
    aim: AimMotion,
}

/// A tick this far past the last one is a snapshot load, not play.
pub const JUMP_TICKS: u32 = 30;

/// The objective activities (`bots::mind::Doing`).
pub const OBJECTIVE_DOINGS: [&str; 3] = ["carry", "keep", "play"];

/// What the report needs beyond the tally.
#[derive(Clone, Debug)]
pub struct Finish<'a> {
    /// The match as played (`config::Match::resolved_json`).
    pub match_: serde_json::Value,
    pub team_play: bool,
    pub bots: &'a [BotInfo],
    pub end: End,
    /// Its `ticks` are ignored: the tally's count is used.
    pub speed: SpeedMeter,
    /// The process's memory, as the report's `wall` block carries it.
    pub memory: MemoryUsage,
    /// `loaded` or `missing`.
    pub nav: &'a str,
    pub notes: Vec<String>,
}

impl Finish<'_> {
    /// A finish with nothing but the end (tests, the agent's early report).
    pub fn bare(end: End) -> Finish<'static> {
        Finish {
            match_: serde_json::Value::Null,
            team_play: false,
            bots: &[],
            end,
            speed: SpeedMeter::default(),
            memory: MemoryUsage::default(),
            nav: "loaded",
            notes: Vec::new(),
        }
    }
}

/// The part of the tally which controls tick limits and detects a rewind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickCursor {
    started: Option<u32>,
    last_tick: Option<u32>,
    ticks: u64,
    resets: u32,
}

/// Aggregates a match.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    started: Option<u32>,
    last_tick: Option<u32>,
    ticks: u64,
    acc: BTreeMap<u8, Acc>,
    kills: Vec<KillEvent>,
    last: Option<TickSample>,
    hashes: Vec<(u32, u64, Option<u64>)>,
    components: crate::diff::Recorder,
    /// How much state the run records ([`Detail`]).
    detail: Detail,
    rolling: Rolling,
    resets: u32,
    /// `(attacker, victim)` → the first and the latest tick of the fight's
    /// hits, for the time to kill.
    fights: BTreeMap<(u8, u8), (u32, u32)>,
    census: CensusTally,
}

impl Tally {
    pub fn new() -> Tally {
        Tally::default()
    }

    /// The game started (or restarted): everything so far is dropped.
    pub fn start(&mut self, tick: u32) {
        let resets = self.resets;
        let detail = self.detail;
        *self = Tally::default();
        self.resets = resets;
        self.detail = detail;
        self.started = Some(tick);
    }

    pub fn started(&self) -> bool {
        self.started.is_some()
    }

    /// Game ticks counted since the start.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    pub fn cursor(&self) -> TickCursor {
        TickCursor {
            started: self.started,
            last_tick: self.last_tick,
            ticks: self.ticks,
            resets: self.resets,
        }
    }

    /// Continue a film's tick-limit and discontinuity decisions from the
    /// recorded boundary. Theater does not produce a training report.
    pub fn resume_playback(&mut self, cursor: TickCursor) -> Result<(), String> {
        let elapsed = cursor
            .last_tick
            .zip(cursor.started)
            .and_then(|(last, start)| last.checked_sub(start));
        if elapsed.is_none_or(|ticks| cursor.ticks == 0 || cursor.ticks > u64::from(ticks) + 1)
            || cursor.resets == u32::MAX
        {
            return Err("invalid sim keyframe clock".into());
        }
        *self = Self {
            started: cursor.started,
            last_tick: cursor.last_tick,
            ticks: cursor.ticks,
            resets: cursor.resets,
            detail: self.detail,
            ..Default::default()
        };
        Ok(())
    }

    /// How many times the tick went backwards (a snapshot load) and the
    /// tally started again.
    pub fn resets(&self) -> u32 {
        self.resets
    }

    pub fn last(&self) -> Option<&TickSample> {
        self.last.as_ref()
    }

    /// After a game tick. Ticks move one at a time; a tick before the last
    /// one, or more than [`JUMP_TICKS`] after it, means the machine was
    /// loaded from a snapshot: the tally starts again from there.
    pub fn tick(&mut self, s: &TickSample) {
        self.tick_owned(s.clone());
    }

    /// Record the sample and retain its player names and vectors for the
    /// next tick's comparisons.
    pub fn tick_owned(&mut self, s: TickSample) {
        if let Some(last) = self.last_tick
            && (s.tick < last || s.tick > last.saturating_add(JUMP_TICKS))
        {
            self.resets += 1;
            self.start(s.tick);
        }
        if self.started.is_none() {
            self.started = Some(s.tick);
        }
        if self.last_tick == Some(s.tick) {
            // The same tick again (an observer called twice): the latest
            // view, counted once.
            self.last = Some(s);
            return;
        }
        self.ticks += 1;
        self.last_tick = Some(s.tick);
        for p in &s.players {
            let a = self.acc.entry(p.index).or_default();
            if p.alive {
                a.alive_ticks += 1;
                if p.in_vehicle {
                    a.vehicle_ticks += 1;
                }
            }
            match (p.unit, p.grenades) {
                (Some(u), Some(g)) if p.alive => {
                    if let Some((lu, lg)) = a.held
                        && lu == u
                        && g < lg
                    {
                        a.grenades += u64::from(lg - g);
                    }
                    a.held = Some((u, g));
                }
                _ => a.held = None,
            }
        }
        self.last = Some(s);
    }

    /// Record the post-tick digest on the same timeline as the tally. The
    /// latest observation wins for a duplicate tick; a snapshot load resets both.
    pub fn tick_hashed(&mut self, sample: TickSample, gameplay: u64, arena: Option<u64>) {
        let tick = sample.tick;
        self.tick_owned(sample);
        let entry = (tick, gameplay, arena);
        if self.detail == Detail::Training {
            self.rolling.observe(entry);
            // Sparse rows: the first, then every `TRAINING_STRIDE`th tick.
            let sampled = self.hashes.is_empty() || tick % TRAINING_STRIDE == 0;
            if let Some(last) = self.hashes.last_mut()
                && last.0 == tick
            {
                *last = entry;
            } else if sampled {
                self.hashes.push(entry);
            }
            return;
        }
        if let Some(last) = self.hashes.last_mut()
            && last.0 == tick
        {
            *last = entry;
        } else {
            self.hashes.push(entry);
        }
    }

    /// How much state this run records. Set it before the first tick; it
    /// survives [`Tally::start`].
    pub fn set_detail(&mut self, detail: Detail) {
        self.detail = detail;
    }

    pub fn detail(&self) -> Detail {
        self.detail
    }

    /// Record the per-component state for the tick just given to
    /// [`Tally::tick_hashed`]; same timeline, same resets.
    pub fn tick_components(
        &mut self,
        tick: u32,
        arena_base: Option<u32>,
        components: impl IntoIterator<Item = (String, String)>,
    ) {
        if self.detail == Detail::Full {
            self.components.record(tick, arena_base, components);
        }
    }

    /// [`Tally::tick_components`] with the arena's block digests as `u64`s
    /// (`arena.00`, `arena.01`, ...): no text is made for an unchanged one.
    pub fn tick_components_blocks(
        &mut self,
        tick: u32,
        arena_base: Option<u32>,
        components: impl IntoIterator<Item = (String, String)>,
        arena_blocks: &[u64],
    ) {
        if self.detail == Detail::Full {
            self.components
                .record_split(tick, arena_base, components, arena_blocks);
        }
    }

    /// The guest census read after game tick `tick` (the tick just given
    /// to [`Tally::tick_hashed`]); a second read of one tick is not
    /// counted. Same timeline, same resets.
    pub fn tick_census(&mut self, tick: u32, sample: &CensusSample) {
        self.census.add(tick, sample);
    }

    /// The census so far, as the report carries it.
    pub fn census(&self) -> Option<GuestCensus> {
        self.census.report()
    }

    /// A shot by `player`.
    pub fn shot(&mut self, player: u8) {
        self.acc.entry(player).or_default().shots += 1;
    }

    /// A damage report: `attacker` (if a player) hurt `victim` (if a
    /// player) by `damage` (body plus shield, in the game's units).
    pub fn hit(&mut self, attacker: Option<u8>, victim: Option<u8>, damage: f32, team_play: bool) {
        let damage = if damage.is_finite() {
            f64::from(damage.max(0.0))
        } else {
            0.0
        };
        if let Some(v) = victim {
            self.acc.entry(v).or_default().damage_taken += damage;
        }
        if let Some(a) = attacker
            && victim.is_some_and(|v| v != a)
        {
            let team = |who| {
                self.last
                    .as_ref()?
                    .players
                    .iter()
                    .find(|p| Some(p.index) == who)?
                    .team
            };
            let opposing = !team_play
                || team(attacker)
                    .zip(team(victim))
                    .is_some_and(|(a, b)| a != b);
            if opposing
                && damage > 0.0
                && let Some(v) = victim
            {
                let now = self.last_tick.map_or(0, |t| t.wrapping_add(1));
                let fight = self.fights.entry((a, v)).or_insert((now, now));
                if now.wrapping_sub(fight.1) > TTK_GAP {
                    fight.0 = now;
                }
                fight.1 = now;
            }
            let acc = self.acc.entry(a).or_default();
            acc.hits += 1;
            acc.damage_dealt += damage;
        }
    }

    /// A death, as `PLAYER_KILLED` gives it.
    pub fn kill(&mut self, tick: u32, killer: Option<u8>, victim: u8, betrayal: bool) {
        let kind = match killer {
            None => KillKind::World,
            Some(k) if k == victim => KillKind::Suicide,
            Some(_) if betrayal => KillKind::Betrayal,
            Some(_) => KillKind::Kill,
        };
        if kind == KillKind::Betrayal
            && let Some(k) = killer
        {
            self.acc.entry(k).or_default().betrayals += 1;
        }
        if kind == KillKind::Kill
            && let Some(k) = killer
            && let Some(&(first, _)) = self.fights.get(&(k, victim))
        {
            // The hits are stamped a tick after the last snapshot; so is
            // a kill on the same tick.
            let now = self.last_tick.map_or(tick, |t| t.wrapping_add(1));
            let a = self.acc.entry(k).or_default();
            a.ttk_kills += 1;
            a.ttk_ticks += u64::from(now.wrapping_sub(first).min(1 << 20));
        }
        // Whoever was fighting the victim starts again on its next life.
        self.fights.retain(|&(_, v), _| v != victim);
        self.kills.push(KillEvent {
            tick,
            killer,
            victim,
            kind,
        });
    }

    /// One game tick of the brain of the bot that became `player`.
    pub fn brain_for_player(&mut self, player: u8, b: BrainTick) {
        let a = self.acc.entry(player).or_default();
        if b.stuck {
            a.stuck_ticks += 1;
        }
        if b.repaths > a.last_repaths {
            a.repaths += u64::from(b.repaths - a.last_repaths);
        }
        a.last_repaths = b.repaths;
        if let Some(s) = b.aim {
            a.aim.sample(s);
        }
        if !b.doing.is_empty() {
            *a.doing.entry(b.doing).or_default() += 1;
            if OBJECTIVE_DOINGS.contains(&b.doing) {
                a.objective_ticks += 1;
            }
        }
    }

    pub fn kills(&self) -> &[KillEvent] {
        &self.kills
    }

    /// Who is winning on the last snapshot: the top team in a team game,
    /// else the top player; `None` on a tie or with no scores.
    pub fn leader(&self, team_play: bool) -> Option<Winner> {
        let last = self.last.as_ref()?;
        if team_play && !last.teams.is_empty() {
            let best = last.teams.iter().map(|t| t.1).max()?;
            let mut top = last.teams.iter().filter(|t| t.1 == best);
            let first = top.next()?;
            return if top.next().is_some() {
                None
            } else {
                Some(Winner::Team(first.0))
            };
        }
        let best = last.players.iter().map(|p| p.score).max()?;
        let mut top = last.players.iter().filter(|p| p.score == best);
        let first = top.next()?;
        if top.next().is_some() {
            None
        } else {
            Some(Winner::Player(first.index))
        }
    }

    /// The report as it stands.
    pub fn report(&self, f: Finish<'_>) -> Report {
        let Finish {
            match_,
            team_play,
            bots,
            end,
            speed,
            memory,
            nav,
            notes,
        } = f;
        let mut players = Vec::new();
        let empty = TickSample::default();
        let last = self.last.as_ref().unwrap_or(&empty);
        let mut indices: Vec<u8> = last.players.iter().map(|p| p.index).collect();
        for i in self.acc.keys() {
            if !indices.contains(i) {
                indices.push(*i);
            }
        }
        indices.sort_unstable();
        for i in indices {
            let sample = last.players.iter().find(|p| p.index == i);
            let a = self.acc.get(&i).cloned().unwrap_or_default();
            let bot = bots.iter().find(|b| b.player == Some(i));
            let shots = a.shots;
            players.push(PlayerReport {
                seat: bot.map(|b| b.seat),
                index: i,
                name: sample
                    .map(|p| p.name.clone())
                    .filter(|n| !n.is_empty())
                    .or_else(|| bot.map(|b| b.name.clone()))
                    .unwrap_or_else(|| format!("player {i}")),
                bot: bot.is_some(),
                arm: bot.and_then(|b| b.arm.clone()),
                team: sample.and_then(|p| p.team),
                score: sample.map_or(0, |p| p.score),
                kills: sample.map_or(0, |p| p.kills),
                deaths: sample.map_or(0, |p| p.deaths),
                assists: sample.map_or(0, |p| p.assists),
                suicides: sample.map_or(0, |p| p.suicides),
                betrayals: a.betrayals,
                shots,
                hits: a.hits,
                accuracy: if shots > 0 {
                    a.hits as f64 / shots as f64
                } else {
                    0.0
                },
                weapon_accuracy: Vec::new(),
                combat: None,
                damage_dealt: a.damage_dealt,
                damage_taken: a.damage_taken,
                alive_ticks: a.alive_ticks,
                vehicle_ticks: a.vehicle_ticks,
                grenades: a.grenades,
                stuck_ticks: a.stuck_ticks,
                repaths: a.repaths,
                objective_ticks: a.objective_ticks,
                doing: a.doing.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
                ttk_kills: a.ttk_kills,
                ttk_ticks: if a.ttk_kills > 0 {
                    a.ttk_ticks as f64 / a.ttk_kills as f64
                } else {
                    0.0
                },
                aim: a.aim.report(),
            });
        }
        let teams = last
            .teams
            .iter()
            .map(|&(team, score)| TeamResult { team, score })
            .collect();
        let mut speed = speed;
        speed.ticks = self.ticks;
        let mut notes = notes;
        if self.resets > 0 {
            notes.push(format!(
                "the tally started again {} time(s): the tick jumped (a snapshot load)",
                self.resets
            ));
        }
        Report {
            format: FORMAT.into(),
            match_,
            result: MatchResult {
                end,
                winner: self.leader(team_play),
                teams,
            },
            ticks: self.ticks,
            state_hashes_v1: self
                .hashes
                .iter()
                .map(|&(tick, gameplay, arena)| TickHash {
                    tick,
                    gameplay: format!("{gameplay:016x}"),
                    arena: arena.map(|h| format!("{h:016x}")),
                })
                .collect(),
            state_components_v1: self.components.finish(),
            state_digest_v1: self.rolling.report(),
            game_seconds: speed.game_seconds(),
            wall: Wall {
                setup_s: speed.setup_s,
                match_s: speed.match_s,
                process_s: speed.setup_s + speed.match_s,
                speedup: speed.speedup(),
                process_speedup: speed.process_speedup(),
                ms_per_tick: speed.ms_per_tick(),
                mapped_read_bytes: memory.mapped_read_bytes,
                copied_read_bytes: memory.copied_read_bytes,
                peak_rss_mib: memory.peak_rss_mib,
                footprint_mib: memory.footprint_mib,
                guest_written_pages: memory.guest_written_pages,
                guest_pages: memory.guest_pages,
                bots_ms_per_tick: None,
                phantom_build_ms_per_tick: None,
                phantom_snapshot_ms_per_tick: None,
                phantom_snapshots_per_tick: None,
                phantom_snapshot_copy: None,
                phantom_build_mismatches: None,
                phantom_setup_ns_per_tick: None,
                phantom_call_ns_per_tick: None,
                phantom_call_accesses_per_tick: None,
                phantom_revert_ns_per_tick: None,
                phantom_read_ns_per_tick: None,
                phantom_pages_written_per_seat: None,
                phantom_minor_faults_per_tick: None,
                lean: WallLean::default(),
            },
            players,
            kills: self.kills.clone(),
            nav: nav.into(),
            dispatch_per_tick: None,
            flat: None,
            guest_census: self.census(),
            notes,
        }
    }
}

// ---- census and lean reports (docs/perf/sim-light.md, lane L1) ----

/// In a [`Detail::Training`] run, one `state_hashes_v1` row in this many
/// ticks (one a game second), and the first.
pub const TRAINING_STRIDE: u32 = 30;

fn fmix(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// A digest rolled over every tick's `(tick, gameplay, arena)` digests in
/// order. A tick observed again replaces itself (as `state_hashes_v1` does),
/// so the latest observation waits in `pending` until the next tick folds it.
#[derive(Clone, Debug, Default)]
struct Rolling {
    acc: u64,
    folded: u64,
    pending: Option<(u32, u64, Option<u64>)>,
}

fn fold(acc: u64, (tick, gameplay, arena): (u32, u64, Option<u64>)) -> u64 {
    let h = fmix(acc ^ u64::from(tick));
    let h = fmix(h ^ gameplay);
    fmix(h ^ arena.unwrap_or(0).rotate_left(1) ^ u64::from(arena.is_some()))
}

impl Rolling {
    fn observe(&mut self, entry: (u32, u64, Option<u64>)) {
        match self.pending {
            Some(p) if p.0 != entry.0 => {
                self.acc = fold(self.acc, p);
                self.folded += 1;
                self.pending = Some(entry);
            }
            _ => self.pending = Some(entry),
        }
    }

    fn report(&self) -> Option<crate::diff::StateDigest> {
        let last = self.pending?;
        Some(crate::diff::StateDigest {
            stride: TRAINING_STRIDE,
            ticks: self.folded + 1,
            digest: format!("{:016x}", fold(self.acc, last)),
        })
    }
}

/// Seconds spent in each stage of getting from the process start to the
/// first counted tick (`wall.setup`); they add up to `wall.setup_s` once the
/// match is counting. A stage a run never goes through reads 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SetupPhases {
    /// Process start to the first game tick at the main menu.
    pub boot_s: f64,
    /// The main menu to the lobby being able to open (networking, lobby
    /// subsystem).
    pub menu_s: f64,
    /// Opening and filling the System Link squad, up to the first wait on a
    /// peer's map copy.
    pub lobby_s: f64,
    /// From the first wait on a peer's map status to the start request
    /// being accepted.
    pub copy_wait_s: f64,
    /// The start request to the game activating (the map load).
    pub load_s: f64,
    /// The game active to the first counted tick: every phantom has a
    /// player and a unit.
    pub bind_s: f64,
}

/// A stage of [`SetupPhases`], in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SetupStage {
    Boot,
    Menu,
    Lobby,
    CopyWait,
    Load,
    Bind,
    Done,
}

/// Times the stages: [`SetupClock::enter`] moves forward only, and charges
/// the time since the last move to the stage it leaves.
#[derive(Clone, Copy, Debug)]
pub struct SetupClock {
    stage: SetupStage,
    since: std::time::Instant,
    spent: SetupPhases,
}

impl SetupClock {
    /// In [`SetupStage::Boot`] from `start`.
    pub fn new(start: std::time::Instant) -> SetupClock {
        SetupClock {
            stage: SetupStage::Boot,
            since: start,
            spent: SetupPhases::default(),
        }
    }

    fn charge(&mut self, now: std::time::Instant) {
        let s = now.saturating_duration_since(self.since).as_secs_f64();
        match self.stage {
            SetupStage::Boot => self.spent.boot_s += s,
            SetupStage::Menu => self.spent.menu_s += s,
            SetupStage::Lobby => self.spent.lobby_s += s,
            SetupStage::CopyWait => self.spent.copy_wait_s += s,
            SetupStage::Load => self.spent.load_s += s,
            SetupStage::Bind => self.spent.bind_s += s,
            SetupStage::Done => {}
        }
        self.since = now;
    }

    /// Start `stage` at `now`. Going back, or to the stage already in, does
    /// nothing.
    pub fn enter(&mut self, stage: SetupStage, now: std::time::Instant) {
        if stage > self.stage {
            self.charge(now);
            self.stage = stage;
        }
    }

    /// The time in each stage, the running one counted to `now`.
    pub fn phases(&self, now: std::time::Instant) -> SetupPhases {
        let mut c = *self;
        c.charge(now);
        c.spent
    }
}

/// One path's reads in `wall.reads`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadStat {
    pub mapped_bytes: u64,
    pub copied_bytes: u64,
    /// Reads served (a copy of one request in chunks counts each chunk).
    pub count: u64,
    /// Why the first mapped read of this path was refused, when one was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// The `wall` fields of the census and the lean reports. Flattened into
/// [`Wall`], so they read as `wall.setup`, `wall.reads` and so on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WallLean {
    /// Where the setup time went ([`SetupPhases`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<SetupPhases>,
    /// What the guest read, by path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reads: BTreeMap<String, ReadStat>,
    /// Live Rust heap bytes by allocation tag at the end
    /// (`mem_watch::heap::totals`); absent when no tag is counted.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub heap_tags: BTreeMap<String, u64>,
    /// Guest RAM pages (4 KiB) whose bytes differ between the first
    /// counted tick and the end (`impl.sim.page-census`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_pages_written_4k: Option<u64>,
    /// The same, counted in aligned 16 KiB groups of pages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guest_pages_written_16k: Option<u64>,
    /// The run's data directory's bytes at the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir_bytes: Option<u64>,
}

impl Report {
    /// Fills `wall`'s census and lean fields.
    pub fn set_lean(&mut self, lean: WallLean) {
        self.wall.lean = lean;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs_are_per_tick_in_the_json() {
        let mut tally = Tally::new();
        for tick in 1..=4 {
            tally.tick_hashed(
                TickSample {
                    tick,
                    ..Default::default()
                },
                u64::from(tick),
                None,
            );
        }
        let mut report = tally.report(Finish::bare(End::TickLimit));
        let plain = serde_json::to_value(&report).unwrap();
        assert!(plain["wall"].get("bots_ms_per_tick").is_none());
        assert!(plain.get("flat").is_none());
        report.set_costs(8_000_000, Some(6));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["wall"]["bots_ms_per_tick"], 2.0);
        assert_eq!(json["flat"]["fixups_per_tick"], 1.5);
        let back: Report = serde_json::from_value(json).unwrap();
        assert_eq!(back.wall.bots_ms_per_tick, Some(2.0));
        report.set_costs(1, None);
        assert!(report.flat.is_none());
    }

    #[test]
    fn costs_need_ticks() {
        let mut report = Tally::new().report(Finish::bare(End::Aborted));
        report.set_costs(5, Some(5));
        assert_eq!(report.wall.bots_ms_per_tick, None);
        assert!(report.flat.is_none());
        report.set_phantom_costs(PhantomCosts {
            build_ns: 5,
            remapped: 1,
            ..PhantomCosts::default()
        });
        assert_eq!(report.wall.phantom_build_ms_per_tick, None);
        assert_eq!(report.wall.phantom_snapshot_copy, None);
    }

    fn four_ticks() -> Tally {
        let mut tally = Tally::new();
        for tick in 1..=4 {
            tally.tick_hashed(
                TickSample {
                    tick,
                    ..Default::default()
                },
                u64::from(tick),
                None,
            );
        }
        tally
    }

    #[test]
    fn phantom_costs_are_per_tick_beside_the_brains() {
        let mut report = four_ticks().report(Finish::bare(End::TickLimit));
        let plain = serde_json::to_value(&report).unwrap();
        for key in [
            "phantom_build_ms_per_tick",
            "phantom_snapshot_ms_per_tick",
            "phantom_snapshots_per_tick",
            "phantom_snapshot_copy",
        ] {
            assert!(plain["wall"].get(key).is_none(), "{key}");
        }
        let start = PhantomCosts {
            build_ns: 1_000_000,
            snapshot_ns: 500_000,
            remapped: 3,
            copied: 0,
            ..PhantomCosts::default()
        };
        let end = PhantomCosts {
            build_ns: 13_000_000,
            snapshot_ns: 2_500_000,
            remapped: 7,
            copied: 0,
            ..PhantomCosts::default()
        };
        report.set_costs(8_000_000, None);
        report.set_phantom_costs(end.since(start));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["wall"]["bots_ms_per_tick"], 2.0);
        assert_eq!(json["wall"]["phantom_build_ms_per_tick"], 3.0);
        assert_eq!(json["wall"]["phantom_snapshot_ms_per_tick"], 0.5);
        assert_eq!(json["wall"]["phantom_snapshots_per_tick"], 1.0);
        assert_eq!(json["wall"]["phantom_snapshot_copy"], "remap");
        assert!(json["wall"].get("phantom_build_mismatches").is_none(), "no seat checked");
        let back: Report = serde_json::from_value(json).unwrap();
        assert_eq!(back.wall, report.wall);
    }

    #[test]
    fn check_mode_reports_its_mismatches_and_only_when_it_ran() {
        let mut report = four_ticks().report(Finish::bare(End::TickLimit));
        let start = PhantomCosts {
            checked: 8,
            mismatches: 1,
            ..PhantomCosts::default()
        };
        let clean = PhantomCosts {
            checked: 24,
            mismatches: 1,
            ..PhantomCosts::default()
        };
        report.set_phantom_costs(clean.since(start));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["wall"]["phantom_build_mismatches"], 0, "checked, none differed");
        let dirty = PhantomCosts {
            mismatches: 3,
            ..clean
        };
        report.set_phantom_costs(dirty.since(start));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["wall"]["phantom_build_mismatches"], 2);
        let back: Report = serde_json::from_value(json).unwrap();
        assert_eq!(back.wall.phantom_build_mismatches, Some(2));
        report.set_phantom_costs(PhantomCosts::default());
        assert_eq!(report.wall.phantom_build_mismatches, None);
    }

    #[test]
    fn the_phantom_split_is_per_tick_by_call_role() {
        let mut report = four_ticks().report(Finish::bare(End::TickLimit));
        let plain = serde_json::to_value(&report).unwrap();
        for key in [
            "phantom_setup_ns_per_tick",
            "phantom_call_ns_per_tick",
            "phantom_call_accesses_per_tick",
            "phantom_revert_ns_per_tick",
            "phantom_read_ns_per_tick",
            "phantom_pages_written_per_seat",
            "phantom_minor_faults_per_tick",
        ] {
            assert!(plain["wall"].get(key).is_none(), "{key}");
        }
        let start = PhantomCosts {
            setup_ns: 100,
            call_ns: [10; 6],
            call_accesses: [1; 6],
            pages_written: 5,
            pages_seats: 1,
            minor_faults: 2,
            ..PhantomCosts::default()
        };
        let end = PhantomCosts {
            setup_ns: 4_100,
            call_ns: [410, 810, 1210, 1610, 2010, 2410],
            call_accesses: [41, 81, 121, 161, 201, 241],
            revert_ns: 2_000,
            read_ns: 800,
            pages_written: 35,
            pages_seats: 7,
            minor_faults: 22,
            ..PhantomCosts::default()
        };
        report.set_phantom_costs(end.since(start));
        let json = serde_json::to_value(&report).unwrap();
        let wall = &json["wall"];
        assert_eq!(wall["phantom_setup_ns_per_tick"], 1000.0);
        assert_eq!(wall["phantom_call_ns_per_tick"]["bind"], 100.0);
        assert_eq!(wall["phantom_call_ns_per_tick"]["look_rates"], 200.0);
        assert_eq!(wall["phantom_call_ns_per_tick"]["camera"], 300.0);
        assert_eq!(wall["phantom_call_ns_per_tick"]["input"], 400.0);
        assert_eq!(wall["phantom_call_ns_per_tick"]["remap"], 500.0);
        assert_eq!(wall["phantom_call_ns_per_tick"]["builder"], 600.0);
        assert_eq!(wall["phantom_call_accesses_per_tick"]["bind"], 10.0);
        assert_eq!(wall["phantom_call_accesses_per_tick"]["builder"], 60.0);
        assert_eq!(wall["phantom_revert_ns_per_tick"], 500.0);
        assert_eq!(wall["phantom_read_ns_per_tick"], 200.0);
        assert_eq!(wall["phantom_pages_written_per_seat"], 5.0);
        assert_eq!(wall["phantom_minor_faults_per_tick"], 5.0);
        let back: Report = serde_json::from_value(json).unwrap();
        assert_eq!(back.wall, report.wall);
    }

    #[test]
    fn no_reverted_seat_leaves_the_page_mean_out() {
        let mut report = four_ticks().report(Finish::bare(End::TickLimit));
        report.set_phantom_costs(PhantomCosts::default());
        let json = serde_json::to_value(&report).unwrap();
        assert!(json["wall"].get("phantom_pages_written_per_seat").is_none());
        assert_eq!(json["wall"]["phantom_setup_ns_per_tick"], 0.0);
    }

    #[test]
    fn the_copy_kind_names_what_the_snapshots_did() {
        let kind = |remapped, copied| {
            PhantomCosts {
                remapped,
                copied,
                ..PhantomCosts::default()
            }
            .copy_kind()
        };
        assert_eq!(kind(0, 0), None);
        assert_eq!(kind(2, 0), Some("remap"));
        assert_eq!(kind(0, 2), Some("copy"));
        assert_eq!(kind(1, 1), Some("mixed"));
        // A counter that went backwards (a reset) gives nothing, not a wrap.
        let earlier = PhantomCosts {
            build_ns: 9,
            ..PhantomCosts::default()
        };
        assert_eq!(PhantomCosts::default().since(earlier).build_ns, 0);
    }

    fn census(users: u32, empty: bool, arrays: [Option<u32>; 5]) -> CensusSample {
        CensusSample {
            local_users: Some(users),
            local_visibility_empty: Some(empty),
            arrays,
        }
    }

    #[test]
    fn the_census_is_mean_and_peak_over_the_ticks_read() {
        let mut tally = four_ticks();
        assert!(tally.census().is_none());
        let plain = serde_json::to_value(tally.report(Finish::bare(End::TickLimit))).unwrap();
        assert!(plain.get("guest_census").is_none());
        tally.tick_census(
            1,
            &census(0, true, [Some(2), Some(1), Some(10), Some(0), None]),
        );
        // A second read of one tick is not counted.
        tally.tick_census(1, &census(4, false, [Some(99); 5]));
        tally.tick_census(
            2,
            &census(0, true, [Some(4), Some(3), Some(30), Some(1), None]),
        );
        tally.tick_census(
            3,
            &CensusSample {
                local_users: None,
                local_visibility_empty: None,
                arrays: [Some(0), Some(2), Some(20), Some(2), None],
            },
        );
        let report = tally.report(Finish::bare(End::TickLimit));
        let c = report.guest_census.clone().unwrap();
        assert_eq!(c.ticks, 3);
        assert_eq!(
            c.local_users,
            Some(CountStat {
                samples: 2,
                mean: 0.0,
                peak: 0
            })
        );
        assert_eq!(
            c.local_visibility,
            Some(VisibilityStat {
                samples: 2,
                empty_ticks: 2
            })
        );
        assert_eq!(c.arrays["effect"].mean, 2.0);
        assert_eq!(c.arrays["effect"].peak, 4);
        assert_eq!(c.arrays["particles"].mean, 20.0);
        assert_eq!(c.arrays["particles"].peak, 30);
        assert_eq!(c.arrays["decals"].peak, 2);
        // An array never read is left out.
        assert!(!c.arrays.contains_key("lights"));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["guest_census"]["ticks"], 3);
        assert_eq!(json["guest_census"]["local_visibility"]["empty_ticks"], 2);
        assert_eq!(json["guest_census"]["arrays"]["particle_system"]["peak"], 3);
        assert_eq!(
            json["guest_census"]["arrays"]["particle_system"]["samples"],
            3
        );
        let back: Report = serde_json::from_value(json).unwrap();
        assert_eq!(back.guest_census, report.guest_census);
        // A restart drops it with the rest of the tally.
        tally.start(100);
        assert!(tally.census().is_none());
    }

    #[test]
    fn a_census_sample_answers_with_nulls_for_what_it_could_not_read() {
        let v = CensusSample {
            local_users: Some(0),
            local_visibility_empty: None,
            arrays: [Some(3), None, Some(7), Some(1), Some(12)],
        }
        .to_json();
        assert_eq!(v["local_users"], 0);
        assert!(v["local_visibility_empty"].is_null());
        assert_eq!(v["arrays"]["effect"], 3);
        assert!(v["arrays"]["particle_system"].is_null());
        assert_eq!(v["arrays"]["lights"], 12);
        assert_eq!(v["arrays"].as_object().unwrap().len(), CENSUS_ARRAYS.len());
    }

    fn p(index: u8, score: i32) -> PlayerSample {
        PlayerSample {
            index,
            name: format!("P{index}"),
            team: Some(i32::from(index % 2)),
            unit: Some(0x1000 + u32::from(index)),
            alive: true,
            grenades: Some(2),
            score,
            ..PlayerSample::default()
        }
    }

    fn sample(tick: u32, players: Vec<PlayerSample>) -> TickSample {
        TickSample {
            tick,
            players,
            teams: vec![],
        }
    }

    #[test]
    fn tick_hashes_follow_duplicates_gaps_and_rewinds() {
        let mut tally = Tally::new();
        for (tick, hash) in [(5, 1), (5, 2), (7, 3)] {
            tally.tick_hashed(
                TickSample {
                    tick,
                    ..Default::default()
                },
                hash,
                Some(hash + 1),
            );
        }
        let report = tally.report(Finish::bare(End::Stopped));
        assert_eq!(report.ticks, 2);
        assert_eq!(report.state_hashes_v1.len(), 2);
        assert_eq!(
            report.state_hashes_v1[0],
            TickHash {
                tick: 5,
                gameplay: "0000000000000002".into(),
                arena: Some("0000000000000003".into()),
            }
        );
        assert_eq!(report.state_hashes_v1[1].tick, 7);
        let mut old = serde_json::to_value(report).unwrap();
        old.as_object_mut().unwrap().remove("state_hashes_v1");
        assert!(
            serde_json::from_value::<Report>(old)
                .unwrap()
                .state_hashes_v1
                .is_empty()
        );
        tally.tick_hashed(
            TickSample {
                tick: 3,
                ..Default::default()
            },
            9,
            None,
        );
        assert_eq!(tally.hashes, [(3, 9, None)]);
    }

    #[test]
    fn counts_ticks_alive_and_vehicles() {
        let mut t = Tally::new();
        t.start(10);
        for tick in 10..20 {
            let mut a = p(0, 0);
            a.in_vehicle = tick >= 15;
            let mut b = p(1, 0);
            b.alive = tick < 12;
            t.tick(&sample(tick, vec![a, b]));
        }
        // The same tick twice counts once.
        t.tick(&sample(19, vec![p(0, 0)]));
        assert_eq!(t.ticks(), 10);
        let r = t.report(Finish {
            team_play: false,
            ..Finish::bare(End::TickLimit)
        });
        let a = r.player(0).unwrap();
        assert_eq!(a.alive_ticks, 10);
        assert_eq!(a.vehicle_ticks, 5);
        assert_eq!(r.player(1).unwrap().alive_ticks, 2);
    }

    #[test]
    fn grenades_fall_within_a_life() {
        let mut t = Tally::new();
        t.start(0);
        let with = |tick, unit, g| {
            let mut a = p(0, 0);
            a.unit = Some(unit);
            a.grenades = Some(g);
            sample(tick, vec![a])
        };
        t.tick(&with(0, 1, 4));
        t.tick(&with(1, 1, 3)); // threw one
        t.tick(&with(2, 1, 4)); // picked one up
        t.tick(&with(3, 1, 2)); // threw two
        t.tick(&with(4, 2, 2)); // new life: no throw
        t.tick(&with(5, 2, 1)); // threw one
        let r = t.report(Finish {
            team_play: false,
            ..Finish::bare(End::Stopped)
        });
        assert_eq!(r.player(0).unwrap().grenades, 4);
    }

    #[test]
    fn shots_hits_and_damage() {
        let mut t = Tally::new();
        t.start(0);
        t.tick(&sample(0, vec![p(0, 0), p(1, 0)]));
        for _ in 0..4 {
            t.shot(0);
        }
        t.hit(Some(0), Some(1), 0.25, false);
        t.hit(Some(0), Some(0), 0.5, false); // own grenade: taken, not dealt
        t.hit(None, Some(1), 1.0, false); // the world
        t.hit(Some(0), None, 1.0, false); // a vehicle or a crate
        t.hit(Some(0), Some(1), f32::NAN, false);
        let r = t.report(Finish {
            team_play: false,
            ..Finish::bare(End::Stopped)
        });
        let a = r.player(0).unwrap();
        assert_eq!((a.shots, a.hits), (4, 2));
        assert_eq!(a.accuracy, 0.5);
        assert_eq!(a.damage_dealt, 0.25);
        assert_eq!(a.damage_taken, 0.5);
        assert_eq!(r.player(1).unwrap().damage_taken, 1.25);
    }

    #[test]
    fn fights_exclude_friendly_self_and_zero_damage() {
        let mut t = Tally::new();
        let mut players = vec![p(0, 0), p(1, 0), p(2, 0)];
        for (p, team) in players.iter_mut().zip([0, 0, 1]) {
            p.team = Some(team);
        }
        t.tick(&sample(1, players));
        for (v, dmg) in [(0, 1.), (1, 2.), (2, 3.), (2, 0.)] {
            t.hit(Some(0), Some(v), dmg, true);
        }
        let r = t.report(Finish::bare(End::GameOver));
        assert_eq!(t.fights.keys().copied().collect::<Vec<_>>(), [(0, 2)]);
        assert_eq!(r.player(0).unwrap().damage_dealt, 5.);
        assert_eq!(r.player(2).unwrap().damage_taken, 3.);
    }

    #[test]
    fn kills_are_classified() {
        let mut t = Tally::new();
        t.start(0);
        t.kill(5, Some(0), 1, false);
        t.kill(6, Some(1), 1, false);
        t.kill(7, Some(2), 0, true);
        t.kill(8, None, 2, false);
        let kinds: Vec<_> = t.kills().iter().map(|k| k.kind).collect();
        assert_eq!(
            kinds,
            [
                KillKind::Kill,
                KillKind::Suicide,
                KillKind::Betrayal,
                KillKind::World
            ]
        );
        let r = t.report(Finish {
            team_play: true,
            ..Finish::bare(End::GameOver)
        });
        assert_eq!(r.player(2).unwrap().betrayals, 1);
    }

    #[test]
    fn brain_ticks_count_by_the_player_the_bot_became() {
        let mut t = Tally::new();
        t.start(0);
        let busy = |stuck, repaths, doing| BrainTick {
            stuck,
            repaths,
            doing,
            aim: None,
        };
        t.tick(&sample(0, vec![p(5, 0)]));
        t.brain_for_player(5, busy(true, 1, "fight"));
        t.brain_for_player(5, busy(false, 1, "carry"));
        t.brain_for_player(5, busy(false, 0, "roam")); // new life: count restarts
        t.brain_for_player(5, busy(false, 2, "roam"));
        let bots = [BotInfo {
            seat: 2,
            player: Some(5),
            name: "Clanker 2".into(),
            arm: Some("b".into()),
        }];
        let r = t.report(Finish {
            bots: &bots,
            ..Finish::bare(End::Stopped)
        });
        let b = r.player(5).unwrap();
        assert!(b.bot);
        assert_eq!(b.seat, Some(2));
        assert_eq!(b.arm.as_deref(), Some("b"));
        assert_eq!(b.stuck_ticks, 1);
        assert_eq!(b.repaths, 3);
        assert_eq!(b.objective_ticks, 1);
        assert_eq!(b.doing.get("roam"), Some(&2));
        assert_eq!(b.doing.get("fight"), Some(&1));
    }

    #[test]
    fn going_back_in_time_restarts() {
        let mut t = Tally::new();
        t.start(100);
        t.tick(&sample(100, vec![p(0, 0)]));
        t.shot(0);
        t.tick(&sample(101, vec![p(0, 0)]));
        t.tick(&sample(50, vec![p(0, 0)]));
        assert_eq!(t.resets(), 1);
        assert_eq!(t.ticks(), 1);
        let r = t.report(Finish {
            team_play: false,
            ..Finish::bare(End::Stopped)
        });
        assert_eq!(r.player(0).unwrap().shots, 0);
        assert_eq!(r.notes.len(), 1);
        // Forward past a second's worth of ticks: a load too.
        t.tick(&sample(51, vec![]));
        t.tick(&sample(51 + JUMP_TICKS, vec![]));
        assert_eq!((t.resets(), t.ticks()), (1, 3));
        t.tick(&sample(52 + 2 * JUMP_TICKS, vec![]));
        assert_eq!((t.resets(), t.ticks()), (2, 1));
    }

    #[test]
    fn time_to_kill_runs_from_the_fights_first_hit() {
        let mut t = Tally::new();
        t.start(0);
        let both = || vec![p(0, 0), p(1, 0)];
        // Player 0 first hurts 1 on tick 11, again on 31, and kills on 41.
        for tick in 0..=40 {
            t.tick(&sample(tick, both()));
            if tick == 10 || tick == 30 {
                t.hit(Some(0), Some(1), 0.3, false);
            }
        }
        t.kill(41, Some(0), 1, false);
        // A new life: one hit, a long gap, then a second fight of 21 ticks.
        for tick in 41..=300 {
            t.tick(&sample(tick, both()));
            if tick == 50 || tick == 279 {
                t.hit(Some(0), Some(1), 0.3, false);
            }
            // No damage and a hit on itself start nothing.
            if tick == 45 {
                t.hit(Some(1), Some(0), 0.0, false);
                t.hit(Some(1), Some(1), 0.5, false);
            }
        }
        t.kill(300, Some(0), 1, false);
        // Killed with nothing landed first (a fall, a stick): no time.
        t.kill(300, Some(1), 0, false);
        t.kill(300, None, 1, false);
        let r = t.report(Finish::bare(End::GameOver));
        let a = r.player(0).unwrap();
        assert_eq!(a.ttk_kills, 2);
        assert_eq!(a.ttk_ticks, (30.0 + 21.0) / 2.0);
        let b = r.player(1).unwrap();
        assert_eq!((b.ttk_kills, b.ttk_ticks), (0, 0.0));
    }

    /// A view a tick: the error, and a stick that follows its sign.
    fn aim(t: &mut Tally, tick: u32, target: u32, yaw: f32, stick: f32) {
        t.brain_for_player(
            0,
            BrainTick {
                doing: "fight",
                aim: Some(AimSample {
                    tick,
                    target,
                    error: [yaw, 0.0],
                    stick: (stick, 0.0),
                }),
                ..BrainTick::default()
            },
        );
    }

    #[test]
    fn aim_motion_measures_overshoot_settle_and_jitter() {
        let mut t = Tally::new();
        t.start(0);
        t.tick(&sample(0, vec![p(0, 0)]));
        // A flick from 0.8: ten views closing, four past the target (0.12
        // at the worst), then on it.
        let mut tick = 100;
        let mut errors: Vec<f32> = (0..10).map(|i| 0.8 - 0.08 * i as f32).collect();
        errors.extend([-0.04, -0.12, -0.08, -0.02]);
        errors.extend([0.01; 12]);
        for e in &errors {
            let stick = if *e > 0.03 {
                -0.5
            } else if *e < -0.03 {
                0.5
            } else {
                0.0
            };
            aim(&mut t, tick, 7, *e, stick);
            // The brain's view repeated on a tick it did not think: once.
            aim(&mut t, tick, 7, *e, 0.9);
            tick += 1;
        }
        // Another target, close already: an engagement, not a flick.
        for i in 0..5 {
            aim(&mut t, tick + i, 8, 0.1, -0.2);
        }
        let r = t.report(Finish::bare(End::GameOver));
        let a = r.player(0).unwrap().aim.clone().unwrap();
        assert_eq!(a.tracked_ticks, errors.len() as u64 + 5);
        assert_eq!(a.flicks, 1);
        assert!((a.overshoot_ratio - 0.12 / 0.8).abs() < 1e-6, "{a:?}");
        // Views 13.. are inside 0.05; the sixth of them settles it.
        assert_eq!((a.settled, a.settle_ticks), (1, 13.0));
        // One reversal past the target; the stick moved on two views of
        // the first engagement's 25 pairs and none of the second's four.
        let pairs = (errors.len() - 1 + 4) as f64;
        assert!((a.reversals_per_s - 30.0 / pairs).abs() < 1e-9, "{a:?}");
        assert!((a.stick_jitter - (1.0 + 0.5) / pairs).abs() < 1e-6, "{a:?}");
        let mean = (errors.iter().map(|e| e.abs()).sum::<f32>() + 0.5) / (errors.len() + 5) as f32;
        assert!((a.mean_error - f64::from(mean)).abs() < 1e-5, "{a:?}");
        // The report as it stands does not close the tally's engagement.
        let again = t.report(Finish::bare(End::GameOver));
        assert_eq!(again.player(0).unwrap().aim, Some(a));
    }

    #[test]
    fn aim_motion_splits_engagements_and_leaves_out_what_never_settles() {
        let mut t = Tally::new();
        t.start(0);
        t.tick(&sample(0, vec![p(0, 0)]));
        // Never inside 0.05 for six views: a flick that does not settle.
        for i in 0..40 {
            aim(&mut t, i, 3, if i % 4 == 1 { 0.02 } else { 0.5 }, -1.0);
        }
        // The same target after a gap is a new flick; it overshoots by
        // half, and only what happens in the first two seconds counts.
        for i in 0..80 {
            let e = match i {
                0 => 0.4,
                1..=9 => -0.2,
                70 => -0.4,
                _ => 0.0,
            };
            aim(&mut t, 50 + i, 3, e, 0.0);
        }
        // A view that is not a number is dropped.
        aim(&mut t, 500, 3, f32::NAN, 0.0);
        let a = t
            .report(Finish::bare(End::GameOver))
            .player(0)
            .unwrap()
            .aim
            .clone()
            .unwrap();
        assert_eq!(a.tracked_ticks, 120);
        assert_eq!(a.flicks, 2);
        assert!((a.overshoot_ratio - 0.25).abs() < 1e-6, "{a:?}");
        assert_eq!((a.settled, a.settle_ticks), (1, 10.0));
        assert_eq!(a.reversals_per_s, 0.0);
        // A player with no tracked view has no aim section at all.
        let mut none = Tally::new();
        none.tick(&sample(0, vec![p(0, 0)]));
        none.brain_for_player(0, BrainTick::default());
        let r = none.report(Finish::bare(End::GameOver));
        assert_eq!(r.player(0).unwrap().aim, None);
        let v = serde_json::to_value(&r).unwrap();
        assert!(v["players"][0].get("aim").is_none());
        assert_eq!(v["players"][0]["ttk_kills"], 0);
    }

    #[test]
    fn earlier_reports_still_load() {
        // A player line as written before the time to kill and the aim
        // section existed.
        let old = serde_json::json!({
            "index": 0, "name": "A1", "bot": true, "score": 3, "kills": 3, "deaths": 1,
            "assists": 0, "suicides": 0, "betrayals": 0, "shots": 10, "hits": 4,
            "accuracy": 0.4, "damage_dealt": 2.0, "damage_taken": 1.0, "alive_ticks": 9,
            "vehicle_ticks": 0, "grenades": 0, "stuck_ticks": 0, "repaths": 0,
            "objective_ticks": 0
        });
        let p: PlayerReport = serde_json::from_value(old).unwrap();
        assert_eq!((p.ttk_kills, p.ttk_ticks, p.aim.clone()), (0, 0.0, None));
        assert!(p.combat.is_none() && p.weapon_accuracy.is_empty());
        // And the new fields round trip.
        let new = PlayerReport {
            ttk_kills: 2,
            ttk_ticks: 25.5,
            aim: Some(AimReport {
                tracked_ticks: 30,
                mean_error: 0.04,
                flicks: 1,
                overshoot_ratio: 0.15,
                settled: 1,
                settle_ticks: 13.0,
                stick_jitter: 0.08,
                reversals_per_s: 1.2,
            }),
            ..p
        };
        let v = serde_json::to_value(&new).unwrap();
        assert_eq!(v["aim"]["overshoot_ratio"], 0.15);
        assert_eq!(serde_json::from_value::<PlayerReport>(v).unwrap(), new);
    }

    #[test]
    fn a_report_from_before_the_memory_fields_still_parses() {
        let old = serde_json::json!({
            "setup_s": 1.0, "match_s": 2.0, "process_s": 3.0,
            "speedup": 4.0, "process_speedup": 5.0, "ms_per_tick": 6.0
        });
        let wall: Wall = serde_json::from_value(old).unwrap();
        assert_eq!(wall.peak_rss_mib, 0);
        assert_eq!(wall.footprint_mib, None);
        // A report without a footprint is not serialized with one.
        let v = serde_json::to_value(&wall).unwrap();
        assert!(v.get("footprint_mib").is_none());
    }

    #[test]
    fn winners() {
        let mut t = Tally::new();
        t.start(0);
        t.tick(&sample(0, vec![p(0, 3), p(1, 5)]));
        assert_eq!(t.leader(false), Some(Winner::Player(1)));
        t.tick(&sample(1, vec![p(0, 5), p(1, 5)]));
        assert_eq!(t.leader(false), None);
        let mut s = sample(2, vec![p(0, 5), p(1, 5)]);
        s.teams = vec![(0, 7), (1, 9)];
        t.tick(&s);
        assert_eq!(t.leader(true), Some(Winner::Team(1)));
        assert_eq!(Tally::new().leader(false), None);
    }

    #[test]
    fn report_round_trips_as_json() {
        let mut t = Tally::new();
        t.start(0);
        let mut s = sample(0, vec![p(0, 1)]);
        s.teams = vec![(0, 1)];
        t.tick(&s);
        t.kill(0, Some(0), 0, false);
        let speed = SpeedMeter {
            setup_s: 1.0,
            match_s: 0.5,
            ticks: 999,
        };
        let r = t.report(Finish {
            match_: serde_json::json!({"map": "lockout"}),
            speed,
            memory: MemoryUsage {
                mapped_read_bytes: 65536,
                copied_read_bytes: 13,
                peak_rss_mib: 981,
                footprint_mib: Some(143),
                ..MemoryUsage::default()
            },
            ..Finish::bare(End::GameOver)
        });
        // The meter's ticks are the tally's.
        assert_eq!(r.ticks, 1);
        assert_eq!(r.wall.process_s, 1.5);
        assert_eq!(r.wall.mapped_read_bytes, 65536);
        assert_eq!(r.wall.copied_read_bytes, 13);
        assert_eq!(r.wall.peak_rss_mib, 981);
        assert_eq!(r.wall.footprint_mib, Some(143));
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["format"], FORMAT);
        assert_eq!(v["match"]["map"], "lockout");
        assert_eq!(v["result"]["end"], "game_over");
        assert_eq!(v["result"]["winner"], serde_json::json!({"player": 0}));
        assert_eq!(v["kills"][0]["kind"], "suicide");
        assert_eq!(v["wall"]["peak_rss_mib"], 981);
        assert_eq!(v["wall"]["footprint_mib"], 143);
        let back: Report = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn the_lean_fields_round_trip_and_old_reports_still_load() {
        let mut report = Tally::new().report(Finish::bare(End::Stopped));
        // Nothing set, nothing written: an old consumer sees the old shape.
        let plain = serde_json::to_value(&report).unwrap();
        for key in [
            "setup",
            "reads",
            "heap_tags",
            "guest_pages_written_4k",
            "guest_pages_written_16k",
            "data_dir_bytes",
        ] {
            assert!(plain["wall"].get(key).is_none(), "{key}");
        }
        assert!(plain.get("state_digest_v1").is_none());
        let lean = WallLean {
            setup: Some(SetupPhases {
                boot_s: 1.5,
                menu_s: 2.0,
                lobby_s: 3.0,
                copy_wait_s: 4.0,
                load_s: 5.0,
                bind_s: 0.25,
            }),
            reads: BTreeMap::from([(
                "\\Device\\Harddisk0\\maps\\lockout.map".to_string(),
                ReadStat {
                    mapped_bytes: 0,
                    copied_bytes: 4096,
                    count: 3,
                    refused: Some("short read".into()),
                },
            )]),
            heap_tags: BTreeMap::from([("guest.ram".to_string(), 64 << 20)]),
            guest_pages_written_4k: Some(2100),
            guest_pages_written_16k: Some(900),
            data_dir_bytes: Some(1 << 30),
        };
        report.set_lean(lean.clone());
        let v = serde_json::to_value(&report).unwrap();
        // They sit in `wall`, beside the fields they join.
        assert_eq!(v["wall"]["setup"]["copy_wait_s"], 4.0);
        assert_eq!(v["wall"]["guest_pages_written_16k"], 900);
        assert_eq!(v["wall"]["data_dir_bytes"], 1u64 << 30);
        assert_eq!(v["wall"]["setup_s"], 0.0);
        let key = "\\Device\\Harddisk0\\maps\\lockout.map";
        assert_eq!(v["wall"]["reads"][key]["refused"], "short read");
        assert_eq!(v["wall"]["reads"][key]["count"], 3);
        let back: Report = serde_json::from_value(v).unwrap();
        assert_eq!(back.wall.lean, lean);
        assert_eq!(back, report);
        // A wall block from before any of it parses with them empty.
        let old = serde_json::json!({
            "setup_s": 1.0, "match_s": 2.0, "process_s": 3.0,
            "speedup": 4.0, "process_speedup": 5.0, "ms_per_tick": 6.0,
            "guest_written_pages": 7, "guest_pages": 16384
        });
        let wall: Wall = serde_json::from_value(old).unwrap();
        assert_eq!(wall.lean, WallLean::default());
        assert_eq!(wall.guest_written_pages, Some(7));
    }

    #[test]
    fn the_setup_clock_charges_each_stage_and_only_moves_forward() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut c = SetupClock::new(t0);
        c.enter(SetupStage::Menu, at(10));
        c.enter(SetupStage::Menu, at(11)); // already there: no charge
        c.enter(SetupStage::Lobby, at(13));
        c.enter(SetupStage::Menu, at(14)); // never back
        c.enter(SetupStage::Load, at(20)); // skipping the copy wait
        c.enter(SetupStage::Bind, at(25));
        let p = c.phases(at(26)); // the running stage counts to now
        assert_eq!(
            (p.boot_s, p.menu_s, p.lobby_s, p.copy_wait_s, p.load_s, p.bind_s),
            (10.0, 3.0, 7.0, 0.0, 5.0, 1.0)
        );
        c.enter(SetupStage::Done, at(27));
        let p = c.phases(at(99)); // done: nothing more accrues
        assert_eq!(p.bind_s, 2.0);
        assert_eq!(p.boot_s + p.menu_s + p.lobby_s + p.load_s + p.bind_s, 27.0);
    }

    fn training_tally() -> Tally {
        let mut t = Tally::new();
        t.set_detail(Detail::Training);
        t.start(0);
        t
    }

    fn feed(t: &mut Tally, ticks: impl IntoIterator<Item = u32>, salt: u64) {
        for tick in ticks {
            t.tick_hashed(
                TickSample { tick, ..Default::default() },
                u64::from(tick) ^ salt,
                None,
            );
            t.tick_components(tick, None, [("x".to_string(), tick.to_string())]);
        }
    }

    #[test]
    fn a_training_tally_keeps_sparse_rows_a_rolling_digest_and_no_components() {
        let mut t = training_tally();
        feed(&mut t, 1..=100, 0);
        let r = t.report(Finish::bare(End::TickLimit));
        let rows: Vec<u32> = r.state_hashes_v1.iter().map(|h| h.tick).collect();
        assert_eq!(rows, [1, 30, 60, 90]);
        assert!(r.state_components_v1.is_none());
        let d = r.state_digest_v1.clone().unwrap();
        assert_eq!((d.ticks, d.stride, d.digest.len()), (100, TRAINING_STRIDE, 16));
        // Deterministic, and every tick matters, even between the sampled ones.
        let again = |salt_at: Option<u32>| {
            let mut t = training_tally();
            for tick in 1..=100 {
                let salt = if Some(tick) == salt_at { 1 } else { 0 };
                feed(&mut t, [tick], salt);
            }
            t.report(Finish::bare(End::TickLimit)).state_digest_v1.unwrap().digest
        };
        assert_eq!(again(None), d.digest);
        assert_ne!(again(Some(45)), d.digest);
        // A rewind starts the series again, in the same detail.
        feed(&mut t, [5000], 0);
        let r = t.report(Finish::bare(End::TickLimit));
        assert_eq!(t.detail(), Detail::Training);
        assert_eq!(r.state_digest_v1.as_ref().unwrap().ticks, 1);
        // It survives the JSON round trip.
        let back: Report = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(back.state_digest_v1.map(|d| d.ticks), Some(1));
    }

    #[test]
    fn a_tick_seen_twice_counts_once_in_the_rolling_digest() {
        let mut once = training_tally();
        feed(&mut once, 1..=4, 0);
        let mut twice = training_tally();
        feed(&mut twice, 1..=3, 0);
        feed(&mut twice, [4], 9); // a stale view of tick 4 ...
        feed(&mut twice, [4], 0); // ... replaced by the latest
        let (a, b) = (
            once.report(Finish::bare(End::TickLimit)),
            twice.report(Finish::bare(End::TickLimit)),
        );
        assert_eq!(a.state_digest_v1, b.state_digest_v1);
        assert_eq!(b.state_digest_v1.unwrap().ticks, 4);
    }

    #[test]
    fn a_full_tally_has_dense_rows_and_no_digest() {
        let mut t = Tally::new();
        t.start(0);
        feed(&mut t, 1..=40, 0);
        let r = t.report(Finish::bare(End::TickLimit));
        assert_eq!(r.state_hashes_v1.len(), 40);
        assert!(r.state_digest_v1.is_none());
        assert_eq!(r.state_components_v1.unwrap().ticks.len(), 40);
    }
}
