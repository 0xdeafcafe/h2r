//! Sim mode (`H2_SIM=<match file>`, `h2 sim run`; docs/runtime/sim.md): the
//! game's simulation with bots, without rendering, as fast as the host goes,
//! ending in a JSON report.
//!
//! What it does to a boot (wired in `run_hosted_with_live`):
//!
//! - **The level** is the match's multiplayer map, through a System Link
//!   lobby the host opens: every bot joins as a phantom peer.
//! - **The bots** are the match file's, each a [`bots::Brain`] with its
//!   skill, its seed (from the match seed and its logical seat) and its tuning, in a
//!   [`Metered`] wrapper that counts what only the brain knows.
//! - **The render front-end** is skipped in world frames on the light
//!   host ([`sim_bridge::sim_frame`]). Full headless runs can opt into the
//!   render worker and collect the same tick hashes. The kernel trace
//!   file is off and nav meshes are never built in the background
//!   (`bots_nav_auto`).
//! - **The pace** is the match's ([`sim_bench::clock::Pace`]); unpaced by default.
//! - **The metrics** come from read-only observers ([`SimMetrics`]): each
//!   tick's player snapshot and guest census ([`crate::sim_census`]), the
//!   shots, the damage reports, the kills and the end latch, into a
//!   [`sim_bench::metrics::Tally`].
//! - **The spatial trace** (`detail = "spatial"`): the same observers
//!   feed a [`sim_bench::spatial::Recorder`] beside the tally
//!   ([`spatial`]), and [`Sim::finish`] writes it beside the report.
//! - **The RNG seed** is written once, before the match's
//!   first tick ([`SimMetrics`]'s one writing hook).
//! - **The end**: the game's end latch, the match's tick limit, its wall
//!   limit or `match.stop` stop the boot loop ([`Sim::keep_running`]), and
//!   [`Sim::finish`] writes the report to `H2_SIM_REPORT`.
//!
//! The agent gets `match.status`, `match.report`, `match.stop` and
//! `sim.guest_census` ([`Sim::add_methods`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use agent::Access;
use agent_proto::{RpcError, code, method};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use bots::pad_source::{Frame, PadSource, Sampled};
use sim_bridge::sim_frame::SimFrame;

use crate::telemetry;
use hooks::{Flow, points};
use input::host::HostPadState;
use sim_bench::config::{BotParams, Detail, Match};
use sim_bench::metrics::{
    BotInfo, BrainTick, End, Finish, PlayerSample, ReadStat, Report, SetupClock, SetupStage, Tally,
    TickSample, WallLean,
};

use crate::level_boot;
use std::collections::BTreeMap;
mod film_state;
pub mod hosted;
pub mod spatial;
mod state_hash;

/// The engine's shared random seed: a pointer to the struct whose first
/// word it is.
pub const RANDOM_SEED_PTR: u32 = 0x004e_7408;

/// The sim report's `state_hashes_v1` gameplay digest of the tick just
/// ended, with that tick, as sim mode takes it (`projected`: the
/// `sim.metrics-projection` switch, default on). `sim.hashes` records it
/// on any run ([`crate::agent_frames`]).
pub fn report_gameplay_hash(guest: &runtime::Guest, projected: bool) -> Option<(u32, u64)> {
    let snapshot = if projected {
        telemetry::sim_snapshot(guest)
    } else {
        telemetry::snapshot(guest)
    }
    .ok()?;
    let rng = guest
        .u32(RANDOM_SEED_PTR)
        .ok()
        .filter(|p| *p != 0)
        .and_then(|p| guest.u32(p).ok());
    Some((
        (snapshot.tick & 0xffff_ffff) as u32,
        state_hash::gameplay(&snapshot, rng),
    ))
}

/// Film checkpoints use the simulation report's post-tick digest. Campaigns
/// without a multiplayer statborg can still verify the engine's game-state arena.
pub fn film_gameplay_hash(guest: &runtime::Guest) -> Option<u64> {
    if let Ok(snapshot) = telemetry::sim_snapshot(guest) {
        let rng = guest.u32(RANDOM_SEED_PTR).ok().filter(|p| *p != 0)
            .and_then(|p| guest.u32(p).ok());
        return Some(state_hash::gameplay(&snapshot, rng));
    }
    match level_boot::active_mode(guest) {
        Some(level_boot::MODE_CAMPAIGN | level_boot::MODE_MULTIPLAYER) =>
            lockstep::hash_gameplay(guest, lockstep::SimRegion::HALO2),
        _ => None,
    }
}

/// Weapons fired between two ticks that are kept for attribution.
const MAX_FIRED: usize = 512;

/// Where the match is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Booting, loading, or a game that is not the match.
    Waiting,
    Binding,
    Playing,
    Over,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Waiting => "waiting",
            Phase::Binding => "binding",
            Phase::Playing => "playing",
            Phase::Over => "over",
        }
    }
}

struct State {
    machines: BTreeMap<u8, [u8; 6]>,
    bindings: BTreeMap<u8, u8>,
    tally: Tally,
    phase: Phase,
    end: Option<End>,
    first_tick: Option<Instant>,
    last_tick: Option<Instant>,
    dispatch_start: Option<runtime::DispatchCounts>,
    dispatch_end: Option<runtime::DispatchCounts>,
    /// The running cost totals when counting began, for
    /// `wall.bots_ms_per_tick`, `wall.phantom_*` and `flat.fixups_per_tick`.
    costs_start: Option<Costs>,
    /// The initial RNG seed has been written for this match.
    set_up: bool,
    /// The game options' spawn words have been written for this game.
    spawn_seeded: bool,
    /// Weapon datums fired since the last tick.
    fired: Vec<u32>,
    notes: Vec<String>,
    /// A copy-on-write guest RAM base census at the match's end
    /// (`state.load` with `"cow": true`): pages written over the base.
    cow: Option<(u64, u64)>,
    file_reads: vfs::GuestReadBytes,
    /// Where the setup time went (`wall.setup`).
    setup: SetupClock,
    /// The page census (`impl.sim.page-census`).
    pages: Pages,
    /// The end-of-run census and lean fields ([`Sim::note_lean`]).
    lean: WallLean,
    /// The spatial trace, with `detail = "spatial"`; taken by
    /// [`Sim::finish`].
    spatial: Option<sim_bench::spatial::Recorder>,
}

impl State {
    /// A kill, into the tally and the trace alike.
    fn kill(
        &mut self,
        tick: u32,
        killer: Option<u8>,
        victim: u8,
        betrayal: bool,
        ground: &dyn sim_bench::spatial::Ground,
    ) {
        self.tally.kill(tick, killer, victim, betrayal);
        if let Some(r) = self.spatial.as_mut() {
            r.kill(tick, killer, victim, betrayal, ground);
        }
    }

    /// A damage report, into the tally and, against a player, the trace;
    /// `tick` is the hook's, else the tick after the last sample.
    fn hit(
        &mut self,
        tick: Option<u64>,
        attacker: Option<u8>,
        victim: Option<u8>,
        damage: f32,
        team_play: bool,
        ground: &dyn sim_bench::spatial::Ground,
    ) {
        self.tally.hit(attacker, victim, damage, team_play);
        let tick = tick
            .map(|t| (t & 0xffff_ffff) as u32)
            .or_else(|| self.tally.last().map(|l| l.tick.wrapping_add(1)))
            .unwrap_or(0);
        if let (Some(r), Some(v)) = (self.spatial.as_mut(), victim) {
            r.damage(tick, attacker, v, damage, ground);
        }
    }

    /// The trace's side of a tick: spawns, pickups and the samples.
    fn trace_tick(
        &mut self,
        tick: u32,
        snap: &telemetry::Snapshot,
        ground: &dyn sim_bench::spatial::Ground,
    ) {
        if let Some(r) = self.spatial.as_mut() {
            r.tick(tick, &spatial::poses(snap), ground);
        }
    }

    /// The match (re)starts for the tally's sake: so does the trace.
    fn trace_start(&mut self, tick: u32) {
        if let Some(r) = self.spatial.as_mut() {
            r.start(tick);
        }
    }
}

/// The page census's state: the page hashes of the first counted tick.
#[derive(Default)]
struct Pages {
    /// The switch is on.
    enabled: bool,
    /// The next tick takes the baseline.
    wanted: bool,
    base: Option<Vec<u64>>,
}

struct Inner {
    path: PathBuf,
    hosted_map: std::sync::OnceLock<level_boot::Request>,
    lobby: Arc<Mutex<hosted::LobbyState>>,
    matched: Match,
    report_path: Option<PathBuf>,
    frame: SimFrame,
    state: Mutex<State>,
    stop: AtomicBool,
    created: Instant,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Ends the match (the first end wins) and asks the loop to stop.
    fn end(&self, s: &mut State, end: End) {
        if s.end.is_none() {
            s.end = Some(end);
            s.phase = Phase::Over;
            eprintln!(
                "sim: match over ({}) at {} ticks, {:.1}x real time",
                end.name(),
                s.tally.ticks(),
                speed(self, s).speedup()
            );
        }
        self.stop.store(true, Ordering::Release);
    }
}

/// Sim mode for one run. Cheap to clone; clones share.
#[derive(Clone)]
pub struct Sim(Arc<Inner>);

/// Whether sim uses the shared light profile. Off supplies a full headless
/// control and supports the old machine-snapshot workflow.
pub fn use_light_host() -> Result<bool> {
    let switches = hooks::Switches::from_env().map_err(anyhow::Error::msg)?;
    Ok(switches.feature("host-light", true))
}

/// The match file `H2_SIM` names, read and checked; `None` when unset.
/// `H2_BOOT_GFX` is refused: sim mode has no GPU.
pub fn from_env() -> Result<Option<Sim>> {
    let Some(path) = std::env::var_os(sim_bench::ENV).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if std::env::var_os("H2_BOOT_GFX").is_some() {
        bail!("{}: sim mode renders nothing; unset H2_BOOT_GFX", sim_bench::ENV);
    }
    let path = PathBuf::from(path);
    let matched = sim_bench::config::load_match(&path)
        .map_err(|e| anyhow::anyhow!("{}: {}", sim_bench::ENV, e.replace('\n', "; ")))?;
    let report_path = std::env::var_os(sim_bench::REPORT_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    Ok(Some(Sim::new(path, matched, report_path)))
}

/// `sim_bench::config::Skill` as the bots crate's.
pub fn skill(s: sim_bench::config::Skill) -> bots::mind::Skill {
    match s {
        sim_bench::config::Skill::Easy => bots::mind::Skill::Easy,
        sim_bench::config::Skill::Normal => bots::mind::Skill::Normal,
        sim_bench::config::Skill::Heroic => bots::mind::Skill::Heroic,
        sim_bench::config::Skill::Legendary => bots::mind::Skill::Legendary,
    }
}

/// A tuning file's numbers as the bots crate takes them.
pub fn tuning(p: &BotParams) -> bots::tuning::Tuning {
    let work = bots::tuning::WorkRates::default();
    let rate = |value: Option<u32>, default| value.unwrap_or(default).clamp(1, 30);
    bots::tuning::Tuning {
        reaction: p.reaction,
        error: p.error,
        lead: p.lead,
        gain: p.gain,
        fire_cone: p.fire_cone_deg.map(f32::to_radians),
        burst: p.burst,
        burst_on: p.burst_on,
        burst_off: p.burst_off,
        grenades_per_minute: p.grenades_per_minute,
        awareness: p.awareness,
        aggression: p.aggression,
        push_bias: 0.0,
        twitchy: false,
        caution: p.caution,
        teamwork: p.teamwork,
        objective_focus: p.objective_focus,
        perception_ticks: p.perception_ticks,
        motor_ticks: p.motor_ticks,
        button_sigma_ms: p.button_sigma_ms,
        stick_step: p.stick_step,
        thumb_switch_ticks: p.thumb_switch_ticks,
        phantom_ping_ms: p.phantom_ping_ms,
        phantom_jitter_ms: p.phantom_jitter_ms,
        aim_model: p
            .aim_model
            .as_deref()
            .and_then(bots::tuning::AimModel::parse),
        work: bots::tuning::WorkRates {
            foot_plan_ticks: rate(p.foot_plan_ticks, work.foot_plan_ticks),
            vehicle_plan_ticks: rate(p.vehicle_plan_ticks, work.vehicle_plan_ticks),
            vehicle_lookahead_ticks: rate(p.vehicle_lookahead_ticks, work.vehicle_lookahead_ticks),
        },
        bounce: p.bounce.as_ref().map(bounce_tuning),
    }
}

/// A tuning file's `[bounce]` over the built-in route
/// (`bots::tuning::BOUNCE_DEFAULT`; docs/game/bots.md, "Tuning the
/// bounce"): a field left out plays the built-in value.
pub fn bounce_tuning(p: &sim_bench::config::BounceParams) -> bots::tuning::BounceTuning {
    let d = bots::tuning::BOUNCE_DEFAULT;
    macro_rules! over {
        ($($f:ident),* $(,)?) => {
            bots::tuning::BounceTuning { $( $f: p.$f.unwrap_or(d.$f), )* }
        };
    }
    over!(
        approach,
        approach_speed,
        settle,
        settle_ticks,
        settle_gain,
        settle_min,
        settle_max,
        still_ticks,
        jump_ticks,
        drift,
        bounce_tries,
        crouch_first,
        crouch_ticks,
        release_ticks,
        in_speed,
        push_in,
        start_out,
        approach_turn_deg,
        out_speed,
        line_up,
        line_up_ticks,
        run_off_speed,
        air_gain,
        drop_tries,
        fly_speed,
    )
}

/// A bot's practice event as the sim's bounce tally takes it, with its
/// game tick.
pub fn practice_event(e: &bots::mind::PracticeEvent) -> (u32, sim_bench::bounce::Event) {
    use bots::mind::PracticeEvent as P;
    use sim_bench::bounce::Event;
    match e {
        P::Started { site, tick } => (*tick, Event::Started { site: *site }),
        P::Primed {
            site,
            tick,
            read,
            own,
        } => (
            *tick,
            Event::Primed {
                site: *site,
                read: *read,
                own: *own,
            },
        ),
        P::Tried {
            site,
            tick,
            bounced,
            why,
        } => (
            *tick,
            Event::Tried {
                site: *site,
                bounced: *bounced,
                why: why.map(str::to_string),
            },
        ),
        P::Intent {
            site,
            tick,
            monster,
            links,
            fumble,
            again,
        } => (
            *tick,
            Event::Intent {
                site: *site,
                monster: *monster,
                links: *links,
                fumble: fumble.map(str::to_string),
                again: *again,
            },
        ),
        P::Chained {
            site,
            tick,
            tried,
            made,
        } => (
            *tick,
            Event::Chained {
                site: *site,
                tried: *tried,
                made: *made,
            },
        ),
        P::Yielded {
            site,
            tick,
            waited,
            gave_up,
        } => (
            *tick,
            Event::Yielded {
                site: *site,
                waited: *waited,
                gave_up: *gave_up,
            },
        ),
        P::SteppedAside { tick } => (*tick, Event::SteppedAside),
    }
}

/// A medal award as the sim's bounce tally takes it: the player (the
/// award's slot), its game tick and the event. A Bounced award ends a
/// chain; every other is a bounce `chain` into its chain.
pub fn award_event(a: &crate::superbounce_chain::Award) -> (u8, u32, sim_bench::bounce::Event) {
    use hud_kit_core::views::superbounce::Medal;
    let e = if a.medals.contains(&Medal::Bounced) {
        sim_bench::bounce::Event::Bounced { chain: a.chain }
    } else {
        sim_bench::bounce::Event::Chain { chain: a.chain }
    };
    (a.slot, a.tick, e)
}

/// What the brain is doing, as the report names it.
pub fn doing_name(d: &bots::mind::Doing) -> &'static str {
    bots::stats::goal_name(*d)
}

