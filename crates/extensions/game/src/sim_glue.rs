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
//!   tick's player snapshot, the shots, the damage reports, the kills and
//!   the end latch, into a [`sim_bench::metrics::Tally`].
//! - **The RNG seed** is written once, before the match's
//!   first tick ([`SimMetrics`]'s one writing hook).
//! - **The end**: the game's end latch, the match's tick limit, its wall
//!   limit or `match.stop` stop the boot loop ([`Sim::keep_running`]), and
//!   [`Sim::finish`] writes the report to `H2_SIM_REPORT`.
//!
//! The agent gets `match.status`, `match.report` and `match.stop`
//! ([`Sim::add_methods`]).

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
use sim_bench::config::{BotParams, Match};
use sim_bench::metrics::{BotInfo, BrainTick, End, Finish, PlayerSample, Report, Tally, TickSample};

use crate::level_boot;
use std::collections::BTreeMap;
mod film_state;
pub mod hosted;
mod state_hash;

/// The engine's shared random seed: a pointer to the struct whose first
/// word it is (docs/graphics/notes/pipelined-render.md, "The prototype").
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
            lockstep::hash_sim(guest, lockstep::SimRegion::HALO2),
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
    /// The initial RNG seed has been written for this match.
    set_up: bool,
    /// Weapon datums fired since the last tick.
    fired: Vec<u32>,
    notes: Vec<String>,
    /// A copy-on-write guest RAM base census at the match's end
    /// (`state.load` with `"cow": true`): pages written over the base.
    cow: Option<(u64, u64)>,
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
    }
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
            .on_skip(crate::d3d_diag::note_skipped_present);
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
                tally: Tally::new(),
                phase: Phase::Waiting,
                end: None,
                first_tick: None,
                last_tick: None,
                dispatch_start: None,
                dispatch_end: None,
                set_up: false,
                fired: Vec::new(),
                notes: Vec::new(),
                cow: None,
            }),
            stop: AtomicBool::new(false),
            created: Instant::now(),
        }))
    }

    pub fn matched(&self) -> &Match {
        &self.0.matched
    }

    /// Resolve the match's multiplayer map for the System Link lobby.
    pub fn select_map(&self, disc: &Path, data_root: &Path) -> Result<()> {
        let m = &self.0.matched;
        let req = level_boot::resolve_in(
            disc,
            Some(data_root),
            &m.config.map,
            level_boot::Difficulty::Normal,
        )
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
                ..Default::default()
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
        features
            .add(&SimMetrics { sim: Arc::clone(&self.0), film_playback })
            .map_err(|e| anyhow::anyhow!("sim metrics: {e}"))?;
        eprintln!(
            "sim: {} on {}, {} bot(s), {} ({})",
            self.0.path.display(),
            self.0.matched.config.map,
            self.0.matched.bots.len(),
            self.0.matched.game_type.name(),
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
        report
    }

    /// The guest RAM census at the match's end, for the report's `wall`
    /// block: `(written, total)` pages over a shared copy-on-write base.
    pub fn note_cow(&self, written: Option<(u64, u64)>) {
        let mut s = self.0.lock();
        s.cow = written;
    }

    /// The report at the end of the run, written to `H2_SIM_REPORT` when
    /// set, and its summary on stderr.
    pub fn finish(&self) -> Result<Report> {
        let r = self.report();
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
        if self.0.lock().bindings.len() != self.matched().bots.len() {
            bail!(
                "sim: match ended before all {} seats had a player and a spawned unit",
                self.matched().bots.len()
            );
        }
        Ok(r)
    }

    /// `match.status`, `match.report` and `match.stop` (docs/runtime/agent-api.md,
    /// "Sim matches").
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
            "game_type": inner.matched.game_type.name(),
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
            },
            "end": s.end.map(End::name),
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
    report
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
            let mut s = self.sim.lock();
            if s.phase == Phase::Playing
                && let Some(&player) = s.bindings.get(&self.seat)
            {
                s.tally.brain_for_player(player, b);
            }
        }
    }
}

/// The metrics feature (`sim-metrics`): read-only observers, and one
/// `before` on the game tick that writes the seed once a match.
struct SimMetrics {
    sim: Arc<Inner>,
    film_playback: Option<bool>,
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
            drop(g);
            let mut s = me.lock();
            s.bindings.clear();
            s.tally.start((tick & 0xffff_ffff) as u32);
            s.phase = Phase::Binding;
            s.set_up = false;
            s.fired.clear();
            s.first_tick = None;
            s.dispatch_start = None;
            s.dispatch_end = None;
            eprintln!("sim: match started at tick {tick}");
        });
        let me = Arc::clone(&self.sim);
        let arena_buffer = b
            .switches()
            .implementation("sim.arena-hash", false)
            .then(|| Mutex::new(Vec::new()));
        let projected = b.switches().implementation("sim.metrics-projection", true);
        b.frame().on_tick("sim: tick sample", move |o| {
            if !matches!(me.lock().phase, Phase::Playing | Phase::Binding) {
                return;
            }
            let g = o.guest();
            if me.lock().phase == Phase::Binding
                && bots::phantom::session::host_session(&*g).is_err()
            {
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
            drop(g);
            let mut s = me.lock();
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
                        s.first_tick = Some(Instant::now());
                        s.tally.start(sample.tick);
                        s.dispatch_start = o.runtime.dispatch_counts();
                    }
                    let resets = s.tally.resets();
                    let tick = sample.tick;
                    s.tally
                        .tick_hashed(sample, state_hash::gameplay(&snap, rng), arena);
                    let mut components = state_hash::components(&snap, rng);
                    components.extend(arena_blocks.iter().enumerate().map(|(i, h)| {
                        (format!("{}{i:02}", sim_bench::diff::ARENA_PREFIX), format!("{h:016x}"))
                    }));
                    s.tally.tick_components(tick, arena_base, components);
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
                        s.set_up = false;
                        eprintln!(
                            "sim: the tick jumped to {tick} (a snapshot load); counting from here"
                        );
                    }
                    for p in shooters(&snap, &fired) {
                        s.tally.shot(p);
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
            drop(g);
            let mut s = me.lock();
            if ended && s.phase == Phase::Playing {
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
                        bots::vehicle::read::firing_player(&*g, engine::DatumHandle(w))
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
                let mut s = me.lock();
                if s.phase != Phase::Playing {
                    return;
                }
                let target = player_of_unit(s.tally.last(), hit.victim);
                s.tally.hit(
                    player_of_datum(hit.owner_player),
                    target,
                    hit.body + hit.shield,
                    me.matched.team_play,
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
                let mut s = me.lock();
                if s.phase == Phase::Playing {
                    s.tally.kill(
                        (tick & 0xffff_ffff) as u32,
                        player_of_datum(attacker),
                        v,
                        betrayal,
                    );
                }
            });
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
        Ok(())
    }
}

/// The 32-bit seed for a match seed.
pub fn seed32(seed: u64) -> u32 {
    ((seed >> 32) as u32) ^ (seed as u32)
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
        let holder = engine::DatumHandle::new(0xe, 1);
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

    fn sim_with(text: &str) -> Sim {
        let c = sim_bench::config::parse_match(text, false).unwrap();
        let m = sim_bench::config::resolve(c, &|_| Err("no files".into())).unwrap();
        Sim::new(PathBuf::from("/tmp/m.toml"), m, None)
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
