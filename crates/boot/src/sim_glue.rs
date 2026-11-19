//! Sim mode (`H2_SIM=<match file>`, `h2 sim run`; docs/runtime/sim.md): the
//! game's simulation with bots, without rendering, as fast as the host goes,
//! ending in a JSON report.
//!
//! What it does to a boot (wired in `run_hosted_with_live`):
//!
//! - **The level** is the match's multiplayer map, through the level boot's
//!   direct path, with the bot count, the game type and the variant
//!   settings handed over as a [`level_boot::MpSetup`].
//! - **The input** has no keyboard pad: all four ports are free for bots
//!   ([`bots::local::Local::set_capacity`]).
//! - **The bots** are the match file's, each a [`bots::Brain`] with its
//!   skill, its seed (from the match seed and its port) and its tuning, in a
//!   [`Metered`] wrapper that counts what only the brain knows.
//! - **The render front-end** is skipped in world frames
//!   ([`feature::sim_frame`]); the pipelined render is off, the kernel
//!   trace file is off, and the nav meshes are never built in the
//!   background (`bots_nav_auto`).
//! - **The pace** is the match's ([`sim::clock::Pace`]); unpaced by default.
//! - **The metrics** come from read-only observers ([`SimMetrics`]): each
//!   tick's player snapshot, the shots, the damage reports, the kills and
//!   the end latch, into a [`sim::metrics::Tally`].
//! - **The seed and the teams** are written once, before the match's
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

use bots::local::{Frame, PadSource};
use feature::sim_frame::SimFrame;

use crate::telemetry;
use feature::{Flow, points};
use input::host::HostPadState;
use sim::config::{BotParams, Match};
use sim::metrics::{BotInfo, BrainTick, End, Finish, PlayerSample, Report, Tally, TickSample};

use crate::level_boot;

/// The engine's shared random seed: a pointer to the struct whose first
/// word it is (docs/graphics/pipelined-render.md, "random").
pub const RANDOM_SEED_PTR: u32 = 0x004e_7408;

/// Weapons fired between two ticks that are kept for attribution.
const MAX_FIRED: usize = 512;

/// Where the match is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Booting, loading, or a game that is not the match.
    Waiting,
    Playing,
    Over,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Waiting => "waiting",
            Phase::Playing => "playing",
            Phase::Over => "over",
        }
    }
}

struct State {
    tally: Tally,
    phase: Phase,
    end: Option<End>,
    first_tick: Option<Instant>,
    last_tick: Option<Instant>,
    /// The seed and teams are written for this match.
    set_up: bool,
    /// Weapon datums fired since the last tick.
    fired: Vec<u32>,
    notes: Vec<String>,
}