/// One tick's telemetry snapshot as the tally takes it.
pub fn tick_sample(s: &telemetry::Snapshot) -> TickSample {
    let players = s
        .players
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let o = s.objects.get(i).and_then(Option::as_ref);
            let alive = o.is_some_and(|o| o.vitality.is_none_or(|(body, _)| body > 0.0));
            PlayerSample {
                index: (p.player.index & 0xff) as u8,
                name: p.player.name.clone(),
                team: Some(p.player.team),
                unit: (p.unit != u32::MAX).then_some(p.unit),
                alive,
                in_vehicle: o.is_some_and(|o| o.vehicle.is_some()),
                grenades: o.and_then(|o| o.motion.grenades),
                score: p
                    .stats
                    .score
                    .clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
                kills: p.stats.kills.min(i32::MAX as u32) as i32,
                deaths: p.stats.deaths.min(i32::MAX as u32) as i32,
                assists: p.stats.assists.min(i32::MAX as u32) as i32,
                suicides: p.stats.suicides.min(i32::MAX as u32) as i32,
            }
        })
        .collect();
    TickSample {
        tick: (s.tick & 0xffff_ffff) as u32,
        players,
        teams: s.teams.iter().map(|t| (t.team, t.score)).collect(),
    }
}

/// Who held each fired weapon this tick: `(player index)` per shot whose
/// weapon a player's unit holds now.
pub fn shooters(s: &telemetry::Snapshot, fired: &[u32]) -> Vec<u8> {
    let mut out = Vec::new();
    for w in fired {
        let holder = s.players.iter().enumerate().find(|(i, _)| {
            s.objects
                .get(*i)
                .and_then(Option::as_ref)
                .and_then(|o| o.weapon.as_ref())
                .is_some_and(|wp| wp.datum == *w)
        });
        if let Some((_, p)) = holder {
            out.push((p.player.index & 0xff) as u8);
        }
    }
    out
}

/// Each player's height this tick, for the bounce tally's launches:
/// `(player index, z)` of every player with a unit.
pub fn heights(s: &telemetry::Snapshot) -> Vec<(u8, f32)> {
    s.players
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let o = s.objects.get(i).and_then(Option::as_ref)?;
            Some(((p.player.index & 0xff) as u8, o.pos[2] as f32))
        })
        .collect()
}

/// Match each seat's phantom machine ID to the unit the game spawned for
/// its player.
fn seat_bindings(
    m: &Match,
    machines: &BTreeMap<u8, [u8; 6]>,
    snap: &telemetry::Snapshot,
) -> BTreeMap<u8, u8> {
    let mut bindings = BTreeMap::new();
    for b in &m.bots {
        let Some((i, p)) = snap
            .players
            .iter()
            .enumerate()
            .find(|(_, p)| p.local_user.is_none() && machines.get(&b.seat) == Some(&p.machine))
        else {
            continue;
        };
        if p.unit == u32::MAX
            || snap.objects.get(i).and_then(Option::as_ref).is_none()
            || (m.team_play && b.team.map(i32::from) != Some(p.player.team))
        {
            continue;
        }
        if let Ok(index) = u8::try_from(p.player.index) {
            bindings.insert(b.seat, index);
        }
    }
    bindings
}

/// The player whose unit is `object`, from the last sample.
fn player_of_unit(last: Option<&TickSample>, object: u32) -> Option<u8> {
    last?
        .players
        .iter()
        .find(|p| p.unit == Some(object))
        .map(|p| p.index)
}

/// The player a fired weapon's shot counts for. Mounted fire (a vehicle
/// or turret weapon) is the seated player, resolved at once from the
/// players array by the weapon's own seat flags
/// ([`bots::vehicle::read::firing_player`]); a handheld weapon's holder
/// comes from the last sample, and when it is not in one yet the caller
/// defers the shot to the next tick's snapshot ([`shooters`]).
fn shot_of(
    fired_by: Option<bots::vehicle::read::FiredBy>,
    last: Option<&TickSample>,
) -> Option<u8> {
    match fired_by {
        Some(bots::vehicle::read::FiredBy::Seated(player)) => u8::try_from(player).ok(),
        Some(bots::vehicle::read::FiredBy::Holder(unit)) => {
            player_of_unit(last, unit.raw())
        }
        None => None,
    }
}

/// A kill's attacker or victim datum as a player index (`None` for none).
fn player_of_datum(d: u32) -> Option<u8> {
    (d != u32::MAX).then_some((d & 0xffff) as u8)
}

fn speed(inner: &Inner, s: &State) -> sim_bench::clock::SpeedMeter {
    let now = Instant::now();
    let first = s.first_tick.unwrap_or(now);
    let last = s.last_tick.unwrap_or(first);
    sim_bench::clock::SpeedMeter {
        setup_s: first.duration_since(inner.created).as_secs_f64(),
        match_s: last.duration_since(first).as_secs_f64(),
        ticks: s.tally.ticks(),
    }
}

impl Sim {
    pub fn new(path: PathBuf, matched: Match, report_path: Option<PathBuf>) -> Sim {
        let frame = SimFrame::new(matched.config.render_every)
            .on_skip(crate::presents::note_skipped_present);
        let created = Instant::now();
        let mut tally = Tally::new();
        let detail = matched.config.detail();
        tally.set_detail(detail.tally());
        let spatial = detail.traces().then(|| {
            sim_bench::spatial::Recorder::new(sim_bench::spatial::Header {
                map: matched.config.map.clone(),
                game_type: matched.game_type_name().into(),
                seed: matched.config.seed,
                team_play: matched.team_play,
                ..Default::default()
            })
        });
        Sim(Arc::new(Inner {
            path,
            hosted_map: Default::default(),
            lobby: Default::default(),
            matched,
            report_path,
            frame,
            state: Mutex::new(State {
                machines: Default::default(),
                bindings: Default::default(),
                tally,
                phase: Phase::Waiting,
                end: None,
                first_tick: None,
                last_tick: None,
                dispatch_start: None,
                dispatch_end: None,
                costs_start: None,
                set_up: false,
                spawn_seeded: false,
                fired: Vec::new(),
                notes: Vec::new(),
                cow: None,
                file_reads: vfs::GuestReadBytes::default(),
                setup: SetupClock::new(created),
                pages: Pages::default(),
                lean: WallLean::default(),
                spatial,
            }),
            stop: AtomicBool::new(false),
            created,
        }))
    }

    pub fn matched(&self) -> &Match {
        &self.0.matched
    }

    /// Resolve the match's multiplayer map for the System Link lobby: a
    /// disc map, else an installed Firefight kit package ([`kit_request`]).
    pub fn select_map(&self, disc: &Path, data_root: &Path) -> Result<()> {
        let m = &self.0.matched;
        let req = level_boot::resolve_in(
            disc,
            Some(data_root),
            &m.config.map,
            level_boot::Difficulty::Normal,
        )
        .or_else(|e| kit_request(&m.config.map).ok_or(e))
        .with_context(|| format!("sim: map {:?}", m.config.map))?;
        if req.kind != level_boot::Kind::Multiplayer {
            bail!(
                "sim: {} is a campaign level; a match needs a multiplayer map",
                req.name
            );
        }
        anyhow::ensure!(
            req.map != u32::MAX,
            "sim: no retail lobby map ID for {}",
            req.name
        );
        self.0
            .hosted_map
            .set(req)
            .map_err(|_| anyhow::anyhow!("sim: hosted map already selected"))
    }

    /// Register each brain as a phantom peer: every bot is one, and the
    /// host console runs the game playerless.
    pub fn add_bots(&self) -> Result<()> {
        bots::phantom::global().set_playerless_host(true);
        if let Some(f) = &self.0.matched.firefight {
            // Firefight (docs/runtime/sim.md, "Firefight"): the runtime
            // reads the match file's choices as it reads Game Setup's page,
            // from a store of their own; the bots sit on the player teams
            // the match file resolved.
            let shared = crate::firefight::feature::AgentSource::installed()
                .0
                .context("sim: a Firefight match needs the firefight feature, which is off")?;
            let store = Arc::new(firefight_store(f)?);
            shared
                .lock()
                .set_options(crate::firefight::options::source(&store));
            *lock_ended() = None;
        }
        // The match's script (docs/runtime/sim.md, "Rules scripts"): the
        // block of a lobby choosing that game type, on the game's record
        // at its start.
        if let Some(rules) = &self.0.matched.config.rules {
            let block = crate::sim_rules::block_for(rules).map_err(anyhow::Error::msg)?;
            crate::game_variants::inject::set_sim_block(Some(block));
        }
        if self.0.matched.superbounce {
            // Bounce practice (docs/runtime/sim.md, "Bounce practice"): the
            // game plays the Superbounce script, which turns the mode on;
            // the bots' route reads its pair word, and the tally counts
            // the bounces.
            crate::superbounce_mode::collect_awards(true);
            crate::sim_bots::install_pair_reader();
            self.0.lock().tally.track_bounces();
            // The bounce drill (docs/runtime/sim.md, "The bounce drill"):
            // one site, each try from its start with the word set to -3
            // (the "sim: drill word" hook writes it).
            bots::drill::set(self.0.matched.config.drill.map(|d| bots::drill::Drill {
                site: d.site,
                start: d.start,
            }));
            // Bounce site training (docs/runtime/sim.md, "Training the
            // bounce sites"): the bots fly the board's sites, this run's
            // shard, with the same word hook as the drill's.
            bots::learn::set(self.0.matched.config.train.map(|t| bots::learn::Train {
                shard: t.shard,
                shards: t.shards,
                budget: t.budget,
                seed: t.seed,
            }));
        }
        if let Some(d) = &self.0.matched.config.drops {
            // Seam drops (docs/runtime/sim.md, "Seam drops"): every seat
            // is placed, primed and dropped by the "sim: drops" hook, its
            // pad held, its brain ignored.
            let map = &self.0.matched.config.map;
            let job = sim_bench::drops::Job::new(d, map, self.0.matched.bots.len() as u32)
                .map_err(anyhow::Error::msg)?;
            eprintln!(
                "sim: drops: {} planned on {} targets, {} seats, control {}",
                job.planned(),
                job.targets().len(),
                self.0.matched.bots.len(),
                job.gate().name()
            );
            crate::sim_drops::set(map, job);
        }
        if let Some(b) = &self.0.matched.config.bounces {
            // The bounce search (docs/runtime/bounce-search.md): every
            // seat is driven by the "sim: bounces" hook, its pad held, its
            // brain ignored.
            let map = &self.0.matched.config.map;
            let job = sim_bench::bounces::Job::new(b, map, self.0.matched.bots.len() as u32)
                .map_err(anyhow::Error::msg)?;
            eprintln!(
                "sim: bounces: at most {} trials on {} sites in {} batches, {} seats",
                job.planned(),
                job.sites(),
                job.batches(),
                self.0.matched.bots.len()
            );
            crate::sim_bounces::set(map, job);
        }
        for b in &self.0.matched.bots {
            let brain = bots::Brain::new(skill(b.skill), b.seed)
                .with_name_profile(&b.name)
                .with_tuning(tuning(&b.params));
            let source = Metered {
                seat: b.seat,
                brain,
                sim: Arc::clone(&self.0),
                last_tick: None,
                thinking: None,
            };
            let id = bots::phantom::Identity {
                xuid: live_proto::bots::local_xuid(u32::from(b.seat) + 1).0,
                gamertag: b.name.clone(),
            };
            let handle =
                bots::phantom::add(id, b.team, Box::new(source)).map_err(anyhow::Error::msg)?;
            self.0
                .lock()
                .machines
                .insert(b.seat, bots::phantom::machine_id(handle));
        }
        Ok(())
    }

    /// The pacer for the match's pace (`None`: unpaced).
    pub fn pacer(&self) -> Option<kernel::pacer::Pacer> {
        let tps = self.0.matched.pace.ticks_per_second()?;
        let mut p = kernel::pacer::Pacer::real_time();
        p.ticks_per_second = tps;
        Some(p)
    }

    /// The frame skip shared with a dedicated host. The report reads its counters.
    pub fn frame(&self) -> &SimFrame {
        &self.0.frame
    }

    /// Match-specific hooks. Boot installs the light profile separately.
    pub fn install(&self, features: &mut hooks::Registry) -> Result<()> {
        self.install_with_film(features, None)
    }

    pub fn install_with_film(
        &self, features: &mut hooks::Registry, film_playback: Option<bool>,
    ) -> Result<()> {
        self.install_with_audio(features, film_playback, None)
    }

    /// Include the actual DirectSound scheduling and delivery sequence in
    /// the arena verification arm's full component series.
    pub fn install_with_audio(
        &self,
        features: &mut hooks::Registry,
        film_playback: Option<bool>,
        audio: Option<Arc<dsound::Hle>>,
    ) -> Result<()> {
        features
            .add(&SimMetrics { sim: Arc::clone(&self.0), film_playback, audio })
            .map_err(|e| anyhow::anyhow!("sim metrics: {e}"))?;
        if self.0.matched.config.bounces.is_some() {
            // The search's teleports go the drops' way.
            match crate::superbounce_verify::calls() {
                Some(q) => crate::sim_bounces::set_calls(q.clone()),
                None => {
                    let q = agent::sim::calls::shared();
                    match features.add(&agent::sim::calls::CallsFeature(q.clone())) {
                        Ok(true) => crate::sim_bounces::set_calls(q),
                        Ok(false) => eprintln!(
                            "sim: bounces: agent-calls is switched off by H2_FEATURES; no trial can be placed"
                        ),
                        Err(e) => bail!("sim: bounces: agent-calls: {e}"),
                    }
                }
            }
        }
        if self.0.matched.config.drops.is_some() {
            // The drops teleport through the agent's guest calls: its
            // queue when the agent is on, else one of our own.
            match crate::superbounce_verify::calls() {
                Some(q) => crate::sim_drops::set_calls(q.clone()),
                None => {
                    let q = agent::sim::calls::shared();
                    match features.add(&agent::sim::calls::CallsFeature(q.clone())) {
                        Ok(true) => crate::sim_drops::set_calls(q),
                        Ok(false) => eprintln!(
                            "sim: drops: agent-calls is switched off by H2_FEATURES; no drop can be placed"
                        ),
                        Err(e) => bail!("sim: drops: agent-calls: {e}"),
                    }
                }
            }
        }
        eprintln!(
            "sim: {} on {}, {} bot(s), {} ({})",
            self.0.path.display(),
            self.0.matched.config.map,
            self.0.matched.bots.len(),
            self.0.matched.game_type_name(),
            self.0.matched.pace.label()
        );
        Ok(())
    }

    /// Between slices: false once the match is over, stopped, or past its
    /// wall-clock limit (counted from the boot, so a run that never starts
    /// still ends).
    pub fn keep_running(&self) -> bool {
        let inner = &self.0;
        let binding_timeout =
            matches!(inner.lock().phase, Phase::Waiting | Phase::Binding).then_some(180);
        if let Some(limit) = inner
            .matched
            .config
            .max_wall_s
            .into_iter()
            .chain(binding_timeout)
            .min()
            && inner.created.elapsed().as_secs() >= limit
            && !inner.stop.load(Ordering::Acquire)
        {
            let mut s = inner.lock();
            inner.end(&mut s, End::WallLimit);
        }
        !inner.stop.load(Ordering::Acquire)
    }

    /// Current bound players, sampled by the private Live training transport.
    pub fn live_combat(&self) -> Vec<live_proto::game::SimCombatSample> {
        let seats: Vec<_> = {
            let s = self.0.lock();
            if s.phase != Phase::Playing {
                return Vec::new();
            }
            s.bindings.iter().filter_map(|(seat, &index)| {
                let handle = bots::phantom::handle_of(*s.machines.get(seat)?)?;
                Some((index, handle))
            }).collect()
        };
        // The worker reads one completed decision per seat. It releases the
        // tally first, since Metered can hold a brain while updating the tally.
        seats.into_iter().filter_map(|(index, handle)| {
            let stats = bots::phantom::global().combat_stats(handle)?;
            combat_sample(index, &stats, crate::bot_accuracy::rows(handle))
        }).collect()
    }

    /// The report as it stands (`end`: the match's, else `Aborted`).
    pub fn report(&self) -> Report {
        let inner = &self.0;
        // Metered holds the pad source while recording to the sim tally.
        // Release the tally before locking a source to read its diagnostics.
        let (mut report, machines) = {
            let state = inner.lock();
            (build_report(inner, &state), state.machines.clone())
        };
        for player in &mut report.players {
            let Some(handle) = player.seat
                .and_then(|seat| machines.get(&seat).copied())
                .and_then(bots::phantom::handle_of)
            else {
                continue;
            };
            player.weapon_accuracy = crate::bot_accuracy::rows(handle);
            player.combat = bots::phantom::global().combat_stats(handle)
                .map(|stats| json!(stats));
        }
        report.drops = crate::sim_drops::report();
        report.bounces = crate::sim_bounces::report();
        report
    }

    /// The guest RAM census at the match's end, for the report's `wall`
    /// block: `(written, total)` pages over a shared copy-on-write base.
    pub fn note_memory(&self, written: Option<(u64, u64)>, reads: vfs::GuestReadBytes) {
        let mut s = self.0.lock();
        s.cow = written;
        s.file_reads = reads;
    }

    /// The census and lean fields read at the run's end (`wall.reads`,
    /// `wall.heap_tags`, `wall.guest_pages_written_*`, `wall.data_dir_bytes`):
    /// what the guest read by path, the guest's RAM, and the data directory.
    pub fn note_lean(
        &self,
        guest: &runtime::Guest,
        reads: BTreeMap<String, vfs::PathReads>,
        data_dir: &Path,
    ) {
        let end = {
            let s = self.0.lock();
            s.pages.base.is_some().then(|| crate::sim_pages::hashes(guest))
        };
        let mut s = self.0.lock();
        let written = s
            .pages
            .base
            .as_deref()
            .zip(end.as_deref())
            .and_then(|(base, end)| crate::sim_pages::written(base, end));
        s.lean.guest_pages_written_4k = written.map(|w| w.0);
        s.lean.guest_pages_written_16k = written.map(|w| w.1);
        s.lean.reads = reads
            .into_iter()
            .map(|(path, r)| {
                (
                    path,
                    ReadStat {
                        mapped_bytes: r.mapped_bytes,
                        copied_bytes: r.copied_bytes,
                        count: r.reads,
                        refused: r.refusal.map(str::to_owned),
                    },
                )
            })
            .collect();
        s.lean.heap_tags = mem_watch::heap::totals()
            .0
            .into_iter()
            .filter(|(_, bytes)| *bytes > 0)
            .collect();
        s.lean.data_dir_bytes = crate::host_mem::dir_bytes(data_dir);
    }

    /// The report at the end of the run, written to `H2_SIM_REPORT` when
    /// set, and its summary on stderr.
    pub fn finish(&self) -> Result<Report> {
        // The drops' proven targets into the run's verified file, before
        // the report says where.
        if let Some(note) = crate::sim_drops::save() {
            eprintln!("sim: {note}");
            self.0.lock().notes.push(note);
        }
        if let Some(note) = crate::sim_bounces::save() {
            eprintln!("sim: {note}");
            self.0.lock().notes.push(note);
        }
        let mut r = self.report();
        if let Some(note) = self.write_trace(&r) {
            eprintln!("sim: {note}");
            r.notes.push(note);
        }
        eprintln!(
            "sim: {} after {} ticks ({:.0} game s) in {:.1} s: {:.2}x real time ({:.2}x with the boot)",
            r.result.end.name(),
            r.ticks,
            r.game_seconds,
            r.wall.match_s,
            r.wall.speedup,
            r.wall.process_speedup
        );
        for p in &r.players {
            eprintln!(
                "sim:   {:<12} score {:>3}  kills {:>3}  deaths {:>3}  accuracy {:.2}  stuck {:.0} s",
                p.name,
                p.score,
                p.kills,
                p.deaths,
                p.accuracy,
                p.stuck_ticks as f64 / 30.0
            );
        }
        if let Some(path) = &self.0.report_path {
            let text = serde_json::to_string_pretty(&r)?;
            std::fs::write(path, text)
                .with_context(|| format!("writing the report {}", path.display()))?;
            eprintln!("sim: report {}", path.display());
        }
        // A read recording (H2_SIM_STORE_RECORD) is written with the
        // report, merged into what the file already holds.
        vfs::sim_store::record::flush();
        // A control that did not bounce: the run's named error.
        if let Some(e) = crate::sim_drops::error() {
            bail!("sim: drops: {e}");
        }
        // A fork that failed: the search's named error.
        if let Some(e) = crate::sim_bounces::error() {
            bail!("sim: bounces: {e}");
        }
        if self.0.lock().bindings.len() != self.matched().bots.len() {
            bail!(
                "sim: match ended before all {} seats had a player and a spawned unit",
                self.matched().bots.len()
            );
        }
        Ok(r)
    }

    /// The spatial trace, written beside the report
    /// ([`sim_bench::spatial::trace_path`]) and checked against its kills;
    /// what happened, as a note. `None` when the run does not trace.
    fn write_trace(&self, report: &Report) -> Option<String> {
        let recorder = self.0.lock().spatial.take()?;
        let Some(report_path) = &self.0.report_path else {
            return Some("spatial: no report path, so no trace was written".into());
        };
        let ground = spatial::MapGround::current();
        let trace = recorder.finish(&ground, ground.mesh());
        let path = sim_bench::spatial::trace_path(report_path);
        if let Err(e) = trace.write(&path) {
            return Some(format!("spatial: {e}"));
        }
        Some(match trace.agrees_with(&report.kills) {
            Ok(()) => format!(
                "spatial trace {} ({} events; its {} kills are the report's)",
                path.display(),
                trace.events.len(),
                report.kills.len()
            ),
            Err(e) => format!("spatial trace {}: {e}", path.display()),
        })
    }

    /// `match.status`, `match.report`, `match.stop` and `sim.guest_census`
    /// (docs/runtime/agent-api.md, "Sim matches").
    pub fn add_methods(&self, agent: &agent::Agent) {
        let me = self.clone();
        agent.add_method(method::MATCH_STATUS, Access::Read, move |_| Ok(me.status()));
        let me = self.clone();
        agent.add_method(method::MATCH_REPORT, Access::Read, move |_| {
            serde_json::to_value(me.report())
                .map_err(|e| RpcError::new(code::INVALID_REQUEST, e.to_string()))
        });
        let me = self.clone();
        agent.add_method(method::MATCH_STOP, Access::Act, move |_| {
            let inner = &me.0;
            let mut s = inner.lock();
            let already = s.end;
            inner.end(&mut s, End::Stopped);
            Ok(json!({ "stopping": true, "end": s.end.map(End::name), "already_over": already.is_some() }))
        });
        let me = self.clone();
        agent.add_game_method(
            crate::sim_census::METHOD,
            Access::Read,
            move |params, ctx| {
                let now = ctx.read_guest(|m| crate::sim_census::read(m)).ok();
                let so_far = me.0.lock().tally.census();
                crate::sim_census::answer(&params, now, so_far)
            },
        );
    }