struct Inner {
    path: PathBuf,
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

/// The match file `H2_SIM` names, read and checked; `None` when unset.
/// `H2_BOOT_GFX` is refused: sim mode has no GPU.
pub fn from_env() -> Result<Option<Sim>> {
    let Some(path) = std::env::var_os(sim::ENV).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    if std::env::var_os("H2_BOOT_GFX").is_some() {
        bail!("{}: sim mode renders nothing; unset H2_BOOT_GFX", sim::ENV);
    }
    if std::env::var_os(bots::ENV).is_some_and(|v| !v.is_empty()) {
        bail!(
            "{}: the match file says which bots play; unset {}",
            sim::ENV,
            bots::ENV
        );
    }
    let path = PathBuf::from(path);
    let matched = sim::config::load_match(&path)
        .map_err(|e| anyhow::anyhow!("{}: {}", sim::ENV, e.replace('\n', "; ")))?;
    let report_path = std::env::var_os(sim::REPORT_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    Ok(Some(Sim::new(path, matched, report_path)))
}

/// `sim::config::Skill` as the bots crate's.
pub fn skill(s: sim::config::Skill) -> bots::mind::Skill {
    match s {
        sim::config::Skill::Easy => bots::mind::Skill::Easy,
        sim::config::Skill::Normal => bots::mind::Skill::Normal,
        sim::config::Skill::Heroic => bots::mind::Skill::Heroic,
        sim::config::Skill::Legendary => bots::mind::Skill::Legendary,
    }
}

/// A tuning file's numbers as the bots crate takes them.
pub fn tuning(p: &BotParams) -> bots::tuning::Tuning {
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
        caution: p.caution,
        teamwork: p.teamwork,
        objective_focus: p.objective_focus,
    }
}

/// What the brain is doing, as the report names it.
pub fn doing_name(d: &bots::mind::Doing) -> &'static str {
    use bots::mind::Doing;
    match d {
        Doing::Idle => "idle",
        Doing::Fight(_) => "fight",
        Doing::Chase(_) => "chase",
        Doing::Roam => "roam",
        Doing::Support(_) => "support",
        Doing::Supply(_) => "supply",
        Doing::Vantage(_) => "vantage",
        Doing::Rearm(_) => "rearm",
        Doing::Recover(_) => "recover",
        Doing::Carry(_) => "carry",
        Doing::Keep => "keep",
        Doing::Play(_) => "play",
        Doing::Mount(_) => "mount",
        Doing::Drive(_) => "drive",
        Doing::Ride(_) => "ride",
    }
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
                controller: p.controller,
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

/// The player whose unit is `object`, from the last sample.
fn player_of_unit(last: Option<&TickSample>, object: u32) -> Option<u8> {
    last?
        .players
        .iter()
        .find(|p| p.unit == Some(object))
        .map(|p| p.index)
}

/// A kill's attacker or victim datum as a player index (`None` for none).
fn player_of_datum(d: u32) -> Option<u8> {
    (d != u32::MAX).then_some((d & 0xffff) as u8)
}

fn speed(inner: &Inner, s: &State) -> sim::clock::SpeedMeter {
    let now = Instant::now();
    let first = s.first_tick.unwrap_or(now);
    let last = s.last_tick.unwrap_or(first);
    sim::clock::SpeedMeter {
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
            matched,
            report_path,
            frame,
            state: Mutex::new(State {
                tally: Tally::new(),
                phase: Phase::Waiting,
                end: None,
                first_tick: None,
                last_tick: None,
                set_up: false,
                fired: Vec::new(),
                notes: Vec::new(),
            }),
            stop: AtomicBool::new(false),
            created: Instant::now(),
        }))
    }

    pub fn matched(&self) -> &Match {
        &self.0.matched
    }

    /// The match's level request, and its multiplayer setup handed to the
    /// level boot.
    pub fn level_request(&self, disc: &Path, data_root: &Path) -> Result<level_boot::Request> {
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
        level_boot::set_mp_setup(level_boot::MpSetup {
            players: m.bots.len() as u32,
            game_type: m.game_type.list_index(),
            settings: m.variant.clone(),
            label: m.game_type.name().into(),
        })?;
        Ok(req)
    }

    /// The match's bots on ports 0 up, all four places open.
    pub fn add_bots(&self) {
        let local = bots::local::global();
        local.set_capacity(4);
        for b in &self.0.matched.bots {
            let brain = bots::Brain::new(skill(b.skill), b.seed).with_tuning(tuning(&b.params));
            let source = Metered {
                brain,
                sim: Arc::clone(&self.0),
                last_tick: None,
            };
            match local.add(b.port, &b.name, Box::new(source)) {
                Ok(h) => eprintln!(
                    "sim: {} (bot {}, {}{}) wants port {}",
                    b.name,
                    h.0,
                    b.skill.name(),
                    b.arm
                        .as_deref()
                        .map(|a| format!(", arm {a}"))
                        .unwrap_or_default(),
                    b.port
                ),
                Err(e) => eprintln!("sim: {}: {e}", b.name),
            }
        }
    }

    /// The pacer for the match's pace (`None`: unpaced).
    pub fn pacer(&self) -> Option<kernel::pacer::Pacer> {
        let tps = self.0.matched.pace.ticks_per_second()?;
        let mut p = kernel::pacer::Pacer::real_time();
        p.ticks_per_second = tps;
        Some(p)
    }

    /// The render skip and the metrics, into the plan.
    pub fn install(&self, features: &mut feature::Registry) -> Result<()> {
        features
            .add(&self.0.frame)
            .map_err(|e| anyhow::anyhow!("sim frame: {e}"))?;
        features
            .add(&SimMetrics(Arc::clone(&self.0)))
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
        if let Some(limit) = inner.matched.config.max_wall_s
            && inner.created.elapsed().as_secs() >= limit
            && !inner.stop.load(Ordering::Acquire)
        {
            let mut s = inner.lock();
            inner.end(&mut s, End::WallLimit);
        }
        !inner.stop.load(Ordering::Acquire)
    }

    /// The report as it stands (`end`: the match's, else `Aborted`).
    pub fn report(&self) -> Report {
        let inner = &self.0;
        let s = inner.lock();
        build_report(inner, &s)
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
        let s = inner.lock();
        let sp = speed(inner, &s);
        let f = inner.frame.stats();
        json!({
            "match": inner.path.display().to_string(),
            "map": inner.matched.config.map,
            "game_type": inner.matched.game_type.name(),
            "bots": inner.matched.bots.len(),
            "phase": s.phase.name(),
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

fn build_report(inner: &Inner, s: &State) -> Report {
    let bots: Vec<BotInfo> = inner
        .matched
        .bots
        .iter()
        .map(|b| BotInfo {
            port: b.port,
            name: b.name.clone(),
            arm: b.arm.clone(),
        })
        .collect();
    let f = inner.frame.stats();
    let mut notes = s.notes.clone();
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
    s.tally.report(Finish {
        match_: inner.matched.resolved_json(),
        team_play: inner.matched.team_play,
        bots: &bots,
        end: s.end.unwrap_or(End::Aborted),
        speed: speed(inner, s),
        nav,
        notes,
    })
}

/// A bot's brain, counting per game tick what only it knows: stuck, its
/// repaths, its activity.
pub struct Metered {
    brain: bots::Brain,
    sim: Arc<Inner>,
    last_tick: Option<u32>,
}

impl PadSource for Metered {
    fn pad(&mut self, f: &Frame<'_>) -> HostPadState {
        let pad = self.brain.pad(f);
        if let Some(t) = f.tick
            && self.last_tick != Some(t)
        {
            self.last_tick = Some(t);
            let m = &self.brain.mind;
            let b = BrainTick {
                stuck: m.stuck.level != bots::stuck::Level::Moving,
                repaths: m.stuck.repaths,
                doing: doing_name(&m.doing),
            };
            let mut s = self.sim.lock();
            if s.phase == Phase::Playing {
                s.tally.brain(f.port, b);
            }
        }
        pad
    }
}

/// The metrics feature (`sim-metrics`): read-only observers, and one
/// `before` on the game tick that writes the seed and the teams once a
/// match.
struct SimMetrics(Arc<Inner>);

impl feature::Feature for SimMetrics {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new("sim-metrics", "Sim mode: the match's metrics and its end")
            .doc("docs/runtime/sim.md")
    }

    fn build(&self, b: &mut feature::Builder) -> std::result::Result<(), feature::BuildError> {
        // A load (a fork) winds the tick back; the tally starts again there.
        b.snapshot(feature::SnapshotPolicy::Stateless);
        let me = Arc::clone(&self.0);
        b.game().on_start("sim: match start", move |o| {
            let g = o.guest();
            let mode = level_boot::active_mode(&*g);
            if mode != Some(level_boot::MODE_MULTIPLAYER) {
                return;
            }
            let tick = telemetry::game_tick(&g).unwrap_or(0);
            drop(g);
            let mut s = me.lock();
            s.tally.start((tick & 0xffff_ffff) as u32);
            s.phase = Phase::Playing;
            s.set_up = false;
            s.fired.clear();
            s.first_tick = Some(Instant::now());
            eprintln!("sim: match started at tick {tick}");
        });
        let me = Arc::clone(&self.0);
        b.frame().on_tick("sim: tick sample", move |o| {
            if me.lock().phase != Phase::Playing {
                return;
            }
            let g = o.guest();
            let snap = telemetry::snapshot(&g);
            let ended = g
                .u32(telemetry::layout::GAME_GLOBALS_PTR)
                .ok()
                .filter(|p| *p != 0)
                .and_then(|p| g.u8(p + telemetry::layout::GAME_ENDED).ok())
                .is_some_and(|b| b != 0);
            drop(g);
            let mut s = me.lock();
            match snap {
                Ok(snap) => {
                    let fired = std::mem::take(&mut s.fired);
                    let sample = tick_sample(&snap);
                    let resets = s.tally.resets();
                    s.tally.tick(&sample);
                    if s.tally.resets() != resets {
                        // A snapshot load (a forked run): the match is
                        // timed, seeded and teamed from here.
                        s.first_tick = Some(Instant::now());
                        s.set_up = false;
                        eprintln!("sim: the tick jumped to {} (a snapshot load); counting from here", sample.tick);
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
        let me = Arc::clone(&self.0);
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
        let me = Arc::clone(&self.0);
        b.raw()
            .observe_before(&points::WEAPON_FIRE, "sim: shots", move |o| {
                if let Some(w) = o.arg_u32("weapon") {
                    let mut s = me.lock();
                    if s.phase == Phase::Playing && s.fired.len() < MAX_FIRED {
                        s.fired.push(w);
                    }
                }
            })
            .optional();
        let me = Arc::clone(&self.0);
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
                );
            })
            .optional();
        let me = Arc::clone(&self.0);
        b.raw()
            .observe_before(&points::PLAYER_KILLED, "sim: kills", move |o| {
                let esp = o.cpu.esp;
                let read = {
                    let g = o.guest();
                    (|| {
                        let attacker = g.u32(esp.wrapping_add(4))?;
                        let victim = g.u32(esp.wrapping_add(8))?;
                        let betrayal = g.u32(esp.wrapping_add(0x0c))? != 0;
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
        let me = Arc::clone(&self.0);
        b.raw()
            .before(&points::GAME_TICK, "sim: seed and teams", move |c| {
                {
                    let s = me.lock();
                    if s.phase != Phase::Playing || s.set_up {
                        return Ok(Flow::Continue);
                    }
                }
                let rt = Arc::clone(c.ctx.runtime());
                let mut g = rt.memory();
                let mut notes = Vec::new();
                match write_seed(&mut g, me.matched.config.seed) {
                    Ok(v) => eprintln!("sim: seed {:#010x} written", v),
                    Err(e) => notes.push(format!("the seed was not written: {e:?}")),
                }
                if me.matched.team_play {
                    notes.extend(write_teams(&mut g, &me.matched));
                }
                drop(g);
                let mut s = me.lock();
                s.set_up = true;
                for n in notes {
                    eprintln!("sim: {n}");
                    s.notes.push(n);
                }
                Ok(Flow::Continue)
            });
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

/// Each bot's team on its player and unit (as `sim.state.set` writes them);
/// the problems, as notes.
fn write_teams(g: &mut runtime::Guest, m: &Match) -> Vec<String> {
    let mut notes = Vec::new();
    let players = match telemetry::players(g) {
        Ok(p) => p,
        Err(e) => return vec![format!("the teams were not written: {e:?}")],
    };
    for b in &m.bots {
        let Some(team) = b.team else { continue };
        let Some(p) = players.iter().find(|p| p.controller == Some(b.port)) else {
            notes.push(format!(
                "{}: no player on port {} for its team",
                b.name, b.port
            ));
            continue;
        };
        let params = agent_proto::sim::SetParams {
            player: format!("player.{}", p.player.index),
            team: Some(i32::from(team)),
            ..Default::default()
        };
        if let Err(e) = agent::sim::state::apply(&mut *g, p.player.index, &params) {
            notes.push(format!(
                "{}: team {team} not written: {}",
                b.name, e.message
            ));
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let t = tuning(&p);
        assert_eq!(t.reaction, Some(4));
        assert!((t.fire_cone.unwrap() - std::f32::consts::PI).abs() < 1e-6);
        assert_eq!(
            (t.burst, t.burst_on, t.burst_off),
            (Some(true), Some(6), Some(2))
        );
        assert_eq!(t.objective_focus, Some(0.4));
        assert!(tuning(&BotParams::default()).is_empty());
    }

    #[test]
    fn skills_and_doings_map() {
        for (a, b) in [
            (sim::config::Skill::Easy, bots::mind::Skill::Easy),
            (sim::config::Skill::Legendary, bots::mind::Skill::Legendary),
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
        for d in sim::metrics::OBJECTIVE_DOINGS {
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
                p0.controller,
                p0.unit,
                p0.alive,
                p0.in_vehicle,
                p0.grenades
            ),
            (0, Some(0), Some(0xe001), true, false, Some(2))
        );
        assert_eq!((p0.score, p0.kills, p0.deaths), (3, 3, 1));
        let p1 = &s.players[1];
        assert!(!p1.alive, "body depleted");
        assert!(p1.in_vehicle);
        let p2 = &s.players[2];
        assert_eq!((p2.unit, p2.alive, p2.controller), (None, false, None));
        // Shots go to whoever holds the weapon now.
        assert_eq!(shooters(&snap, &[0xa2, 0xa1, 0xa1, 0xdead]), [1, 0, 0]);
        assert_eq!(player_of_unit(Some(&s), 0xe002), Some(1));
        assert_eq!(player_of_unit(Some(&s), 0x1234), None);
        assert_eq!(player_of_unit(None, 0xe002), None);
        assert_eq!(player_of_datum(u32::MAX), None);
        assert_eq!(player_of_datum(0xe5a4_0002), Some(2));
    }

    #[test]
    fn seeds_fold_to_32_bits() {
        assert_eq!(seed32(7), 7);
        assert_eq!(seed32(1 << 32), 1);
        assert_ne!(seed32(1), seed32(2));
    }

    fn sim_with(text: &str) -> Sim {
        let c = sim::config::parse_match(text, false).unwrap();
        let m = sim::config::resolve(c, &|_| Err("no files".into())).unwrap();
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