    /// What `match.status` answers.
    pub fn status(&self) -> Value {
        let inner = &self.0;
        let lobby = inner
            .lobby
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status();
        let s = inner.lock();
        let sp = speed(inner, &s);
        let f = inner.frame.stats();
        json!({
            "match": inner.path.display().to_string(),
            "map": inner.matched.config.map,
            "game_type": inner.matched.game_type_name(),
            "bots": inner.matched.bots.len(),
            "phase": s.phase.name(),
            "lobby": lobby,
            "bound_seats": s.bindings,
            "ticks": s.tally.ticks(),
            "max_ticks": inner.matched.config.max_ticks,
            "game_seconds": sp.game_seconds(),
            "wall_s": inner.created.elapsed().as_secs_f64(),
            "match_s": sp.match_s,
            "speedup": sp.speedup(),
            "frames": {
                "skipped": f.skipped,
                "rendered_world": f.rendered_world,
                "other": f.other,
                "stub_faults": f.stub_faults,
                "checked": f.checked,
                "mismatch_frame_counter": f.mismatch_frame_counter,
                "mismatch_one_shot": f.mismatch_one_shot,
                "mismatch_present_counter": f.mismatch_present_counter,
                "fell_back": f.fell_back,
            },
            "end": s.end.map(End::name),
            "drops": crate::sim_drops::status(),
            "bounces": crate::sim_bounces::status(),
        })
    }
}

fn combat_sample(
    index: u8, stats: &bots::stats::CombatStats, mut accuracy: Vec<live_proto::game::WeaponAccuracy>,
) -> Option<live_proto::game::SimCombatSample> {
    use live_proto::game::{MAX_LIVE_ACCURACY_ROWS, SimCombatCounters, SimCombatSample, SimDecision};
    let tick = stats.tick?;
    if accuracy.len() > MAX_LIVE_ACCURACY_ROWS {
        accuracy.sort_by_key(|row| std::cmp::Reverse(row.shots));
        accuracy.truncate(MAX_LIVE_ACCURACY_ROWS);
    }
    Some(SimCombatSample {
        index,
        tick,
        combat: SimCombatCounters {
            melee_presses: stats.melee_presses,
            grenade_requests: stats.grenade_requests,
            grenades_used: stats.grenades_used,
            switch_presses: stats.switch_presses,
            weapon_switches: stats.weapon_switches,
            reactions: stats.reactions,
            total_reaction_ticks: stats.total_reaction_ticks,
            think_ticks: stats.think_ticks,
            total_think_ms: stats.total_think_ms,
            goal: stats.goal.map(str::to_owned),
            weapon: stats.weapon,
            weapon_role: stats.weapon_role.map(str::to_owned),
            range: stats.range,
            think_ms: stats.think_ms,
            last_decisions: stats.last_decisions.back().into_iter().map(|decision| SimDecision {
                tick: decision.tick, action: decision.action.to_owned(),
            }).collect(),
        },
        weapon_accuracy: accuracy,
    })
}

fn build_report(inner: &Inner, s: &State) -> Report {
    let bots: Vec<BotInfo> = inner
        .matched
        .bots
        .iter()
        .map(|b| BotInfo {
            seat: b.seat,
            player: s.bindings.get(&b.seat).copied(),
            name: b.name.clone(),
            arm: b.arm.clone(),
        })
        .collect();
    let f = inner.frame.stats();
    let mut notes = s.notes.clone();
    if s.bindings.len() != inner.matched.bots.len() {
        let missing = inner
            .matched
            .bots
            .iter()
            .filter(|b| !s.bindings.contains_key(&b.seat))
            .map(|b| format!("{} (phantom seat {})", b.name, b.seat))
            .collect::<Vec<_>>();
        notes.push(format!(
            "waiting for player/unit bindings: {}",
            missing.join(", ")
        ));
    }
    notes.push(format!(
        "frames: {} world frames skipped, {} rendered (render_every {}), {} menu or loading frames",
        f.skipped, f.rendered_world, inner.matched.config.render_every, f.other
    ));
    if f.fell_back {
        notes.push(format!(
            "a rendered frame's tick writes differed from the skip's (frame counter {}, one-shot {}, present counter {} of {} checked); world frames rendered from then on",
            f.mismatch_frame_counter, f.mismatch_one_shot, f.mismatch_present_counter, f.checked
        ));
    }
    if f.stub_faults > 0 {
        notes.push(format!(
            "{} skipped frames could not write their stubs",
            f.stub_faults
        ));
    }
    if s.end.is_none() {
        notes.push(format!(
            "the match had not ended (phase {})",
            s.phase.name()
        ));
    }
    let nav = if bots::cache::current().is_some() {
        "loaded"
    } else {
        "missing"
    };
    let mut memory = crate::host_mem::usage();
    memory.mapped_read_bytes = s.file_reads.mapped;
    memory.copied_read_bytes = s.file_reads.copied;
    memory.guest_written_pages = s.cow.map(|(w, _)| w);
    memory.guest_pages = s.cow.map(|(_, t)| t);
    let mut report = s.tally.report(Finish {
        match_: inner.matched.resolved_json(),
        team_play: inner.matched.team_play,
        bots: &bots,
        end: s.end.unwrap_or(End::Aborted),
        speed: speed(inner, s),
        memory,
        nav,
        notes,
    });
    if report.ticks > 0
        && let Some((start, end)) = s.dispatch_start.zip(s.dispatch_end)
    {
        report.dispatch_per_tick = Some(
            end.since(start)
                .map(|(name, count)| (name.to_string(), count as f64 / report.ticks as f64))
                .collect(),
        );
    }
    if let Some(start) = s.costs_start {
        let now = costs_now();
        report.set_costs(
            now.bots.saturating_sub(start.bots),
            start.fixups.zip(now.fixups).map(|(a, b)| b.saturating_sub(a)),
        );
        report.set_phantom_costs(now.phantom.since(start.phantom));
        report.tick_phases = tick_phases(&start.ticks, &now.ticks);
    }
    let mut lean = s.lean.clone();
    lean.setup = Some(s.setup.phases(s.first_tick.unwrap_or_else(Instant::now)));
    report.set_lean(lean);
    report.rounds = round_scores(&bots);
    if let Some(f) = &inner.matched.firefight {
        use sim_bench::firefight::FirefightReport;
        report.firefight = Some(match lock_ended().as_ref() {
            Some(ended) => FirefightReport::ended(ended),
            None => FirefightReport::unfinished(f, &report.result.teams),
        });
    }
    report
}

/// The report's `rounds`: the rules runtime's score log, a bot's name on
/// the slot it holds (docs/runtime/sim.md, "Rounds").
fn round_scores(bots: &[BotInfo]) -> Vec<sim_bench::metrics::RoundScores> {
    use sim_bench::metrics::{RoundPlayer, RoundScores, RoundTeam};
    crate::sim_rounds::take()
        .into_iter()
        .enumerate()
        .map(|(i, s)| RoundScores {
            round: i as u32 + 1,
            tick: s.tick,
            players: s
                .players
                .into_iter()
                .map(|(slot, team, player_score)| RoundPlayer {
                    slot,
                    name: bots.iter().find(|b| b.player == Some(slot)).map(|b| b.name.clone()),
                    team,
                    player_score,
                })
                .collect(),
            teams: s.teams.into_iter().map(|(team, score)| RoundTeam { team, score }).collect(),
        })
        .collect()
}

// ---- Firefight (docs/runtime/sim.md, "Firefight") ----

/// The result the Firefight runtime reported this run ([`note_firefight`]).
static FIREFIGHT_ENDED: Mutex<Option<live_proto::firefight::FirefightEnded>> = Mutex::new(None);

fn lock_ended() -> MutexGuard<'static, Option<live_proto::firefight::FirefightEnded>> {
    FIREFIGHT_ENDED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Keeps the Firefight runtime's result for a sim report's `firefight`
/// section. Boot's report sink passes every report here as well as to Live
/// telemetry; only the result is kept, the last one.
pub fn note_firefight(report: &crate::firefight::runtime::Report) {
    if let crate::firefight::runtime::Report::Ended(ended) = report {
        *lock_ended() = Some(ended.clone());
    }
}

/// The Firefight page's choices for a match file's `[firefight]`, in a
/// store of their own over the page's settings
/// (`crate::firefight::options::DEFS`).
fn firefight_store(f: &sim_bench::firefight::Firefight) -> Result<settings::Store> {
    use sim_bench::firefight::Choice;
    let store = settings::Store::defaults(&crate::firefight::options::DEFS[..]);
    for (key, choice) in f.settings() {
        if store.def(key).is_none() {
            // The set's own teams needs no setting.
            if key == "firefight.teams" && choice == Choice::Int(0) {
                continue;
            }
            bail!("sim: this build has no {key} setting for the match's [firefight] table");
        }
        let value = match choice {
            Choice::Bool(b) => settings::Value::Bool(b),
            Choice::Int(i) => settings::Value::Int(i),
        };
        store
            .set(key, value)
            .map_err(|e| anyhow::anyhow!("sim: [firefight] as {key}: {e:?}"))?;
    }
    Ok(store)
}

/// The installed Firefight kit package a match file names, as the lobby's
/// map select lists it: by its package directory (`firefight-<map>`, as
/// `h2 firefight kit` installs it) or its list name (`Firefight – <map>`).
/// The request is its map list slot.
fn kit_request(name: &str) -> Option<level_boot::Request> {
    let catalogue = crate::map_select::current()?;
    let map = kit_map_named(catalogue.all(), name)?;
    Some(level_boot::Request {
        name: map.short_name(),
        // The game options' scenario path for a package is its slot path.
        scenario: map.path.clone(),
        campaign: u32::MAX,
        map: map.id,
        difficulty: level_boot::Difficulty::Normal,
        kind: level_boot::Kind::Multiplayer,
        variant: None,
        options: None,
    })
}

/// The kit map among `maps` that `name` names ([`kit_request`]).
fn kit_map_named<'a>(
    mut maps: impl Iterator<Item = &'a crate::map_select::Map>,
    name: &str,
) -> Option<&'a crate::map_select::Map> {
    let name = name.trim().trim_end_matches(".map");
    let lower = name.to_ascii_lowercase();
    let listed = lower
        .strip_prefix("firefight-")
        .map(|base| format!("{}{base}", crate::lobby_start::KIT_NAME_PREFIX));
    maps.find(|m| {
        crate::lobby_start::is_kit_map(m)
            && (m.name.eq_ignore_ascii_case(name)
                || listed.as_deref().is_some_and(|l| m.name.eq_ignore_ascii_case(l)))
    })
}

/// The tick driver's time per phase between two ledger reads: the report's
/// `tick_phases`.
fn tick_phases(
    start: &engine::sim_tick::Ledger,
    end: &engine::sim_tick::Ledger,
) -> Option<sim_bench::metrics::TickPhases> {
    sim_bench::metrics::TickPhases::over(
        end.ticks().saturating_sub(start.ticks()),
        end.tick_ns().saturating_sub(start.tick_ns()),
        end.loop_ns().saturating_sub(start.loop_ns()),
        end.loops().saturating_sub(start.loops()),
        engine::sim_tick::PHASES
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name.to_owned(), end.phase_ns(i).saturating_sub(start.phase_ns(i)))),
    )
}

/// The process's running cost totals, for the report's costs.
#[derive(Clone, Copy)]
struct Costs {
    /// Bot game-thread nanoseconds ([`bots::think::game_thread_ns`]).
    bots: u64,
    /// Flat window fixups (`None` with no window).
    fixups: Option<u64>,
    /// The phantoms' action build ([`crate::bot_glue::phantom_build_costs`]).
    phantom: sim_bench::metrics::PhantomCosts,
    /// The tick driver's ledger (`sim_bridge::tick::ledger`): time per phase.
    ticks: engine::sim_tick::Ledger,
}

fn costs_now() -> Costs {
    let w = runtime::window_process_stats();
    Costs {
        bots: bots::think::game_thread_ns(),
        fixups: (w.windows > 0).then_some(w.fixups),
        phantom: crate::bot_glue::phantom_build_costs(),
        ticks: sim_bridge::tick::ledger(),
    }
}

/// A bot's brain, counting per game tick what only it knows: stuck, its
/// repaths, its activity.
pub struct Metered {
    seat: u8,
    brain: bots::Brain,
    sim: Arc<Inner>,
    last_tick: Option<u32>,
    /// A split sample's tick and, once thought, its pad (bots::think): the
    /// count waits for the game thread's conclusion.
    thinking: Option<(u32, Option<HostPadState>)>,
}

impl PadSource for Metered {
    fn combat_stats(&self) -> Option<&bots::stats::CombatStats> {
        Some(self.brain.combat_stats())
    }

    fn brain_view(&self) -> Option<bots::BrainView> {
        Some(self.brain.view())
    }

    fn reset_input(&mut self) {
        self.brain.reset_input();
        self.last_tick = None;
        self.thinking = None;
    }

    fn order(&mut self, order: Option<bots::brain::Order>) -> bool {
        self.brain.set_order(order);
        true
    }

    // The seam drops and `superbounce.verify` drive a seat by holding its
    // pad (`sim_drops`); without this the brain's own pad goes out.
    fn hold_pad(&mut self, pad: Option<HostPadState>) -> bool {
        self.brain.hold_pad(pad)
    }

    fn phantom_latency(&self) -> bots::tuning::PhantomLatency {
        self.brain.phantom_latency()
    }

    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        if self.sim.lock().phase != Phase::Playing {
            return HostPadState::default();
        }
        let pad = self.brain.pad(f);
        self.meter(f.tick, pad);
        pad
    }

    fn observe(&mut self, f: &Frame<'_>) -> Sampled {
        if self.sim.lock().phase != Phase::Playing {
            return Sampled::Pad(HostPadState::default());
        }
        match self.brain.observe(f) {
            Sampled::Pad(pad) => {
                self.meter(f.tick, pad);
                Sampled::Pad(pad)
            }
            Sampled::Think => {
                self.thinking = Some((f.tick, None));
                Sampled::Think
            }
        }
    }

    fn think(&mut self) -> HostPadState {
        let pad = self.brain.think();
        if let Some((_, thought)) = &mut self.thinking {
            *thought = Some(pad);
        }
        pad
    }

    fn conclude(&mut self) {
        self.brain.conclude();
        if let Some((tick, Some(pad))) = self.thinking.take() {
            self.meter(tick, pad);
        }
    }
}

impl Metered {
    /// The brain's tick for the sim's tally: once per game tick, on the
    /// game thread.
    fn meter(&mut self, t: u32, pad: HostPadState) {
        if self.last_tick != Some(t) {
            self.last_tick = Some(t);
            let m = &self.brain.mind;
            let b = BrainTick {
                stuck: m.stuck.level != bots::stuck::Level::Moving,
                repaths: m.stuck.repaths,
                doing: doing_name(&m.doing),
                // The hand's last view of its target, with the stick the
                // pad reports this tick (after the human-input limits).
                aim: m.aim_trace().map(|a| sim_bench::metrics::AimSample {
                    tick: a.tick,
                    target: a.target,
                    error: a.error,
                    stick: pad.right_stick,
                }),
            };
            let events = if self.sim.matched.superbounce {
                self.brain.mind.take_practice_events()
            } else {
                Vec::new()
            };
            let traces = self.sim.matched.config.detail().traces();
            let toward = self.brain.mind.stuck.goal();
            let ground = spatial::MapGround::when(traces);
            let mut s = self.sim.lock();
            if s.phase == Phase::Playing
                && let Some(&player) = s.bindings.get(&self.seat)
            {
                s.tally.brain_for_player(player, b);
                if let Some(r) = s.spatial.as_mut() {
                    r.brain(player, b.stuck, b.repaths, toward, &ground);
                }
                for e in &events {
                    let (tick, e) = practice_event(e);
                    s.tally.bounce_event(player, tick, e);
                }
            }
        }
    }
}

/// The metrics feature (`sim-metrics`): read-only observers, and one
/// `before` on the game tick that writes the seed once a match.
struct SimMetrics {
    sim: Arc<Inner>,
    film_playback: Option<bool>,
    audio: Option<Arc<dsound::Hle>>,
}

impl hooks::Feature for SimMetrics {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("sim-metrics", "Sim mode: the match's metrics and its end")
            .doc("docs/runtime/sim.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> std::result::Result<(), hooks::BuildError> {
        hosted::install(b, Arc::clone(&self.sim))?;
        let policy = match self.film_playback {
            Some(playback) => film_state::policy(&self.sim, playback),
            None => hooks::SnapshotPolicy::Stateless,
        };
        b.snapshot(policy);
        let audio = self.audio.clone().filter(|_| {
            b.switches().implementation("sim.arena-hash", false)
                && self.sim.matched.config.detail().tally() == Detail::Full
                && self.sim.matched.config.components != Some(false)
        });
        let start_audio = audio.clone();
        let me = Arc::clone(&self.sim);
        b.game().on_start("sim: match start", move |o| {
            let g = o.guest();
            let mode = level_boot::active_mode(&*g);
            if mode != Some(level_boot::MODE_MULTIPLAYER) {
                return;
            }
            let tick = telemetry::game_tick(&g).unwrap_or(0);
            if let Some(req) = me.hosted_map.get() {
                if !level_boot::active_game(&*g)
                    .is_some_and(|(mode, path)| level_boot::is_wanted(Some(req), mode, &path))
                {
                    return;
                }
            }
            // The scenario's places, for the trace's spawns and pickups.
            let places = me
                .matched
                .config
                .detail()
                .traces()
                .then(|| spatial::places(&*g));
            drop(g);
            if let Some(hle) = &start_audio {
                hle.begin_voice_trace();
            }
            let mut s = me.lock();
            s.bindings.clear();
            s.tally.start((tick & 0xffff_ffff) as u32);
            s.trace_start((tick & 0xffff_ffff) as u32);
            if let Some(r) = s.spatial.as_mut() {
                match places {
                    Some(Ok((starts, items))) => r.set_places(starts, items),
                    Some(Err(e)) => eprintln!(
                        "sim: spatial: the scenario's places faulted at {:#x}",
                        e.addr
                    ),
                    None => {}
                }
            }
            s.phase = Phase::Binding;
            s.set_up = false;
            s.fired.clear();
            s.first_tick = None;
            s.dispatch_start = None;
            s.dispatch_end = None;
            s.costs_start = None;
            s.setup.enter(SetupStage::Bind, Instant::now());
            s.pages.base = None;
            s.pages.wanted = false;
            eprintln!("sim: match started at tick {tick}");
        });
        let me = Arc::clone(&self.sim);
        let arena_buffer = b
            .switches()
            .implementation("sim.arena-hash", false)
            .then(|| Mutex::new(Vec::new()));
        let projected = b.switches().implementation("sim.metrics-projection", true);
        let page_census = b.switches().implementation("sim.page-census", true);
        me.lock().pages.enabled = page_census;
        let full_detail = me.matched.config.detail().tally() == Detail::Full;
        let traces = me.matched.config.detail().traces();
        // The Havok heap walk (docs/engine/sim-rewrite-havok.md §9.2): the
        // arena hash cannot cover the heap, so its digest joins the named
        // components when the report has them.
        let havok_digest = full_detail && me.matched.config.components != Some(false);
        b.frame().on_tick("sim: tick sample", move |o| {
            if !matches!(me.lock().phase, Phase::Playing | Phase::Binding) {
                return;
            }
            let g = o.guest();
            // The baseline of the page census, the tick after counting
            // begins (the lock is not held while the pages are hashed).
            let page_base = (page_census && me.lock().pages.wanted)
                .then(|| crate::sim_pages::hashes(&g));
            if me.lock().phase == Phase::Binding
                && bots::phantom::session::host_session(&*g).is_err()
            {
                if let Some(hle) = &audio {
                    let tick = telemetry::game_tick(&g).unwrap_or(0) as u32;
                    drop(g);
                    let _ = hle.direct_sound().voice_events(tick);
                }
                return;
            }
            let snap = if projected {
                telemetry::sim_snapshot(&g)
            } else {
                telemetry::snapshot(&g)
            };
            let rng = g
                .u32(RANDOM_SEED_PTR)
                .ok()
                .filter(|p| *p != 0)
                .and_then(|p| g.u32(p).ok());
            let ended = g
                .u32(telemetry::layout::GAME_GLOBALS_PTR)
                .ok()
                .filter(|p| *p != 0)
                .and_then(|p| g.u8(p + telemetry::layout::GAME_ENDED).ok())
                .is_some_and(|b| b != 0);
            // With arena verification on, also hash it in fixed blocks so a
            // divergence names the first differing block.
            let mut arena_blocks = Vec::new();
            let arena = arena_buffer.as_ref().and_then(|buffer| {
                let mut bytes = buffer.lock().unwrap_or_else(|e| e.into_inner());
                lockstep::hash_sim_blocks(
                    &g,
                    lockstep::SimRegion::HALO2,
                    sim_bench::diff::ARENA_BLOCK as usize,
                    &mut bytes,
                    &mut arena_blocks,
                )
            });
            let (arena_base, arena) = match arena {
                Some((base, hash)) => (Some(base), Some(hash)),
                None => (None, None),
            };
            // A handful of reads (docs/runtime/sim.md §6, `guest_census`).
            let census = crate::sim_census::read(&*g);
            let voice_tick = audio.as_ref().map(|_| {
                snap.as_ref()
                    .map(|s| s.tick)
                    .unwrap_or_else(|_| telemetry::game_tick(&g).unwrap_or(0)) as u32
            });
            let havok = havok_digest
                .then(|| live_telemetry::havok_agents::digest::read(&*g).ok())
                .flatten();
            drop(g);
            let voice_events = audio.as_ref().zip(voice_tick)
                .and_then(|(hle, tick)| hle.direct_sound().voice_events(tick));
            let ground = spatial::MapGround::when(traces);
            let mut s = me.lock();
            if let Some(hashes) = page_base {
                s.pages.base = Some(hashes);
                s.pages.wanted = false;
            }
            match snap {
                Ok(snap) => {
                    let fired = std::mem::take(&mut s.fired);
                    let sample = tick_sample(&snap);
                    if s.phase == Phase::Binding {
                        s.bindings = seat_bindings(&me.matched, &s.machines, &snap);
                        if !s.set_up || s.bindings.len() != me.matched.bots.len() {
                            if ended {
                                me.end(&mut s, End::GameOver);
                            }
                            return;
                        }
                        s.phase = Phase::Playing;
                        let now = Instant::now();
                        s.first_tick = Some(now);
                        s.setup.enter(SetupStage::Done, now);
                        s.pages.wanted = s.pages.enabled;
                        s.tally.start(sample.tick);
                        s.trace_start(sample.tick);
                        s.dispatch_start = o.runtime.dispatch_counts();
                        s.costs_start = Some(costs_now());
                    }
                    let resets = s.tally.resets();
                    let tick = sample.tick;
                    s.tally
                        .tick_hashed(sample, state_hash::gameplay(&snap, rng), arena);
                    s.tally.tick_census(tick, &census);
                    // `components = false` in the match file leaves the
                    // breakdown out (a timing arm); the digests above stay.
                    // `detail = "training"` or "spatial" leaves it out too,
                    // with all but a sparse few of the digests.
                    if full_detail && me.matched.config.components != Some(false) {
                        let mut components = state_hash::components(&snap, rng);
                        if let Some(events) = voice_events {
                            components.push(("sound.voices".into(), events));
                        }
                        // The Havok walk names the first tick where physics
                        // differs when the arena hash cannot.
                        let havok = havok.map_or_else(
                            || "unreadable".to_owned(),
                            |d| {
                                format!(
                                    "hash={:016x} bodies={} constraints={} pairs={} heap={}",
                                    d.hash, d.bodies, d.constraints, d.pairs, d.heap_used
                                )
                            },
                        );
                        components.push(("havok.world".into(), havok));
                        s.tally
                            .tick_components_blocks(tick, arena_base, components, &arena_blocks);
                    }
                    if arena_buffer.is_some() && arena.is_none() {
                        const NOTE: &str = "arena hash unavailable on one or more sampled ticks";
                        if !s.notes.iter().any(|n| n == NOTE) {
                            s.notes.push(NOTE.into());
                        }
                    }
                    s.dispatch_end = o.runtime.dispatch_counts();
                    if s.tally.resets() != resets {
                        // A tick jump resets the match timer, shot IDs and seed.
                        crate::bot_accuracy::reset();
                        s.first_tick = Some(Instant::now());
                        s.dispatch_start = s.dispatch_end;
                        s.costs_start = Some(costs_now());
                        s.set_up = false;
                        s.pages.base = None;
                        s.pages.wanted = s.pages.enabled;
                        s.trace_start(tick);
                        eprintln!(
                            "sim: the tick jumped to {tick} (a snapshot load); counting from here"
                        );
                    }
                    for p in shooters(&snap, &fired) {
                        s.tally.shot(p);
                    }
                    s.trace_tick(tick, &snap, &ground);
                    if me.matched.superbounce {
                        for (index, z) in heights(&snap) {
                            s.tally.bounce_height(index, tick, z);
                        }
                        for a in crate::superbounce_mode::take_awards() {
                            let (player, at, e) = award_event(&a);
                            s.tally.bounce_event(player, at, e);
                            if a.monster() {
                                s.tally.bounce_event(
                                    player,
                                    at,
                                    sim_bench::bounce::Event::Monster,
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    if s.notes.len() < 16 {
                        s.notes.push(format!("a tick's snapshot failed: {e:?}"));
                    }
                }
            }
            s.last_tick = Some(Instant::now());
            if ended {
                me.end(&mut s, End::GameOver);
            } else if me
                .matched
                .config
                .max_ticks
                .is_some_and(|m| s.tally.ticks() >= m)
            {
                me.end(&mut s, End::TickLimit);
            } else if me.matched.config.train.is_some() && bots::learn::done() {
                // Every site of this run's shard is closed: nothing left
                // to train (the report's end is `stopped`).
                me.end(&mut s, End::Stopped);
            }
        });
        let me = Arc::clone(&self.sim);
        b.game().on_end("sim: match end", move |o| {
            let g = o.guest();
            let ended = g
                .u32(telemetry::layout::GAME_GLOBALS_PTR)
                .ok()
                .filter(|p| *p != 0)
                .and_then(|p| g.u8(p + telemetry::layout::GAME_ENDED).ok())
                .is_some_and(|b| b != 0);
            // The calls of the boundary that ended the game (a script's
            // last round's points, then `end_round`) were made before the
            // game's end and after the last tick's sample: the result
            // reads the scores as the game ended.
            let last = ended.then(|| telemetry::snapshot(&g).ok()).flatten();
            drop(g);
            let mut s = me.lock();
            if ended && s.phase == Phase::Playing {
                if let Some(snap) = last {
                    s.tally.tick(&tick_sample(&snap));
                }
                me.end(&mut s, End::GameOver);
            }
        });
        let me = Arc::clone(&self.sim);
        b.raw()
            .observe_before(&points::WEAPON_FIRE, "sim: shots", move |o| {
                if let Some(w) = o.arg_u32("weapon") {
                    // Who fires it: the biped that holds it, or the player
                    // seated at a vehicle or turret weapon (a Ghost's
                    // plasma, a Warthog's chaingun), so mounted fire
                    // counts as the seated player's shots too
                    // (docs/runtime/sim.md §6).
                    let fired_by = {
                        let g = o.guest();
                        bots::vehicle::read::firing_player(&*g, guest_layout::DatumHandle(w))
                            .ok()
                            .flatten()
                    };
                    let mut s = me.lock();
                    if s.phase == Phase::Playing {
                        if let Some(p) = shot_of(fired_by, s.tally.last()) {
                            s.tally.shot(p);
                        } else if s.fired.len() < MAX_FIRED {
                            s.fired.push(w);
                        }
                    }
                }
            })
            .optional();
        let me = Arc::clone(&self.sim);
        b.raw()
            .observe_before(&points::DAMAGE_REPORT, "sim: hits", move |o| {
                let (Some(report), Some(victim)) = (o.arg_u32("report"), o.arg_u32("victim"))
                else {
                    return;
                };
                let Ok(hit) = telemetry::hit(&o.guest(), report, victim) else {
                    return;
                };
                let ground = spatial::MapGround::when(traces);
                let mut s = me.lock();
                if s.phase != Phase::Playing {
                    return;
                }
                let target = player_of_unit(s.tally.last(), hit.victim);
                s.hit(
                    hit.tick,
                    player_of_datum(hit.owner_player),
                    target,
                    hit.body + hit.shield,
                    me.matched.team_play,
                    &ground,
                );
            })
            .optional();
        let me = Arc::clone(&self.sim);
        b.raw()
            .observe_before(&points::PLAYER_KILLED, "sim: kills", move |o| {
                let esp = o.cpu.esp;
                let read = {
                    let g = o.guest();
                    (|| {
                        let attacker = g.u32(esp.wrapping_add(4))?;
                        let victim = g.u32(esp.wrapping_add(8))?;
                        // A betrayal by the teams, as the bots tell it: the
                        // third argument is not a betrayal flag.
                        let betrayal = bots::emote::kill_from_guest(&*g, attacker, victim)
                            .ok()
                            .flatten()
                            .is_some_and(|k| k.betrayal);
                        let tick = telemetry::game_tick(&g)?;
                        Ok::<_, runtime::GuestFault>((attacker, victim, betrayal, tick))
                    })()
                };
                let Ok((attacker, victim, betrayal, tick)) = read else {
                    return;
                };
                let Some(v) = player_of_datum(victim) else {
                    return;
                };
                let ground = spatial::MapGround::when(traces);
                let mut s = me.lock();
                if s.phase == Phase::Playing {
                    s.kill(
                        (tick & 0xffff_ffff) as u32,
                        player_of_datum(attacker),
                        v,
                        betrayal,
                        &ground,
                    );
                }
            });
        // Spawn locations do not draw from the shared seed: `0x14ef00` mixes
        // the player index and spawn count with two words of the game
        // options (`[[0x4e6948]+0x10]`, `+0x14`), which the session fills
        // anew for each game. Written from the match seed ahead of a game's
        // first spawn, two runs of one match start their players in the
        // same places.
        let me = Arc::clone(&self.sim);
        b.raw()
            .observe_before(&points::GAME_ACTIVATE, "sim: new game", move |_| {
                me.lock().spawn_seeded = false;
            });
        let me = Arc::clone(&self.sim);
        b.raw()
            .before(&points::PLAYER_SPAWN, "sim: spawn words", move |c| {
                if std::mem::replace(&mut me.lock().spawn_seeded, true) {
                    return Ok(Flow::Continue);
                }
                let rt = Arc::clone(c.ctx.runtime());
                let mut g = rt.memory();
                if let Err(e) = write_spawn_words(&mut g, me.matched.config.seed) {
                    eprintln!("sim: the spawn words were not written: {e:?}");
                }
                Ok(Flow::Continue)
            })
            .first();
        let me = Arc::clone(&self.sim);
        b.raw()
            .before(&points::GAME_TICK, "sim: match seed", move |c| {
                {
                    let s = me.lock();
                    if !matches!(s.phase, Phase::Playing | Phase::Binding) || s.set_up {
                        return Ok(Flow::Continue);
                    }
                }
                let rt = Arc::clone(c.ctx.runtime());
                let mut g = rt.memory();
                let mut notes = Vec::new();
                let seed = write_seed(&mut g, me.matched.config.seed);
                let seeded = seed.is_ok();
                match seed {
                    Ok(v) => eprintln!("sim: seed {:#010x} written", v),
                    Err(e) => notes.push(format!("the seed was not written: {e:?}")),
                }
                drop(g);
                let mut s = me.lock();
                s.set_up = seeded;
                for n in notes {
                    eprintln!("sim: {n}");
                    if !s.notes.contains(&n) {
                        s.notes.push(n);
                    }
                }
                Ok(Flow::Continue)
            })
            .last();
        if self.sim.matched.superbounce
            && (self.sim.matched.config.drill.is_some() || self.sim.matched.config.train.is_some())
        {
            // The bounce drill's and the training's word: each bot at a
            // try's start asks (bots::drill::request_arm), and before the
            // next tick its unit's level pair word is set to -3, as
            // `havok.pair.set` writes it. A unit with no single level pair
            // (asleep, or airborne away from the level) is asked for again.
            let me = Arc::clone(&self.sim);
            b.raw()
                .before(&points::GAME_TICK, "sim: drill word", move |c| {
                    let asks = bots::drill::take_arm_requests();
                    if asks.is_empty() {
                        return Ok(Flow::Continue);
                    }
                    let rt = Arc::clone(c.ctx.runtime());
                    let mut g = rt.memory();
                    let mut written = 0u32;
                    for unit in asks {
                        if drill_arm(&mut *g, unit) {
                            written += 1;
                        }
                    }
                    drop(g);
                    if written > 0 {
                        let mut s = me.lock();
                        const NOTE: &str = "drill: the word was set to -3 at the start";
                        if !s.notes.iter().any(|n| n == NOTE) {
                            eprintln!("sim: {NOTE}");
                            s.notes.push(NOTE.into());
                        }
                    }
                    Ok(Flow::Continue)
                })
                .last();
        }
        if self.sim.matched.config.drops.is_some() {
            // Seam drops (docs/runtime/sim.md, "Seam drops"): every bound
            // seat's body and level pairs read, and its act (the word, the
            // pad, a teleport) carried out, before each tick. Last: after
            // the guest calls have made last tick's teleports.
            let me = Arc::clone(&self.sim);
            b.raw()
                .before(&points::GAME_TICK, "sim: drops", move |c| {
                    let seats: Vec<crate::sim_drops::Seat> = {
                        let s = me.lock();
                        if s.phase != Phase::Playing {
                            return Ok(Flow::Continue);
                        }
                        s.bindings
                            .iter()
                            .map(|(&seat, &player)| crate::sim_drops::Seat {
                                seat,
                                player,
                                handle: s
                                    .machines
                                    .get(&seat)
                                    .copied()
                                    .and_then(bots::phantom::handle_of),
                            })
                            .collect()
                    };
                    let rt = Arc::clone(c.ctx.runtime());
                    let mut g = rt.memory();
                    let done = crate::sim_drops::tick(&mut g, &seats);
                    drop(g);
                    if done {
                        let mut s = me.lock();
                        if let Some(e) = crate::sim_drops::error() {
                            let note = format!("drops: {e}");
                            if !s.notes.contains(&note) {
                                eprintln!("sim: {note}");
                                s.notes.push(note);
                            }
                        }
                        me.end(&mut s, End::Stopped);
                    }
                    Ok(Flow::Continue)
                })
                .last();
        }
        if self.sim.matched.config.bounces.is_some() {
            // The bounce search (docs/runtime/bounce-search.md): the
            // drops' reads and acts before the tick, and a fork when the
            // job asks, after it (below).
            let me = Arc::clone(&self.sim);
            b.raw()
                .before(&points::GAME_TICK, "sim: bounces", move |c| {
                    let seats: Vec<crate::sim_drops::Seat> = {
                        let s = me.lock();
                        if s.phase != Phase::Playing {
                            return Ok(Flow::Continue);
                        }
                        s.bindings
                            .iter()
                            .map(|(&seat, &player)| crate::sim_drops::Seat {
                                seat,
                                player,
                                handle: s
                                    .machines
                                    .get(&seat)
                                    .copied()
                                    .and_then(bots::phantom::handle_of),
                            })
                            .collect()
                    };
                    let rt = Arc::clone(c.ctx.runtime());
                    let mut g = rt.memory();
                    let ticked = crate::sim_bounces::tick(&mut g, &seats);
                    drop(g);
                    if ticked.done {
                        let mut s = me.lock();
                        if let Some(e) = crate::sim_bounces::error() {
                            let note = format!("bounces: {e}");
                            if !s.notes.contains(&note) {
                                eprintln!("sim: {note}");
                                s.notes.push(note);
                            }
                        }
                        me.end(&mut s, End::Stopped);
                    }
                    Ok(Flow::Continue)
                })
                .last();
            // The fork's slice ends after the tick returned, where
            // `state.save` acts (docs/runtime/snapshots.md, "The rule"):
            // inside the tick the game thread is in the tick chain's
            // after-hooks continuation, which no save can carry, so a
            // fork asked from the `before` above was refused every time.
            b.frame()
                .on_tick("sim: bounces fork", |o| {
                    if crate::sim_bounces::fork_due() {
                        o.runtime.request_preempt(runtime::preempt::REQUEST);
                    }
                })
                .last();
        }
        Ok(())
    }
}

/// The word a forced crouch leaves (docs/game/superbounce.md, "The pair
/// word"): -3.
const PRIMED_WORD: u32 = 0xffff_fffd;

/// Sets `unit`'s level pair word to -3 (`havok.pair.set`'s write). False
/// when it has no single pair against the level, or the write faulted.
fn drill_arm(m: &mut dyn guest_layout::GuestMemory, unit: u32) -> bool {
    use live_telemetry::havok_agents as hk;
    let Ok(Some(entry)) = hk::fixed_pair(&*m, unit) else {
        return false;
    };
    matches!(hk::set_pair_word(m, unit, entry, PRIMED_WORD), Ok(Some(_)))
}

/// The 32-bit seed for a match seed.
pub fn seed32(seed: u64) -> u32 {
    ((seed >> 32) as u32) ^ (seed as u32)
}

/// The game options' two spawn words (`[[0x4e6948]+0x10]`, `+0x14`) for a
/// match seed: the 32-bit seed, then the same of the seed's SplitMix64 step.
pub fn spawn_words(seed: u64) -> [u32; 2] {
    let mut x = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    [seed32(seed), seed32(x ^ (x >> 31))]
}

const GAME_OPTIONS_SPAWN_WORDS: u32 = 0x10;

fn write_spawn_words(g: &mut runtime::Guest, seed: u64) -> std::result::Result<(), runtime::GuestFault> {
    let globals = g.u32(telemetry::layout::GAME_GLOBALS_PTR)?;
    let [a, b] = spawn_words(seed);
    g.set_u32(globals + GAME_OPTIONS_SPAWN_WORDS, a)?;
    g.set_u32(globals + GAME_OPTIONS_SPAWN_WORDS + 4, b)
}

fn write_seed(g: &mut runtime::Guest, seed: u64) -> std::result::Result<u32, runtime::GuestFault> {
    let p = g.u32(RANDOM_SEED_PTR)?;
    let v = seed32(seed);
    g.set_u32(p, v)?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_words_follow_the_match_seed_alone() {
        assert_eq!(spawn_words(1), spawn_words(1));
        assert_eq!(spawn_words(1)[0], seed32(1));
        assert_ne!(spawn_words(1), spawn_words(2));
        assert_ne!(spawn_words(7)[0], spawn_words(7)[1]);
    }

    #[test]
    fn live_combat_uses_the_last_completed_decision_and_bounds_the_preview() {
        use bots::stats::{CombatStats, Decision};
        use live_proto::game::{MAX_LIVE_ACCURACY_ROWS, WeaponAccuracy};
        let mut stats = CombatStats::default();
        assert!(combat_sample(2, &stats, vec![]).is_none());
        stats.tick = Some(10);
        stats.weapon_switches = 2;
        stats.range = Some(6.0);
        stats.last_decisions.extend([
            Decision { tick: 8, action: "fight" }, Decision { tick: 10, action: "switch" },
        ]);
        let rows = (0..=MAX_LIVE_ACCURACY_ROWS).map(|i| WeaponAccuracy {
            weapon: "spray".into(), damage_code: (i % 64) as u8, range: "close".into(),
            hits: 0, shots: 1 + i as u64, accuracy: 0.0,
        }).collect();
        let sample = combat_sample(2, &stats, rows).unwrap();
        assert_eq!((sample.index, sample.tick), (2, 10));
        assert_eq!(sample.combat.weapon_switches, 2);
        assert_eq!(sample.combat.last_decisions.len(), 1);
        assert_eq!(sample.combat.last_decisions[0].action, "switch");
        assert_eq!(sample.weapon_accuracy.len(), MAX_LIVE_ACCURACY_ROWS);
        assert!(sample.weapon_accuracy.iter().all(|r| r.shots > 1));
        assert!(sample.valid());
    }

    #[test]
    fn the_bounce_fields_default_to_the_built_in_route() {
        use sim_bench::config::{BOUNCE_FIELDS, BounceParams};
        let d = bots::tuning::BOUNCE_DEFAULT;
        assert_eq!(bounce_tuning(&BounceParams::default()), d);
        let mut every = BounceParams::default();
        for f in BOUNCE_FIELDS {
            every.set(f.name, f.default).unwrap();
        }
        assert_eq!(bounce_tuning(&every), d, "sim_bench's defaults are the bots crate's");
        // Each field reaches the route.
        for f in BOUNCE_FIELDS {
            let mut one = BounceParams::default();
            let v = if f.max != f.default { f.max } else { f.min };
            one.set(f.name, v).unwrap();
            assert_ne!(bounce_tuning(&one), d, "bounce.{} is not carried", f.name);
        }
        let p = BotParams {
            bounce: Some(BounceParams {
                drift: Some(0.3),
                ..BounceParams::default()
            }),
            ..BotParams::default()
        };
        let t = tuning(&p).bounce.unwrap();
        assert_eq!(t.drift, 0.3);
        assert_eq!(t.approach, d.approach);
        assert_eq!(tuning(&BotParams::default()).bounce, None);
    }

    #[test]
    fn practice_events_and_awards_reach_the_bounce_tally() {
        use bots::mind::PracticeEvent as P;
        use hud_kit_core::views::superbounce::Medal;
        use sim_bench::bounce::Event;
        let site = [1.0, 2.0, 3.0];
        assert_eq!(
            practice_event(&P::Started { site, tick: 5 }),
            (5, Event::Started { site })
        );
        assert_eq!(
            practice_event(&P::Primed {
                site,
                tick: 6,
                read: true,
                own: false
            }),
            (
                6,
                Event::Primed {
                    site,
                    read: true,
                    own: false
                }
            )
        );
        assert_eq!(
            practice_event(&P::Tried {
                site,
                tick: 7,
                bounced: false,
                why: Some("too_slow")
            }),
            (
                7,
                Event::Tried {
                    site,
                    bounced: false,
                    why: Some("too_slow".into())
                }
            )
        );
        assert_eq!(
            practice_event(&P::Intent {
                site,
                tick: 8,
                monster: true,
                links: 2,
                fumble: Some("off_line"),
                again: false
            }),
            (
                8,
                Event::Intent {
                    site,
                    monster: true,
                    links: 2,
                    fumble: Some("off_line".into()),
                    again: false
                }
            )
        );
        assert_eq!(
            practice_event(&P::Chained {
                site,
                tick: 9,
                tried: 2,
                made: 1
            }),
            (
                9,
                Event::Chained {
                    site,
                    tried: 2,
                    made: 1
                }
            )
        );
        assert_eq!(
            practice_event(&P::Yielded {
                site,
                tick: 10,
                waited: 45,
                gave_up: true
            }),
            (
                10,
                Event::Yielded {
                    site,
                    waited: 45,
                    gave_up: true
                }
            )
        );
        assert_eq!(
            practice_event(&P::SteppedAside { tick: 11 }),
            (11, Event::SteppedAside)
        );
        let award = |chain, medals: Vec<Medal>| crate::superbounce_chain::Award {
            seq: 1,
            slot: 3,
            user: None,
            tick: 40,
            chain,
            medals,
            spot: None,
            strips: None,
        };
        assert_eq!(
            award_event(&award(
                2,
                crate::superbounce_chain::bounce_medals(2).to_vec()
            )),
            (3, 40, Event::Chain { chain: 2 })
        );
        assert_eq!(
            award_event(&award(4, vec![Medal::Bounced])),
            (3, 40, Event::Bounced { chain: 4 })
        );
    }

    #[test]
    fn tunings_carry_every_number() {
        let p = BotParams {
            reaction: Some(4),
            error: Some(0.01),
            lead: Some(0.9),
            gain: Some(3.0),
            fire_cone_deg: Some(180.0),
            burst: Some(true),
            burst_on: Some(6),
            burst_off: Some(2),
            grenades_per_minute: Some(1.5),
            awareness: Some(0.7),
            aggression: Some(0.1),
            caution: Some(0.2),
            teamwork: Some(0.3),
            objective_focus: Some(0.4),
            skill: Some("heroic".into()),
            perception_ticks: Some(3),
            motor_ticks: Some(4),
            button_sigma_ms: Some(1.5),
            stick_step: Some(0.25),
            thumb_switch_ticks: Some(2),
            phantom_ping_ms: Some(80.0),
            phantom_jitter_ms: Some(2.5),
            aim_model: Some("net".into()),
            foot_plan_ticks: Some(2),
            ..BotParams::default()
        };
        let t = tuning(&p);
        assert_eq!(t.aim_model, Some(bots::tuning::AimModel::Net));
        assert_eq!(
            t.work,
            bots::tuning::WorkRates {
                foot_plan_ticks: 2,
                ..Default::default()
            }
        );
        assert_eq!(
            tuning(&BotParams {
                aim_model: Some("code".into()),
                ..BotParams::default()
            })
            .aim_model,
            Some(bots::tuning::AimModel::Code)
        );
        assert_eq!(t.reaction, Some(4));
        assert!((t.fire_cone.unwrap() - std::f32::consts::PI).abs() < 1e-6);
        assert_eq!(
            (t.burst, t.burst_on, t.burst_off),
            (Some(true), Some(6), Some(2))
        );
        assert_eq!(t.objective_focus, Some(0.4));
        assert_eq!(
            t.input_limits(bots::mind::Skill::Legendary),
            bots::tuning::InputLimits {
                perception_ticks: 3,
                motor_ticks: 4,
                button_sigma_ms: 1.5,
                stick_step: 0.25,
                thumb_switch_ticks: 2,
            }
        );
        assert_eq!(
            t.phantom_latency(),
            bots::tuning::PhantomLatency {
                ping_ms: Some(80.0),
                jitter_ms: 2.5
            }
        );
        assert!(tuning(&BotParams::default()).is_empty());
    }

    #[test]
    fn skills_and_doings_map() {
        for (a, b) in [
            (sim_bench::config::Skill::Easy, bots::mind::Skill::Easy),
            (sim_bench::config::Skill::Legendary, bots::mind::Skill::Legendary),
        ] {
            assert_eq!(skill(a), b);
        }
        assert_eq!(doing_name(&bots::mind::Doing::Fight(3)), "fight");
        assert_eq!(doing_name(&bots::mind::Doing::Keep), "keep");
        let vantage = bots::places::Vantage {
            at: [0., 0., 2.],
            watches: [10., 0., 0.],
            score: 0.75,
        };
        assert_eq!(doing_name(&bots::mind::Doing::Vantage(vantage)), "vantage");
        for d in sim_bench::metrics::OBJECTIVE_DOINGS {
            assert!(["carry", "keep", "play"].contains(&d));
        }
    }

    fn player(index: u32, controller: Option<u8>, unit: u32) -> telemetry::PlayerState {
        telemetry::PlayerState {
            datum: index,
            player: live_proto::game::Player {
                index,
                name: format!("P{index}"),
                xuid: None,
                team: index as i32 % 2,
                local: Some(true),
                appearance: None,
                bot: None,
            },
            appearance: Default::default(),
            local_user: controller,
            controller,
            machine: [0; 6],
            unit,
            stats: telemetry::PlayerStats {
                score: 3,
                kills: 3,
                deaths: 1,
                assists: 0,
                suicides: 0,
            },
        }
    }

    fn object(
        weapon: Option<u32>,
        vehicle: Option<u32>,
        body: f64,
        grenades: u32,
    ) -> telemetry::ObjectState {
        telemetry::ObjectState {
            weapon: weapon.map(|datum| telemetry::Weapon {
                datum,
                definition: 0,
            }),
            vehicle,
            vitality: Some((body, 1.0)),
            motion: telemetry::Motion {
                grenades: Some(grenades),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn tick_hash_detects_motion_inventory_and_rng_before_score_changes() {
        let mut snap = telemetry::Snapshot {
            tick: 77,
            players: vec![player(0, Some(0), 0xe001)],
            objects: vec![Some(object(Some(0xa1), None, 1.0, 2))],
            ..Default::default()
        };
        let hash = |s: &telemetry::Snapshot| state_hash::gameplay(s, Some(7));
        let original = hash(&snap);
        snap.players[0].player.name = "renamed".into();
        assert_eq!(hash(&snap), original);
        assert_ne!(state_hash::gameplay(&snap, Some(8)), original);
        snap.objects[0].as_mut().unwrap().pos[0] = 0.5;
        let moved = hash(&snap);
        assert_ne!(moved, original);
        snap.objects[0].as_mut().unwrap().motion.magazine = Some((0, 0));
        assert_ne!(hash(&snap), moved);
    }

    #[test]
    fn snapshots_become_samples() {
        let snap = telemetry::Snapshot {
            tick: 77,
            players: vec![
                player(0, Some(0), 0xe001),
                player(1, Some(1), 0xe002),
                player(2, None, u32::MAX),
            ],
            teams: vec![live_proto::game::TeamScore { team: 0, score: 4 }],
            objects: vec![
                Some(object(Some(0xa1), None, 1.0, 2)),
                Some(object(Some(0xa2), Some(0xb1), 0.0, 0)),
                None,
            ],
            ..Default::default()
        };
        let s = tick_sample(&snap);
        assert_eq!(s.tick, 77);
        assert_eq!(s.teams, [(0, 4)]);
        let p0 = &s.players[0];
        assert_eq!(
            (
                p0.index,
                p0.unit,
                p0.alive,
                p0.in_vehicle,
                p0.grenades
            ),
            (0, Some(0xe001), true, false, Some(2))
        );
        assert_eq!((p0.score, p0.kills, p0.deaths), (3, 3, 1));
        let p1 = &s.players[1];
        assert!(!p1.alive, "body depleted");
        assert!(p1.in_vehicle);
        let p2 = &s.players[2];
        assert_eq!((p2.unit, p2.alive), (None, false));
        // Shots go to whoever holds the weapon now.
        assert_eq!(shooters(&snap, &[0xa2, 0xa1, 0xa1, 0xdead]), [1, 0, 0]);
        assert_eq!(player_of_unit(Some(&s), 0xe002), Some(1));
        assert_eq!(player_of_unit(Some(&s), 0x1234), None);
        assert_eq!(player_of_unit(None, 0xe002), None);
        assert_eq!(player_of_datum(u32::MAX), None);
        assert_eq!(player_of_datum(0xe5a4_0002), Some(2));
    }

    #[test]
    fn mounted_fire_is_the_seated_players_shot_and_handheld_waits_for_its_holder() {
        // The sample's unit is the holder's own raw datum, so the two
        // agree by construction (`player_of_unit` compares raws).
        let holder = guest_layout::DatumHandle::new(0xe, 1);
        let snap = telemetry::Snapshot {
            players: vec![
                player(0, None, holder.raw()),
                player(1, None, 0xe002),
            ],
            objects: vec![Some(object(None, None, 1.0, 0)), Some(object(None, None, 1.0, 0))],
            ..Default::default()
        };
        let s = tick_sample(&snap);
        use bots::vehicle::read::FiredBy;
        // A vehicle or turret weapon (a Ghost's plasma, a Warthog's
        // chaingun): the seated player's shot, resolved without a sample.
        assert_eq!(shot_of(Some(FiredBy::Seated(1)), None), Some(1));
        assert_eq!(shot_of(Some(FiredBy::Seated(u32::MAX)), None), None);
        // A handheld weapon: the holder biped's player from the sample,
        // else deferred to the next tick's held-weapon snapshot.
        assert_eq!(shot_of(Some(FiredBy::Holder(holder)), Some(&s)), Some(0));
        assert_eq!(shot_of(Some(FiredBy::Holder(holder)), None), None);
        // Nothing readable behind the weapon: deferred as before.
        assert_eq!(shot_of(None, Some(&s)), None);
    }

    #[test]
    fn bindings_require_the_phantom_machine_and_both_unit_objects() {
        let sim = sim_with("map='midship'\n[[bot]]\nkind='phantom'\n[[bot]]\n");
        let machines = BTreeMap::from([(0, [7; 6]), (1, [8; 6])]);
        let mut snap = telemetry::Snapshot {
            players: vec![player(6, None, 11), player(2, None, 12)],
            objects: vec![Some(object(None, None, 1., 0)), None],
            ..Default::default()
        };
        snap.players[0].machine = [8; 6];
        assert_eq!(
            seat_bindings(sim.matched(), &machines, &snap),
            BTreeMap::from([(1, 6)])
        );
        snap.objects[1] = snap.objects[0].clone();
        assert_eq!(
            seat_bindings(sim.matched(), &machines, &snap),
            BTreeMap::from([(1, 6)])
        );
        snap.players[1].machine = [7; 6];
        assert_eq!(
            seat_bindings(sim.matched(), &machines, &snap),
            BTreeMap::from([(0, 2), (1, 6)])
        );
        snap.players[1].unit = u32::MAX;
        assert_eq!(
            seat_bindings(sim.matched(), &machines, &snap),
            BTreeMap::from([(1, 6)])
        );
    }

    #[test]
    fn seeds_fold_to_32_bits() {
        assert_eq!(seed32(7), 7);
        assert_eq!(seed32(1 << 32), 1);
        assert_ne!(seed32(1), seed32(2));
    }

    /// A match file names a kit package by its directory or its list name;
    /// the disc map it is built on and the probe are not it.
    #[test]
    fn a_firefight_match_finds_its_kit_package() {
        use crate::map_select::Map;
        let map = |id, name: &str| Map {
            id,
            name: name.into(),
            sort: 0,
            path: format!("t:\\$C\\0000000{id}\\m-{id:08x}"),
        };
        let maps = [
            map(1, "Beaver Creek"),
            map(2, "Firefight Probe \u{2013} beavercreek"),
            map(3, "Firefight \u{2013} beavercreek"),
            map(4, "Firefight \u{2013} lockout"),
        ];
        for name in [
            "firefight-beavercreek",
            "Firefight-BeaverCreek.map",
            "Firefight \u{2013} beavercreek",
        ] {
            assert_eq!(kit_map_named(maps.iter(), name).map(|m| m.id), Some(3), "{name}");
        }
        assert!(kit_map_named(maps.iter(), "beavercreek").is_none());
        assert!(kit_map_named(maps.iter(), "firefight-midship").is_none());
    }

    /// The `[firefight]` table becomes the page's choices, which the
    /// runtime reads as it reads Game Setup's.
    #[test]
    fn a_firefight_table_becomes_the_pages_choices() {
        let sim = sim_with(
            "map = 'firefight-beavercreek'\ngame_type = 'firefight'\n\
             [firefight]\nset = 'attrition'\ndifficulty = 'heroic'\nlives = 8\nskulls = false\n\
             [[bot]]\n[[bot]]\n",
        );
        let store = firefight_store(sim.matched().firefight.as_ref().unwrap()).unwrap();
        let o = crate::firefight::options::Options::read(&store).unwrap();
        assert!(o.enabled);
        assert_eq!(o.wave_set, 1);
        assert_eq!(o.difficulty, ::firefight::Difficulty::Heroic);
        assert_eq!((o.lives, o.skulls, o.teams), (Some(8), false, None));
        // Crossfire, and Holdout played by two teams.
        for (table, wave_set) in [("set = 'crossfire'", 3), ("teams = 2", 0)] {
            let sim = sim_with(&format!(
                "map = 'firefight-beavercreek'\ngame_type = 'firefight'\n[firefight]\n{table}\n\
                 [[bot]]\n[[bot]]\n"
            ));
            let store = firefight_store(sim.matched().firefight.as_ref().unwrap()).unwrap();
            let o = crate::firefight::options::Options::read(&store).unwrap();
            assert_eq!(o.wave_set, wave_set, "{table}");
            let teams = if wave_set == 3 { None } else { Some(2) };
            assert_eq!(o.teams, teams, "{table}");
        }
    }

    fn sim_with(text: &str) -> Sim {
        sim_reporting(text, None)
    }

    fn sim_reporting(text: &str, report: Option<PathBuf>) -> Sim {
        let c = sim_bench::config::parse_match(text, false).unwrap();
        let m = sim_bench::config::resolve(c, &|_| Err("no files".into())).unwrap();
        Sim::new(PathBuf::from("/tmp/m.toml"), m, report)
    }

    /// Forty ticks of a two-player match through the observers' state
    /// helpers: inside tick 19 player 0 hits and kills player 1, who is
    /// back at 25.
    fn play_forty(s: &Sim) {
        let ground = sim_bench::spatial::NoGround;
        let mut st = s.0.lock();
        st.phase = Phase::Playing;
        st.tally.start(0);
        st.trace_start(0);
        for tick in 0..40u32 {
            let mut a = object(Some(0xa1), None, 1.0, 2);
            a.pos = [f64::from(tick) * 0.5, 0.0, 0.0];
            let down = (19..25).contains(&tick);
            let mut b = object(Some(0xa2), None, if down { 0.0 } else { 1.0 }, 2);
            b.pos = [8.0, 0.0, 0.0];
            let snap = telemetry::Snapshot {
                tick: u64::from(tick),
                players: vec![player(0, None, 0xe001), player(1, None, 0xe002)],
                objects: vec![Some(a), Some(b)],
                ..Default::default()
            };
            st.tally.tick_hashed(
                tick_sample(&snap),
                state_hash::gameplay(&snap, Some(7)),
                None,
            );
            st.trace_tick(tick, &snap, &ground);
            if tick == 18 {
                st.hit(Some(19), Some(0), Some(1), 0.6, true, &ground);
                st.kill(19, Some(0), 1, false, &ground);
            }
        }
    }

    #[test]
    fn the_tally_is_unchanged_with_the_trace_on() {
        let text = |detail: &str| {
            format!(
                "map = \"lockout\"\nteam_play = true\ndetail = \"{detail}\"\n\
                 [[bot]]\nteam = 0\n[[bot]]\nteam = 1\n"
            )
        };
        let off = sim_with(&text("training"));
        let on = sim_with(&text("spatial"));
        assert!(off.0.lock().spatial.is_none());
        assert!(on.0.lock().spatial.is_some());
        play_forty(&off);
        play_forty(&on);
        let (a, b) = (off.report(), on.report());
        let strip = |r: &Report| {
            let mut v = serde_json::to_value(r).unwrap();
            let o = v.as_object_mut().unwrap();
            o.remove("wall");
            o.remove("match");
            v
        };
        assert_eq!(strip(&a), strip(&b));
        assert_eq!(b.kills.len(), 1);
        assert_eq!(a.match_["detail"], "training");
        assert_eq!(b.match_["detail"], "spatial");
        // The trace's kill is the report's, with both places.
        let t = on
            .0
            .lock()
            .spatial
            .take()
            .unwrap()
            .finish(&sim_bench::spatial::NoGround, None);
        t.agrees_with(&b.kills).unwrap();
        use sim_bench::spatial::Event;
        assert_eq!(t.count(|e| matches!(e, Event::Spawn { .. })), 3);
        assert_eq!(t.count(|e| matches!(e, Event::Damage { .. })), 1);
        let Some(Event::Kill {
            killer: Some(k),
            victim,
            ..
        }) = t.events.iter().find(|e| matches!(e, Event::Kill { .. }))
        else {
            panic!("no kill");
        };
        assert_eq!(k.at.unwrap().pos, [9.0, 0.0, 0.0]);
        assert_eq!(victim.at.unwrap().pos, [8.0, 0.0, 0.0]);
    }

    #[test]
    fn a_spatial_match_writes_a_trace_whose_kills_are_the_reports() {
        let dir = std::env::temp_dir().join(format!("h2-sim-spatial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let report = dir.join("report.json");
        let s = sim_reporting(
            "map = \"lockout\"\ndetail = \"spatial\"\n[[bot]]\n[[bot]]\n",
            Some(report),
        );
        play_forty(&s);
        let r = s.report();
        let note = s.write_trace(&r).unwrap();
        assert!(note.contains("are the report's"), "{note}");
        let t = sim_bench::spatial::Trace::read(&dir.join("spatial.bin")).unwrap();
        t.agrees_with(&r.kills).unwrap();
        assert_eq!(t.kills().len(), 1);
        assert_eq!(t.header.map, "lockout");
        // Taken: a second call has nothing to write.
        assert!(s.write_trace(&r).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_sim_seat_sends_the_pad_held_over_its_brain() {
        let s = sim_with("map = \"lockout\"\n[[bot]]\n");
        s.0.lock().phase = Phase::Playing;
        let mut seat = Metered {
            seat: 0,
            brain: bots::Brain::default(),
            sim: Arc::clone(&s.0),
            last_tick: None,
            thinking: None,
        };
        let mut held = HostPadState::default();
        held.left_stick = (0.25, -0.5);
        assert!(seat.hold_pad(Some(held)));
        let g = guest_layout::testing::FakeGuest::new();
        let f = Frame {
            frame: 0,
            tick: 1,
            machine: [7; 6],
            guest: &g,
        };
        assert_eq!(seat.pad(&f), held);
        assert_eq!(seat.observe(&f), Sampled::Pad(held));
        assert!(seat.hold_pad(None));
        assert_eq!(seat.pad(&f), HostPadState::default());
    }

    #[test]
    fn the_limits_end_the_match() {
        let s = sim_with("map = \"lockout\"\nmax_ticks = 2\n[[bot]]\n[[bot]]\n");
        assert!(s.keep_running());
        {
            let inner = &s.0;
            let mut st = inner.lock();
            st.phase = Phase::Playing;
            st.tally.start(0);
            let mut t = TickSample {
                tick: 0,
                players: vec![],
                teams: vec![],
            };
            st.tally.tick(&t);
            t.tick = 1;
            st.tally.tick(&t);
            assert!(
                inner
                    .matched
                    .config
                    .max_ticks
                    .is_some_and(|m| st.tally.ticks() >= m)
            );
            inner.end(&mut st, End::TickLimit);
            // The first end wins.
            inner.end(&mut st, End::Stopped);
        }
        assert!(!s.keep_running());
        let r = s.report();
        assert_eq!(r.result.end, End::TickLimit);
        assert_eq!(r.ticks, 2);
        assert_eq!(r.match_["map"], "lockout");
        let st = s.status();
        assert_eq!(st["phase"], "over");
        assert_eq!(st["end"], "tick_limit");
        // A wall limit of 0 ends a run that never started.
        let w = sim_with("map = \"lockout\"\nmax_wall_s = 0\n[[bot]]\n");
        assert!(!w.keep_running());
        assert_eq!(w.report().result.end, End::WallLimit);
        // Unpaced: no pacer; x2: twice real time.
        assert!(s.pacer().is_none());
        let p = sim_with("map = \"lockout\"\npace = \"x2\"\n[[bot]]\n")
            .pacer()
            .unwrap();
        assert_eq!(p.ticks_per_second, 20_000_000);
    }
}
