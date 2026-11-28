//! `H2_AGENT=<socket>` or `H2_AGENT=tcp://HOST:PORT`: the agent API
//! (crates/extensions/agent, docs/runtime/agent-api.md).
//!
//! Listens, adds the `agent` feature (a tick observer that answers
//! requests at tick boundaries), and answers between slices when the game
//! isn't ticking. Frame stats are collected for a live window when nothing
//! else started them, so `profile.counters` has frames to report.
//!
//! With XAPI input, the agent's input tap goes on `xapi`, wrapping the one
//! `H2_INPUT_RECORD` or `H2_INPUT_REPLAY` set: `input.pad`,
//! `input.release`, `input.state`, `input.record`, `input.replay` and
//! `input.stop` (crates/extensions/agent's `tap`). That is the same on `h2 serve`,
//! `h2 play` and the Xbox app.
//!
//! It also answers the host methods `profile.sample.*` (the in-process
//! sampler, [`crate::sampling`], which `profile.capture {"sample": true}`
//! runs too) and `host.perf` (the newest host perf line), and
//! `render.frame` and `render.stats` (crates/extensions/agent's `render`, over
//! [`crate::render_probe`] and the draw wrapper in [`crate::d3d_diag`]),
//! and `render.compare`, `render.capture`, `render.scene` and
//! `render.parity`. With a socket it adds `agent-frames`
//! ([`crate::agent_frames`]): the tick marks `render.capture` needs and
//! the rows `sim.hashes` answers.
//! `sim.*` ([`install_sim`]) gets its session kind from `coop_probe`'s
//! readers ([`session_view`]); its teleports, `sim.give` and `sim.spawn`
//! call the game's own functions through the `agent-calls` feature
//! (`agent::sim::calls`), and find tags as `edit.*` does
//! (`hot_glue::map_tags`, set once the runtime is up).
//!
//! `shadow.*` checks the native sim ports against the lifted original
//! (crates/extensions/agent's `shadow`, over `sim_bridge::shadow`; the image comes from
//! [`crate::fallback`]).
//!
//! `state.*` (machine snapshots) is the boot loop's: it installs the store
//! on the agent and runs due saves and loads between slices
//! ([`crate::snapshot_glue`]).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use agent::Access;
use agent_proto::{Endpoint, RpcError, code, method};
use anyhow::{Context, Result};
use serde_json::{Value, json};

mod matchmaking;

/// Optional second listener, supplied by the dedicated-host supervisor
/// as a TCP address (without the tcp:// prefix).
pub const HOST_TCP_ENV: &str = "H2_HOST_AGENT_TCP";

/// The agent attached to one run.
pub struct AgentGlue {
    pub agent: agent::Agent,
    /// `sim.*`'s state; `None` for [`AgentGlue::local`].
    pub sim: Option<agent::sim::SimHandle>,
    /// Empty for [`AgentGlue::local`], which has only in-process clients.
    _listeners: Vec<agent::Server>,
}

/// The socket path from `H2_AGENT`, when set, not empty and not one of the
/// switching-off values (`off`, `0`, ...; [`agent_proto::discover::is_off`]).
pub fn socket_from_env() -> Option<PathBuf> {
    std::env::var_os(agent_proto::ENV)
        .filter(|v| !v.is_empty())
        .filter(|v| !v.to_str().is_some_and(agent_proto::discover::is_off))
        .map(PathBuf::from)
}

/// The socket a game opens when `H2_AGENT` is unset: `<pid>.sock` in the
/// agents directory, where editors and `h2 agent --list` find it
/// ([`agent_proto::discover`]). `None` when `H2_AGENT` switches it off, or
/// where there are no Unix sockets.
pub fn discoverable_socket() -> Option<PathBuf> {
    if !cfg!(any(unix, windows)) || agent_proto::discover::off_from_env() {
        return None;
    }
    let path = agent_proto::discover::process_socket();
    // A Unix socket path must fit `sun_path` (104 bytes on macOS), and the
    // directory must be ours to make: a game that only wanted to be found
    // must not fail to start over it.
    let dir = path.parent()?;
    if path.as_os_str().len() > MAX_SOCKET_PATH {
        eprintln!("agent: {} is too long for a socket; not listening by default", path.display());
        return None;
    }
    if let Err(e) = agent_proto::discover::create_private_dir(dir) {
        eprintln!("agent: {}: {e}; not listening by default", dir.display());
        return None;
    }
    Some(path)
}

/// The longest Unix socket path that binds on every platform we run on.
const MAX_SOCKET_PATH: usize = 100;

/// Recording/playback hosts retain the ordinary dispatch and report why a
/// live editor cannot queue a different set of options or map bytes.
pub fn reload_recording_refusal(
    theater_playback: bool,
    film: bool,
    lockstep: &crate::lockstep_glue::Config,
) -> Option<&'static str> {
    if theater_playback || lockstep.replay.is_some() {
        Some("map reload is disabled during playback")
    } else if lockstep.record.is_some() {
        Some("map reload is disabled while recording input")
    } else if film {
        Some("map reload is disabled while recording a film")
    } else {
        None
    }
}

fn editor_points() -> impl Iterator<Item = &'static hooks::points::Point> {
    use hooks::points::{GAME_ACTIVATE, GAME_STATE_REVERT, GAME_TICK};
    [&GAME_TICK, &GAME_ACTIVATE, &GAME_STATE_REVERT]
        .into_iter()
        .chain(agent::sim::calls::reload_hook_points())
}

impl AgentGlue {
    /// Listens at `path`, adds the feature to `registry`, and puts the input
    /// tap on `xapi`. `replaying`: the tap already there is a replay, which
    /// stands in for the live pad (agent steps go on top of it); otherwise
    /// it is a recording, which sees agent steps too.
    pub fn install<S: xapi::Storage>(
        registry: &mut hooks::Registry,
        path: PathBuf,
        xapi: Option<&Mutex<xapi::Xapi<S>>>,
        replaying: bool,
        allow_bot_add: bool,
        data_root: &Path,
    ) -> Result<AgentGlue> {
        Self::with_endpoint(registry, Some(Endpoint::from(path)), xapi, replaying, allow_bot_add, data_root)
    }

    /// A recording uses the player's observation and input hooks even when no
    /// external controller is attached, so guest scheduling and snapshot feature
    /// identities agree when Theater attaches its transport socket later.
    pub fn with_endpoint<S: xapi::Storage>(
        registry: &mut hooks::Registry,
        endpoint: Option<Endpoint>,
        xapi: Option<&Mutex<xapi::Xapi<S>>>,
        replaying: bool,
        allow_bot_add: bool,
        data_root: &Path,
    ) -> Result<AgentGlue> {
        if let Some(Endpoint::Unix(p)) = &endpoint
            && let Some(dir) = p.parent()
        {
            // The agents directory is for this user alone.
            let made = if dir == agent_proto::discover::dir() {
                agent_proto::discover::create_private_dir(dir)
            } else {
                std::fs::create_dir_all(dir)
            };
            made.with_context(|| format!("creating {}", dir.display()))?;
        }
        let agent = agent::Agent::new();
        add_host_methods(&agent);
        // render.scene: the frame step's FrameScene (docs/graphics/framescene.md),
        // and the live mirror's camera layer from the same sealed scene.
        #[cfg(feature = "gpu")]
        {
            crate::scene_glue::install();
            crate::scene_glue::install_cameras(&agent);
        }
        let sim = install_sim(&agent, allow_bot_add);
        // The game's own functions for sim (teleports, give, spawn): a
        // `before` on the tick that makes the calls queued for it.
        let calls = agent::sim::calls::shared();
        agent::sim::calls::lock(&calls)
            .set_spawn_enrollment(crate::actor_net::creature::spawn_enrollment);
        match registry.add(&agent::sim::calls::CallsFeature(calls.clone())) {
            Ok(true) => {
                // The superbounce verifier's trials teleport through it too.
                crate::superbounce_verify::set_calls(calls.clone());
                sim.set_calls(calls)
            }
            Ok(false) => eprintln!(
                "agent: agent-calls switched off by H2_FEATURES; sim teleports, give and spawn are off"
            ),
            Err(e) => anyhow::bail!("agent-calls: {e}"),
        }
        agent::forge::install(&agent, sim.clone(), &agent::sim::hashes::HASHES);
        // firefight.*: the Firefight runtime, or why there is none
        // (docs/game/firefight.md, "Agent API").
        agent::firefight::install(
            &agent,
            std::sync::Arc::new(crate::firefight::feature::AgentSource::installed()),
        );
        // rules.state and rules.reload: the game's rules script, or why
        // there is none, and putting a new one in (docs/game/rules-engine.md,
        // "The host runtime", "Reloading a running script"). A restart
        // queues the game through the agent's guest calls; a film's
        // playback takes no reload.
        #[cfg(feature = "rules")]
        agent::rules::install(
            &agent,
            std::sync::Arc::new(
                crate::rules::feature::AgentSource::installed().playback(replaying),
            ),
            sim.calls(),
        );
        // variants.quick: the lobby variant's Quick Options rows as its
        // script marks them (docs/game/rules-variants.md, "Quick Options
        // in the lobby").
        agent::variants::install(&agent, std::sync::Arc::new(crate::variant_quick::AgentSource));
        let probes = agent::probe::Probes::default();
        if registry.add(&probes).map_err(|e| anyhow::anyhow!("agent-probes: {e}"))? {
            probes.install(&agent, &sim);
        }
        agent::ai::install(&agent);
        agent::pools::install(&agent);
        // net.replication: the event counter is an observer of its own, so
        // H2_FEATURES can drop it and leave the method's other readings.
        match registry.add(&agent::net::NetFeature) {
            Ok(true) => {}
            Ok(false) => eprintln!(
                "agent: agent-net switched off by H2_FEATURES; net.replication counts no events"
            ),
            Err(e) => anyhow::bail!("agent-net: {e}"),
        }
        // actors.net: the actor channel's report of the last tick
        // (docs/game/ai-networking.md, "Interfaces").
        agent::actors::install(&agent, crate::actor_net::report::get);
        agent::net::install(
            &agent,
            || {
                let t = live_hle::traffic::snapshot();
                Some(agent::net::Traffic {
                    sent: t.sent,
                    sent_bytes: t.sent_bytes,
                    received: t.received,
                    received_bytes: t.received_bytes,
                    largest: t.largest,
                })
            },
            |m| {
                use crate::actor_net::creature;
                if !creature::registered(m) {
                    return agent::net::CreatureReplication::default();
                }
                agent::net::CreatureReplication {
                    slot: Some(creature::SLOT),
                    entities: creature::snapshot(m)
                        .into_iter()
                        .map(|row| agent::net::CreatureEntity {
                            entity: row.entity,
                            object: row.object,
                            updates: row.updates,
                        })
                        .collect(),
                }
            },
        );
        agent::havok::install(&agent);
        // The native collision world's counters (impl.sim.native.collision).
        agent::collision::install(&agent);
        // The animation graph walk's arm and body (impl.sim.native.anim).
        agent::anim::install(&agent);
        // Every native-sim arm in one read (`sim.arms`, the phase gate).
        agent::sim_arms::install(&agent);
        agent::raw::install(&agent, &sim);
        agent::havok::install_writes(&agent, &sim);
        // `world.*`'s `physics.standing_fits`: the superbounce mode's
        // capsule test, while it is on (docs/game/superbounce.md, "Charge").
        agent.set_standing_fits(Box::new(crate::superbounce_mode::StandingFitsNow));
        // The superbounce game type's state, published each tick by
        // `superbounce_mode` (docs/game/superbounce.md).
        agent.add_game_method(
            agent_proto::superbounce::method::STATE,
            Access::Read,
            |params, _ctx| {
                crate::superbounce_mode::no_params(&params)
                    .map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?;
                Ok(serde_json::to_value(crate::superbounce_mode::state_reply())
                    .expect("superbounce.state serialises"))
            },
        );
        // Forces the superbounce mode off over the game type (it can't
        // force it on: only a script that requires `hud.superbounce` does).
        agent.add_game_method(
            agent_proto::superbounce::method::SET,
            Access::Act,
            |params, _ctx| {
                let p: agent_proto::superbounce::SetParams = if params.is_null() {
                    Default::default()
                } else {
                    serde_json::from_value(params)
                        .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
                };
                let mode = crate::superbounce_mode::set_override(p.on)
                    .map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?
                    .to_string();
                Ok(serde_json::to_value(agent_proto::superbounce::Set { mode })
                    .expect("superbounce.set serialises"))
            },
        );
        // Re-reads a map's spot files into the running game, and sets
        // live overrides of the speed model (superbounce_reload.rs).
        agent.add_game_method(
            agent_proto::superbounce::method::RELOAD,
            Access::Act,
            |params, _ctx| {
                let p: agent_proto::superbounce::ReloadParams = if params.is_null() {
                    Default::default()
                } else {
                    serde_json::from_value(params)
                        .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
                };
                let r = crate::superbounce_reload::reload(&p)
                    .map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?;
                Ok(serde_json::to_value(r).expect("superbounce.reload serialises"))
            },
        );
        agent.add_game_method(
            agent_proto::superbounce::method::TUNE,
            Access::Act,
            |params, _ctx| {
                let p: agent_proto::superbounce::TuneParams = if params.is_null() {
                    Default::default()
                } else {
                    serde_json::from_value(params)
                        .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
                };
                let r = crate::superbounce_reload::tune(&p)
                    .map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?;
                Ok(serde_json::to_value(r).expect("superbounce.tune serialises"))
            },
        );
        // The superbounce fall assist (superbounce_assist.rs,
        // docs/game/superbounce.md, "Hold X: the fall assist").
        agent.add_game_method(
            agent_proto::superbounce::ASSIST,
            Access::Read,
            |params, _ctx| {
                crate::superbounce_assist::no_params(&params)
                    .map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?;
                Ok(serde_json::to_value(crate::superbounce_assist::assist_reply())
                    .expect("superbounce.assist serialises"))
            },
        );
        // Holds the assist's button for an agent with no pad.
        agent.add_game_method(
            agent_proto::superbounce::ASSIST_TEST,
            Access::Act,
            |params, _ctx| {
                let p: agent_proto::superbounce::AssistTestParams =
                    serde_json::from_value(params)
                        .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
                let hold = crate::superbounce_assist::set_test_hold(p.hold);
                Ok(serde_json::to_value(agent_proto::superbounce::AssistTest { hold })
                    .expect("superbounce.assist_test serialises"))
            },
        );
        // A bounce at a chain count for the first local player, as a real
        // one shows (superbounce_chain.rs, docs/game/superbounce.md,
        // "Medals").
        agent.add_game_method(
            agent_proto::superbounce::MEDAL_TEST,
            Access::Act,
            |params, _ctx| {
                let p: agent_proto::superbounce::MedalTestParams =
                    serde_json::from_value(params)
                        .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
                let r = crate::superbounce_mode::medal_test(&p)
                    .map_err(|why| RpcError::new(code::INVALID_PARAMS, why))?;
                Ok(serde_json::to_value(r).expect("superbounce.medal_test serialises"))
            },
        );
        // A structure BSP's breakable glass: which pieces are broken
        // (bots::guest::breakable; docs/game/bots.md, "Breaking the
        // glass"), to check the bots' break step in a game.
        agent.add_game_method(method::WORLD_BREAKABLE, Access::Read, |params, ctx| {
            let p: agent_proto::BreakableParams = if params.is_null() {
                Default::default()
            } else {
                serde_json::from_value(params)
                    .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
            };
            let bsp = p.bsp.unwrap_or(0);
            let read = ctx
                .read_guest(|m| bots::guest::breakable(m, bsp))?
                .map_err(|e| RpcError::new(code::UNAVAILABLE, format!("{e:?}")))?
                .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "no map loaded"))?;
            Ok(serde_json::to_value(agent_proto::Breakable {
                bsp,
                enabled: read.enabled,
                broken: read.broken(),
                mask: read.mask_hex(),
            })
            .expect("world.breakable serialises"))
        });
        // Trials of the spots in the game (superbounce_verify.rs,
        // docs/game/superbounce.md, "Verifying spots").
        crate::superbounce_verify::install(&agent);
        // The bounce view on the flashlight and the tracker
        // (superbounce_view.rs, docs/game/superbounce.md, "The bounce view").
        crate::superbounce_view::install(&agent);
        // The Pogo Stick's riders, camera and trick test (pogo_agent.rs,
        // docs/game/pogo-stick.md, "Interfaces").
        crate::pogo_agent::install(&agent);
        // Ghost runs of the spots (superbounce_ghost.rs,
        // docs/game/superbounce.md, "Ghost runs").
        crate::superbounce_ghost::install(&agent);
        // Drive player.0 somewhere with the bots' route brain
        // (player_goto.rs, docs/runtime/agent-api.md, "player.goto").
        crate::player_goto::install(&agent);
        // Hand player.0 to a Clanker brain and back (player_bot.rs,
        // docs/runtime/agent-api-sim.md, "player.bot").
        crate::player_bot::install(&agent);
        // HaloScript state: the threads and globals of the running scenario.
        agent::hs_state::install(&agent);
        // On the game thread: the sampler's main target.
        crate::sampling::set_game_thread();
        let mut listeners = Vec::new();
        if endpoint.is_some()
            && let Some(addr) = std::env::var_os(HOST_TCP_ENV).filter(|addr| !addr.is_empty()) {
            let addr = addr
                .into_string()
                .map_err(|_| anyhow::anyhow!("{HOST_TCP_ENV} is not UTF-8"))?;
            listeners.push(listen(&Endpoint::Tcp(addr), &agent)?);
        }
        if let Some(endpoint) = endpoint {
            listeners.push(listen(&endpoint, &agent)?);
        }
        let ticks = match registry.add(&agent::AgentFeature(agent.clone())) {
            Ok(true) => true,
            Ok(false) => {
                agent.set_no_tick_hook();
                eprintln!(
                    "agent: switched off by H2_FEATURES; answering between slices only, no pausing"
                );
                false
            }
            Err(e) => anyhow::bail!("agent: {e}"),
        };
        // render.capture's tick marks and sim.hashes' rows.
        let frames = crate::agent_frames::AgentFrames::default();
        match registry.add(&frames) {
            Ok(true) => frames.views.install_method(&agent),
            Ok(false) => eprintln!(
                "agent: agent-frames switched off by H2_FEATURES; render.capture and sim.hashes see no ticks"
            ),
            Err(e) => anyhow::bail!("agent-frames: {e}"),
        }
        agent::sim::hashes::install(&agent, &agent::sim::hashes::HASHES);
        // theater.start_point.capture and .load: slice 15's experiments.
        crate::theater::start_point_agent::install(&agent);
        match xapi {
            Some(x) => {
                let mut x = x.lock().unwrap_or_else(|e| e.into_inner());
                let chain = if replaying {
                    agent::Chain::Before
                } else {
                    agent::Chain::After
                };
                let inner = x.take_input_tap().map(|t| (t, chain));
                let tap = agent.input_tap(inner);
                crate::ui_press::install(&agent, tap.menu_pad());
                if ticks {
                    crate::agent_route::install(registry, &agent, tap.route_ports(), data_root)?;
                }
                x.set_input_tap(Some(Box::new(tap)));
            }
            None => eprintln!("agent: no XAPI input; the input methods are off"),
        }
        // H2_FRAME_STATS and `h2 play` start their own collection; only
        // start a quiet one when neither will.
        if !timeline::phase::enabled() && timeline::phase::Config::from_env().is_none() {
            let window = timeline::phase::Config::DEFAULT_WINDOW_NS;
            if let Err(e) = timeline::phase::start(timeline::phase::Config::live(window)) {
                eprintln!("agent: frame stats: {e}");
            }
        }
        for listener in &listeners {
            eprintln!(
                "agent: listening on {} (docs/runtime/agent-api.md)",
                listener.endpoint()
            );
        }
        Ok(AgentGlue {
            agent,
            sim: Some(sim),
            _listeners: listeners,
        })
    }

    /// An agent with no socket, for in-process clients (the debug menu,
    /// docs/runtime/debug-menu.md) when `H2_AGENT` is unset: the host
    /// methods, answered between slices. No tick observer (the tick plan
    /// is what it would be without it), no input tap, nothing listening.
    pub fn local() -> AgentGlue {
        let agent = agent::Agent::new();
        add_host_methods(&agent);
        agent.set_no_tick_hook();
        AgentGlue {
            agent,
            sim: None,
            _listeners: Vec::new(),
        }
    }

    /// Observe matchmaking and target-session cleanup for the Live trace.
    pub fn install_matchmaking_trace(&self, registry: &mut hooks::Registry) -> Result<()> {
        matchmaking::install(registry, &self.agent)
    }

    /// The installed plan, for `features.list`. The feature lines are
    /// written from the plan's features in `agent_proto::FeatureState`'s
    /// own form, so the answer's `features` array reads them back exactly
    /// (no client parses the text); the chains and notes follow.
    pub fn set_plan(&self, plan: &hooks::Plan, installed: &hooks::Installed) {
        let mut lines: Vec<String> = plan
            .features
            .iter()
            .map(|f| {
                agent_proto::FeatureState {
                    id: f.id.to_owned(),
                    on: f.off.is_none(),
                    reason: f.off.clone(),
                }
                .plan_line()
            })
            .collect();
        lines.extend(
            plan.text()
                .lines()
                .filter(|l| !l.starts_with("feature "))
                .map(str::to_owned),
        );
        lines.extend(installed.notes.iter().cloned());
        self.agent.set_plan(lines);
    }

    /// Windowed macOS games advertise an enabled agent to the lab even when
    /// the player uses a custom endpoint. `plan` registers the consent
    /// dialog's hooks; `accept` also takes the lab's requests (boot's
    /// `registration::lab_takeover`: a film's recording and its replay
    /// plan the same hooks, and only the player's game accepts).
    pub fn configure_takeover(
        &mut self,
        features: &mut hooks::Registry,
        window: bool,
        plan: bool,
        accept: bool,
    ) -> Result<()> {
        crate::lab_takeover::add_info(&self.agent, window);
        if plan {
            crate::lab_takeover::install(features, &self.agent, accept)?;
        }
        if cfg!(target_os = "macos")
            && window
            && !self._listeners.is_empty()
            && let Some(endpoint) = discoverable_socket().map(Endpoint::Unix)
            && !self._listeners.iter().any(|s| s.endpoint() == endpoint)
        {
            match listen(&endpoint, &self.agent) {
                Ok(listener) => self._listeners.push(listener),
                Err(e) => eprintln!("agent: lab discovery: {e:#}"),
            }
        }
        Ok(())
    }

    /// Publish capabilities once the ordinary tick hooks are installed. Hook
    /// the reload and placement lifecycle only on the first authorized write.
    pub fn install_editor_methods(
        &self,
        plan: &hooks::Plan,
        installed: &hooks::Installed,
        rt: &std::sync::Arc<runtime::Runtime>,
        enabled: bool,
        recording_or_playback: Option<&'static str>,
        skulls: Option<std::sync::Arc<crate::skulls::Tracker>>,
        sources: Option<std::sync::Arc<crate::loaded_map::EditorSources>>,
        rebind: agent::sim::MapRebind,
        before_edit: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    ) {
        let Some(sim) = &self.sim else { return };
        let active = |feature: &str| plan.is_on(feature) && installed.off_reason(feature).is_none();
        if !active("agent") || !active(agent::sim::calls::FEATURE) {
            return;
        }
        if let Some(reason) = recording_or_playback {
            sim.install_reload_refusal(&self.agent, reason);
            return;
        }
        if !enabled {
            return;
        }
        let Some(queue) = sim.calls() else { return };
        let Some(scratch) = plan.allocs.iter().find(|a| a.feature == agent::sim::calls::FEATURE)
            .map(|a| a.addr)
        else { return };
        let Some(shadow) = plan.chains.get(&hooks::points::GAME_TICK.base)
            .and_then(|c| c.shadow).filter(|s| installed.placed.contains(s))
        else { return };
        // A late wrapper can invoke the active handler at entry. A native
        // implementation with separate resume PCs owns additional dispatch
        // state, so leave those chains intact.
        if editor_points().any(|p| {
            rt.resolve_original(p.base).is_none() || plan.chains.get(&p.base)
                .and_then(|chain| chain.implement.as_ref())
                .is_some_and(|hook| active(hook.feature) && !hook.continuation_sites().0.is_empty())
        }) {
            return;
        }
        if active(crate::map_slots::FEATURE) {
            sim.set_map_rebind(rebind);
        }
        let runtime = std::sync::Arc::downgrade(rt);
        sim.on_first_edit(&self.agent, move |agent| {
            // A default film recording cannot replay an outside write.
            if let Some(before) = &before_edit {
                before();
            }
            let rt = runtime.upgrade().ok_or("the game has shut down")?;
            let mut registry = hooks::Registry::new(hooks::Switches::default());
            let sources = sources.clone().map(|sources| -> agent::EditorActivation {
                std::sync::Arc::new(move |guest| sources.activated(guest))
            });
            registry.add(&agent::EditLifecycleFeature(queue.clone(), agent.clone(), sources))
                .map_err(|e| format!("editor lifecycle: {e}"))?;
            let observers = registry.build(&hooks::Base)
                .map_err(|conflicts| format!("editor lifecycle: {conflicts:?}"))?;
            observers.attach_observers(&rt, shadow)?;
            if let Some(tracker) = &skulls {
                tracker.set_editor_queue(scratch, &queue);
            }
            Ok(())
        });
        sim.install_world_edits(&self.agent);
    }

    /// Between slices: answer what's queued.
    pub fn after_slice(&self, rt: &runtime::Runtime, presents: u64) {
        let mut reader = agent::GuestReader::new(|| rt.memory());
        self.agent.between_slices(&mut reader, presents);
    }
}

fn listen(endpoint: &Endpoint, agent: &agent::Agent) -> Result<agent::Server> {
    agent::Server::listen_on(endpoint, agent.clone())
        .with_context(|| format!("agent: listening on {endpoint}"))
}

fn rpc(code: i64) -> impl Fn(String) -> RpcError {
    move |m| RpcError::new(code, m)
}

/// The sampler `profile.sample.*` drives, run over a `profile.capture`
/// window: one sampler, one dump format (`profile.h2prof`, which
/// `h2 profile remote` resolves).
struct BootSampler;

impl agent::Sampler for BootSampler {
    fn start(&mut self) -> std::result::Result<(), String> {
        crate::sampling::start(Value::Null).map(drop)
    }

    fn finish(&mut self) -> std::result::Result<Value, String> {
        crate::sampling::stop(Value::Null)
    }
}

/// Runtime fault counters and tick-boundary collision diagnostics.
pub fn add_runtime_methods(agent: &agent::Agent, rt: &std::sync::Arc<runtime::Runtime>) {
    agent::collision_trace_query::install(agent, rt);
    agent::collision_sources::install(agent, rt);
    let rt = std::sync::Arc::clone(rt);
    let faults = std::sync::Arc::clone(&rt);
    agent.add_method(method::PROFILE_FAULTS, Access::Read, move |v| {
        let top = top_param(&v, 20)?;
        Ok(faults_json(&faults.fault_census().snapshot(), faults.module_features(), top))
    });
    agent.add_method(method::PROFILE_MEMORY_PAGES, Access::Read, move |v| {
        memory_pages(&rt, &v)
    });
}

/// `profile.memory.pages {top, census}`: the pages outside the flat RAM
/// window by reason and owner, and the slow-access census's `top` pages
/// (`runtime::slow_pages`), each as the inline paths see it. `census: true`
/// starts the census from zero, `false` stops it; absent leaves it.
fn memory_pages(rt: &runtime::Runtime, v: &Value) -> std::result::Result<Value, RpcError> {
    let mut rest = v.clone();
    let census = match rest.as_object_mut().and_then(|o| o.remove("census")) {
        None => None,
        Some(Value::Bool(b)) => Some(b),
        Some(_) => return Err(RpcError::new(code::INVALID_PARAMS, "census: true or false")),
    };
    let top = top_param(&rest, 20)?;
    if let Some(on) = census {
        runtime::slow_pages::arm(on);
    }
    let slow = runtime::slow_pages::snapshot(top);
    let memory = rt.memory();
    let mapping = |m: &runtime::guest::Mapping| {
        json!({
            "va": format!("{:#010x}", m.virtual_start),
            "len": m.len,
            "physical": format!("{:#010x}", m.physical_start),
        })
    };
    let runs = memory.outside_runs();
    let mut by_reason = serde_json::Map::new();
    for r in &runs {
        let n = by_reason.entry(r.reason.name()).or_insert(json!(0));
        *n = json!(n.as_u64().unwrap_or(0) + u64::from(r.pages));
    }
    let mut largest: Vec<_> = runs.iter().collect();
    largest.sort_by_key(|r| (std::cmp::Reverse(r.pages), r.va));
    let outside: Vec<Value> = largest
        .into_iter()
        .take(top)
        .map(|r| {
            json!({
                "va": format!("{:#010x}", r.va),
                "pages": r.pages,
                "reason": r.reason.name(),
                "owner": r.owner,
                "mapping": mapping(&r.mapping),
            })
        })
        .collect();
    let pages: Vec<Value> = slow
        .top
        .iter()
        .map(|p| {
            let view = memory.page_view(p.va);
            json!({
                "va": format!("{:#010x}", p.va),
                "reads": p.reads,
                "writes": p.writes,
                "crossings": p.crossings,
                "gate_busy": p.gate_busy,
                "no_view": p.no_view,
                "pc": format!("{:#010x}", p.pc),
                "fast": view.fast.map(|(physical, permissions)| json!({
                    "physical": format!("{physical:#010x}"),
                    "read": permissions & 1 != 0,
                    "write": permissions & 2 != 0,
                })),
                "trapped": {"read": view.trapped.0, "write": view.trapped.1},
                "outside": view.outside.map(|r| r.name()),
                "owner": view.owner,
                "mapping": view.mapping.as_ref().map(mapping),
            })
        })
        .collect();
    Ok(json!({
        "outside": {
            "pages": runs.iter().map(|r| u64::from(r.pages)).sum::<u64>(),
            "runs": runs.len(),
            "by_reason": by_reason,
            "largest": outside,
        },
        "census": {
            "armed": slow.armed,
            "reads": slow.reads,
            "writes": slow.writes,
            "pages": slow.pages,
            "top": pages,
        },
    }))
}

/// `{"top": n}` (1 to 500, `default` when absent), and no other field.
fn top_param(v: &Value, default: usize) -> std::result::Result<usize, RpcError> {
    if let Some(o) = v.as_object()
        && let Some(k) = o.keys().find(|k| *k != "top")
    {
        return Err(RpcError::new(code::INVALID_PARAMS, format!("unknown field {k}")));
    }
    match v.get("top") {
        None => Ok(default),
        Some(n) => n
            .as_u64()
            .filter(|n| (1..=500).contains(n))
            .map(|n| n as usize)
            .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "top: 1 to 500")),
    }
}

/// `profile.faults`' answer: the census totals, its verdict, the module's
/// clock and fault precision (from its `features`), and the `top` fault PCs
/// by count.
pub fn faults_json(c: &runtime::fault_census::FaultCounts, features: u32, top: usize) -> Value {
    use runtime::generated::{FEATURE_BLOCK_CLOCK, FEATURE_IMPRECISE_FAULTS};
    let sites: Vec<Value> = c
        .sites
        .iter()
        .take(top)
        .map(|s| {
            json!({
                "pc": format!("{:#010x}", s.pc),
                "source": s.source.name(),
                "count": s.count,
                "continued": s.continued,
                "addr": format!("{:#010x}", s.first.addr),
                "len": s.first.len,
                "access": format!("{:?}", s.first.access).to_lowercase(),
                "reason": format!("{:?}", s.first.reason),
            })
        })
        .collect();
    json!({
        "generated": c.generated,
        "runtime": c.runtime,
        "ended": c.ended,
        "continued": c.continued,
        "continued_generated": c.continued_generated,
        "continued_imprecise": c.continued_imprecise,
        "precise_exits_needed": c.precise_exits_needed(),
        "imprecise_faults": features & FEATURE_IMPRECISE_FAULTS != 0,
        "clock": if features & FEATURE_BLOCK_CLOCK != 0 { "blocks" } else { "accesses" },
        "sites": c.sites.len(),
        "sites_full": c.sites_full,
        "top": sites,
    })
}

/// The host methods (module docs).
pub fn add_host_methods(agent: &agent::Agent) {
    crate::lab_takeover::add_info(agent, false);
    crate::sim_bots::install_pair_reader();
    agent.add_game_method(agent_proto::sim::method::BOT_STATS, Access::Read, |v, _| {
        crate::sim_bots::stats(v, bots::phantom::global())
    });
    agent.add_method(agent_proto::bot_bounce::BOUNCE_TO, Access::Act, |v| {
        crate::sim_bots::bounce_to(v, bots::phantom::global())
    });
    agent.add_method(agent_proto::bot_bounce::BOUNCE_STATUS, Access::Read, |v| {
        crate::sim_bots::bounce_status(v, bots::phantom::global())
    });
    agent.add_method(agent_proto::superbounce::SPOTS, Access::Read, crate::sim_bots::spots);
    agent.add_method(method::UI_BROWSER_FILTERS, Access::Read, |params| {
        if !params.is_null() && !params.as_object().is_some_and(|fields| fields.is_empty()) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "ui.browser.filters takes no parameters",
            ));
        }
        let filters = crate::custom_games::browser_filters::active_filters();
        Ok(json!({ "filters": filters }))
    });
    agent.add_method(method::LIVE_JOIN_OPEN, Access::Act, |params| {
        use live_hle::join_link::{self, Source, SubmitError};
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Open {
            url: String,
        }
        let open: Open = serde_json::from_value(params)
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
        let link = open.url.parse::<live_proto::JoinLink>()
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
        let id = join_link::submit(link, Source::External).map_err(|e| {
            let code = match e {
                SubmitError::Busy => code::BUSY,
                SubmitError::Unavailable => code::UNAVAILABLE,
            };
            RpcError::new(code, e.to_string())
        })?;
        Ok(json!({ "id": id }))
    });
    agent.add_method(method::LIVE_JOIN_STATUS, Access::Read, |params| {
        if !params.is_null() && !params.as_object().is_some_and(|fields| fields.is_empty()) {
            return Err(RpcError::new(code::INVALID_PARAMS, "Expected no parameters."));
        }
        Ok(json!({ "join": live_hle::join_link::status() }))
    });
    if crate::sampling::available() {
        agent.add_method(method::SAMPLE_START, Access::Act, |v| {
            crate::sampling::start(v).map_err(rpc(code::UNAVAILABLE))
        });
        agent.add_method(method::SAMPLE_STOP, Access::Act, |v| {
            crate::sampling::stop(v).map_err(rpc(code::UNAVAILABLE))
        });
        agent.set_sampler(Box::new(BootSampler));
    }
    #[cfg(feature = "desktop-runtime")]
    agent::render::add_methods(agent, &agent::render::PROBE, crate::render_probe::sources());
    // render.compare, render.capture (fed by d3d_diag's presents, the
    // H2_BOOT_GFX present hook and crate::agent_frames), render.scene and
    // render.parity (their sources register on agent::render::HOSTED).
    #[cfg(feature = "desktop-runtime")]
    agent::render::add_frame_methods(agent, &agent::render::CAPTURE, &agent::render::HOSTED);
    #[cfg(feature = "gpu")]
    agent::render::clip::add_methods(agent);
    agent.add_method(method::PROFILE_FALLBACKS, Access::Read, |v| {
        let top = match v.get("top") {
            None => 20,
            Some(n) => n
                .as_u64()
                .filter(|n| (1..=500).contains(n))
                .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "top: 1 to 500"))?
                as usize,
        };
        if let Some(o) = v.as_object()
            && let Some(k) = o.keys().find(|k| *k != "top")
        {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("unknown field {k}"),
            ));
        }
        let stats = crate::fallback::STATS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default();
        Ok(stats.sites_json(top))
    });
    // The flat RAM window's counters, summed over the process's runtimes
    // (docs/runtime/contract-thin-transfers-flat-ram.md, slice F2).
    agent.add_method(method::PROFILE_MEMORY, Access::Read, |_| {
        let w = runtime::window_process_stats();
        Ok(json!({
            "windows": w.windows,
            "aliased": w.aliased,
            "fixups": w.fixups,
            "pages_outside": w.pages_outside,
            "congruence_skipped": w.congruence_skipped,
        }))
    });
    // Shadow checks of the native ports (docs/engine/shadow-exec.md).
    agent::shadow::add_methods(agent);
    agent.add_method(method::SAMPLE_STATUS, Access::Read, |_| {
        Ok(crate::sampling::status())
    });
    agent.add_method(
        method::HOST_PERF,
        Access::Read,
        |_| match crate::host_perf::last_line() {
            Some((age, line)) => Ok(json!({ "line": line, "age_ms": age.as_millis() as u64 })),
            None => Err(RpcError::new(
                code::UNAVAILABLE,
                "no host perf line yet (H2_HOST_PERF=1; one every 100 frames or 5 s)",
            )),
        },
    );
    crate::ui_audit::install(agent);
    agent.add_game_method(method::UI_CAMPAIGN, Access::Read, |_, ctx| {
        ctx.read_guest(crate::campaign_menu::inspect)
    });
    agent.add_game_method(method::UI_LOBBY_OPTIONS, Access::Read, |params, ctx| {
        if !params.is_null() && !params.as_object().is_some_and(|p| p.is_empty()) {
            return Err(RpcError::new(code::INVALID_PARAMS, "expected no parameters"));
        }
        ctx.read_guest(crate::lobby_options::inspect)?
            .map(|lines| json!({"lines": lines}))
            .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "no active custom game options panel"))
    });
    agent.add_game_method(method::HOST_PEER_RTTS, Access::Read, |params, ctx| {
        if !params.is_null() && !params.as_object().is_some_and(|p| p.is_empty()) {
            return Err(RpcError::new(code::INVALID_PARAMS, "host.peer_rtts takes no parameters"));
        }
        ctx.read_guest(crate::lobby_quality::match_peer_rtts)?
    });
    agent.add_game_method(method::UI_LOBBY_TABS, Access::Read, |params, ctx| {
        if !params.is_null() && !params.as_object().is_some_and(|p| p.is_empty()) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "ui.lobby.tabs takes no parameters",
            ));
        }
        ctx.read_guest(crate::lobby_tabs::header::inspect)?
            .map_err(|why| RpcError::new(code::UNAVAILABLE, why))
    });
    agent.add_game_method(method::UI_LOBBY_PLAYERS, Access::Read, |params, ctx| {
        if !params.is_null() && !params.as_object().is_some_and(|p| p.is_empty()) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "ui.lobby.players takes no parameters",
            ));
        }
        ctx.read_guest(crate::lobby_quality::inspect)?
    });
    agent.add_method(
        method::UI_SCREEN,
        Access::Read,
        |_| {
            crate::ui_watch::last_json().ok_or_else(|| {
                RpcError::new(
                    code::UNAVAILABLE,
                    "no screen read yet (H2_UI_WATCH=1; read on each game tick)",
                )
            })
        },
    );
}

/// `sim.*` (crates/extensions/agent's `sim`): its players are the process's phantom
/// bots ([`crate::sim_bots`]), and the session kind is read as `coop_probe`
/// reads it.
fn install_sim(agent: &agent::Agent, allow_bot_add: bool) -> agent::sim::SimHandle {
    agent::sim::install(
        agent,
        Box::new(crate::sim_bots::SimBots::new(allow_bot_add)),
        Some(Box::new(session_view)),
    )
}

/// [`crate::layout_glue::Gm`] over the agent's guest memory.
struct Mem<'a>(&'a dyn guest_layout::GuestMemory);

impl crate::layout_glue::Gm for Mem<'_> {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.0.read(addr, out).is_ok()
    }
    fn wr(&mut self, _: u32, _: &[u8]) -> bool {
        false
    }
}

/// Called on an external Live service, at gameplay boundaries. Matchmaking plays
/// in its target session, custom games in the squad. Refuse an unreadable
/// session until the next probe can positively identify its network class.
pub fn remote_live_game(m: &dyn guest_layout::GuestMemory) -> bool {
    use bots::lobby;
    use crate::variant_ext::{SESSION_NETWORK_CLASS, SYSTEM_LINK};

    let local = || -> Option<bool> {
        if m.u8(lobby::NETWORKING).ok()? == 0 {
            return Some(true);
        }
        let cycle = m.u32(lobby::SESSION_TYPE).ok()?;
        let session = m.u32(lobby::game_session_at(cycle)).ok()?;
        if session == 0 || m.u32(session.checked_add(lobby::STATE)?).ok()? == 0 {
            return Some(true);
        }
        let class = m.u32(session.checked_add(SESSION_NETWORK_CLASS)?).ok()?;
        Some(matches!(class, 0 | SYSTEM_LINK))
    };
    local() != Some(true)
}

/// Whether edits are refused on an external Live game ([`remote_live_game`]
/// refuses the live stream on every one). A Live custom game this console
/// hosts takes edits as a System Link host does: other machines behind
/// `confirm_peers`, a placement count change only with no other people.
/// Matchmaking (life-cycle types 6 to 9, whoever hosts), a client and a
/// session that cannot be read refuse.
pub fn live_edit_refusal(m: &dyn guest_layout::GuestMemory) -> Option<&'static str> {
    use bots::lobby;

    if !remote_live_game(m) {
        return None;
    }
    let Ok(ty) = m.u32(lobby::SESSION_TYPE) else {
        return Some("the Live session cannot be read");
    };
    if (6..=9).contains(&ty) {
        return Some("a matchmaking game");
    }
    if !(1..=4).contains(&ty) {
        return Some("the Live session cannot be identified as a custom game");
    }
    let host = m.u32(lobby::SESSION).ok().filter(|&s| s != 0).and_then(|s| {
        Some(m.u32(s.checked_add(lobby::HOST_PEER)?).ok()? == m.u32(s.checked_add(lobby::PEER)?).ok()?)
    });
    match host {
        Some(true) => None,
        Some(false) => Some("a Live game another console hosts"),
        None => Some("the Live session cannot be read"),
    }
}

/// The session kind from the simulation mode, and the squad session
/// (`coop_probe::read_sim`, `read_session`).
pub fn session_view(m: &dyn guest_layout::GuestMemory) -> agent_proto::sim::SessionView {
    use crate::coop_probe::{protocol_name, read_session, read_sim};
    let m = Mem(m);
    let sim = read_sim(&m);
    let s = read_session(&m);
    agent_proto::sim::SessionView {
        kind: sim.map_or(agent_proto::sim::SessionKind::Unknown, |s| {
            agent_proto::sim::SessionKind::from_sim_mode(s.mode)
        }),
        sim_mode: sim.map(|s| s.mode),
        host: s.map(|s| s.is_host()),
        peers: s.map(|s| s.peers),
        players: s.map(|s| s.players),
        protocol: s.map(|s| protocol_name(s.protocol).into()),
        campaign: s.map(|s| s.is_campaign()),
        ..Default::default()
    }
}

impl Drop for AgentGlue {
    fn drop(&mut self) {
        self.agent.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Build the real tick features, supply their final install result, and
    /// ask the public registry which editor operations this host offers.
    fn editor_methods(
        theater: bool,
        input_replay: bool,
        recording: bool,
        off: Option<&'static str>,
        missing: Option<&'static str>,
    ) -> Vec<&'static str> {
        use agent_proto::sim::method::{EDITS, MAP_RELOAD};

        struct Image(Option<&'static str>);
        impl hooks::ImageAddrs for Image {
            fn resolve(&self, point: &hooks::points::Point) -> Option<u32> {
                (self.0 != Some(point.name)).then_some(point.base)
            }
        }
        let agent = agent::Agent::new();
        let sim = install_sim(&agent, true);
        let calls = agent::sim::calls::shared();
        let mut registry = hooks::Registry::new(hooks::Switches::default());
        registry
            .add(&agent::sim::calls::CallsFeature(calls.clone()))
            .unwrap();
        registry.add(&agent::AgentFeature(agent.clone())).unwrap();
        sim.set_calls(calls);
        let glue = AgentGlue {
            agent,
            sim: Some(sim),
            _listeners: Vec::new(),
        };
        let plan = registry.build(&Image(missing)).unwrap();
        assert!(plan.is_on("agent"), "the ordinary tick agent stays available");
        let mut d = runtime::Dispatch::default();
        for point in editor_points() {
            if missing != Some(point.name) {
                d.register_generated_rust(point.base, |_, _, _| unreachable!()).unwrap();
            }
        }
        let rt = runtime::Runtime::new(runtime::Guest::new(), d);
        let mut installed = hooks::Installed::default();
        installed.placed.push(plan.chains[&hooks::points::GAME_TICK.base].shadow.unwrap());
        if let Some(feature) = off {
            installed.off.push((feature, "missing generated tick entry".into()));
        }
        let blocked = if recording {
            Some("map reload is disabled while recording a film")
        } else if theater || input_replay {
            Some("map reload is disabled during playback")
        } else {
            None
        };
        glue.install_editor_methods(
            &plan, &installed, &rt, off != Some(agent::EDIT_LIFECYCLE), blocked, None, None,
            Box::new(|_, _, _, _| panic!("a playback or uninstalled hook must not prepare a map")),
            None,
        );
        assert!(!plan.is_on(agent::EDIT_LIFECYCLE));
        assert!(rt.read_dispatch(|d| d.override_name(hooks::points::GAME_ACTIVATE.base).is_none()));
        let mem = Mutex::new(runtime::Guest::new());
        let mut reader = agent::GuestReader::new(|| mem.lock().unwrap());
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let conn = glue.agent.connect(tx);
        let mut call = |method: &str| -> Value {
            let request = json!({"jsonrpc": "2.0", "id": 1, "method": method});
            glue.agent.submit(conn, &request.to_string());
            glue.agent.on_tick(&mut reader);
            serde_json::from_str(&rx.try_recv().unwrap()).unwrap()
        };
        let response = call(method::METHODS);
        let methods = response["result"].as_array().unwrap();
        if off.is_none() && missing.is_none() && (theater || input_replay || recording) {
            let error = call(MAP_RELOAD);
            assert_eq!(error["error"]["code"], agent_proto::sim::REFUSED);
            assert_eq!(
                error["error"]["data"]["why"],
                if recording {
                    "map reload is disabled while recording a film"
                } else {
                    "map reload is disabled during playback"
                }
            );
        }
        EDITS.iter()
            .copied()
            .filter(|name| methods.contains(&json!(name)))
            .collect()
    }

    #[test]
    fn theater_without_an_input_replay_offers_only_the_reload_refusal() {
        let reload = [agent_proto::sim::method::MAP_RELOAD];
        assert_eq!(editor_methods(true, false, false, None, None), reload);
        assert_eq!(editor_methods(false, true, false, None, None), reload);
    }

    #[test]
    fn a_launch_recording_input_refuses_reloads_while_a_trace_only_launch_can_edit() {
        let mut config = crate::lockstep_glue::Config {
            trace: Some("fixture.trace".into()),
            ..Default::default()
        };
        assert_eq!(reload_recording_refusal(false, false, &config), None);
        config.record = Some("fixture.input".into());
        assert_eq!(
            reload_recording_refusal(false, false, &config),
            Some("map reload is disabled while recording input")
        );
    }

    #[test]
    fn film_recording_offers_a_reload_refusal_with_the_recording_reason() {
        assert_eq!(
            editor_methods(false, false, true, None, None),
            [agent_proto::sim::method::MAP_RELOAD],
        );
    }

    #[test]
    fn both_installed_tick_features_are_required_for_editor_methods() {
        assert_eq!(editor_methods(false, false, false, None, None), agent_proto::sim::method::EDITS);
        for feature in ["agent", agent::sim::calls::FEATURE, agent::EDIT_LIFECYCLE] {
            assert!(editor_methods(false, false, false, Some(feature), None).is_empty());
            if feature != agent::EDIT_LIFECYCLE {
                assert!(editor_methods(true, false, false, Some(feature), None).is_empty());
            }
        }
    }

    #[test]
    fn missing_each_edit_lifecycle_observer_leaves_the_base_tick_available() {
        for point in [&hooks::points::GAME_ACTIVATE, &hooks::points::GAME_STATE_REVERT] {
            assert!(editor_methods(false, false, false, None, Some(point.name)).is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn host_tcp_listener_shares_the_unix_agent() {
        use std::time::Instant;

        struct Empty;
        impl agent::Reader for Empty {
            fn status(&mut self) -> agent::GameStatus {
                agent::GameStatus::default()
            }

            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
                Ok(Vec::new())
            }
        }

        let glue = AgentGlue::local();
        let mut count = 0;
        glue.agent.add_method("test.count", Access::Act, move |_| {
            count += 1;
            Ok(json!(count))
        });
        let path = std::env::temp_dir().join(format!(
            "h2h-{}.sock",
            live_proto::sign::random_id()
        ));
        let unix = listen(&Endpoint::Unix(path), &glue.agent).unwrap();
        let tcp = listen(&Endpoint::Tcp("127.0.0.1:0".into()), &glue.agent).unwrap();
        let endpoints = [unix.endpoint(), tcp.endpoint()];
        let clients = std::thread::spawn(move || {
            endpoints.map(|endpoint| {
                let mut client = agent_proto::Client::connect_to(&endpoint).unwrap();
                client.set_timeout(Some(Duration::from_secs(2))).unwrap();
                client.call("test.count", json!({})).unwrap()
            })
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        while !clients.is_finished() {
            assert!(Instant::now() < deadline, "agent calls did not finish");
            glue.agent.between_slices(&mut Empty, 0);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(clients.join().unwrap(), [json!(1), json!(2)]);
    }

    #[test]
    fn remote_live_probe_uses_the_active_session_and_refuses_unreadable_state() {
        use bots::lobby;
        use crate::variant_ext::{SESSION_NETWORK_CLASS, SYSTEM_LINK};
        let mut g = guest_layout::testing::FakeGuest::new();
        assert!(remote_live_game(&g));
        let squad = 0x4000;
        let target = 0x14000;
        g.map(lobby::NETWORKING, 0x80)
            .put_u8(lobby::NETWORKING, 1)
            .put_u32(lobby::SESSION, squad)
            .put_u32(lobby::TARGET_SESSION, target);
        for (addr, class) in [(squad, SYSTEM_LINK), (target, 2)] {
            g.map(addr, 0x8000)
                .put_u32(addr + lobby::STATE, 5)
                .put_u32(addr + SESSION_NETWORK_CLASS, class);
        }
        g.put_u32(lobby::SESSION_TYPE, 3);
        assert!(!remote_live_game(&g));
        g.put_u32(lobby::SESSION_TYPE, 8);
        assert!(remote_live_game(&g));
        g.put_u8(lobby::NETWORKING, 0);
        assert!(!remote_live_game(&g));
    }

    #[test]
    fn live_edits_are_open_to_a_custom_game_host_only() {
        use bots::lobby;
        use crate::variant_ext::{SESSION_NETWORK_CLASS, SYSTEM_LINK};
        let squad = 0x4000;
        let target = 0x14000;
        let scene = |ty: u32, class: u32, squad_host: bool, target_host: bool| {
            let mut g = guest_layout::testing::FakeGuest::new();
            g.map(lobby::NETWORKING, 0x80)
                .put_u8(lobby::NETWORKING, 1)
                .put_u32(lobby::SESSION, squad)
                .put_u32(lobby::TARGET_SESSION, target)
                .put_u32(lobby::SESSION_TYPE, ty);
            for (addr, host) in [(squad, squad_host), (target, target_host)] {
                g.map(addr, 0x8000)
                    .put_u32(addr + lobby::STATE, 5)
                    .put_u32(addr + SESSION_NETWORK_CLASS, class)
                    .put_u32(addr + lobby::PEER, 1)
                    .put_u32(addr + lobby::HOST_PEER, if host { 1 } else { 0 });
            }
            g
        };
        // Live custom game: its host is admitted, a client is not.
        assert_eq!(live_edit_refusal(&scene(3, 2, true, false)), None);
        assert_eq!(live_edit_refusal(&scene(1, 2, true, false)), None);
        assert!(live_edit_refusal(&scene(3, 2, false, true)).unwrap().contains("another console"));
        // Matchmaking is refused, hosted here or not.
        for host in [true, false] {
            assert_eq!(live_edit_refusal(&scene(8, 2, false, host)), Some("a matchmaking game"));
            assert_eq!(live_edit_refusal(&scene(6, 2, false, host)), Some("a matchmaking game"));
        }
        // System Link and a local game are not Live at all.
        assert_eq!(live_edit_refusal(&scene(3, SYSTEM_LINK, false, false)), None);
        let mut g = scene(3, 2, true, false);
        g.put_u8(lobby::NETWORKING, 0);
        assert_eq!(live_edit_refusal(&g), None);
        // An unreadable game is refused.
        assert!(live_edit_refusal(&guest_layout::testing::FakeGuest::new()).is_some());
        assert!(live_edit_refusal(&scene(5, 2, true, true)).unwrap().contains("custom game"));
    }

    #[test]
    fn the_sim_session_kind_comes_from_the_simulation_mode() {
        use agent_proto::sim::SessionKind;
        let mut g = guest_layout::testing::FakeGuest::new();
        assert_eq!(session_view(&g).kind, SessionKind::Unknown);
        let globals = 0x9000_0000;
        g.map(guest_layout::points::SIM_GLOBALS.base, 4);
        g.put_u32(guest_layout::points::SIM_GLOBALS.base, globals);
        g.map(globals, 0x40);
        for (mode, kind) in [
            (2, SessionKind::CoopHost),
            (3, SessionKind::CoopGuest),
            (4, SessionKind::MpHost),
            (5, SessionKind::MpClient),
            (1, SessionKind::Local),
        ] {
            g.put_u32(globals + 8, mode);
            let v = session_view(&g);
            assert_eq!((v.kind, v.sim_mode), (kind, Some(mode)));
            assert_eq!(v.host, None, "no squad session");
        }
    }

    #[test]
    fn the_glue_answers_pad_and_profiler_methods() {
        let a = agent::Agent::new();
        add_host_methods(&a);
        let _tap = a.input_tap(None);
        let (tx, rx) = std::sync::mpsc::sync_channel(16);
        let c = a.connect(tx);
        struct Nothing;
        impl agent::Reader for Nothing {
            fn status(&mut self) -> agent::GameStatus {
                agent::GameStatus::default()
            }
            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
                Ok(vec![])
            }
        }
        let call = |m: &str, p: Value| -> Value {
            a.submit(
                c,
                &json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).to_string(),
            );
            a.between_slices(&mut Nothing, 0);
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
        };
        let r = call(method::UI_LOBBY_PLAYERS, json!({"unknown": true}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        let r = call(method::UI_LOBBY_TABS, json!({"unknown": true}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        let r = call(method::UI_BROWSER_FILTERS, json!({"kind": "coop"}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        let r = call(method::UI_BROWSER_FILTERS, Value::Null);
        assert_eq!(
            r["result"]["filters"],
            serde_json::to_value(crate::custom_games::browser_filters::active_filters()).unwrap()
        );
        let r = call(agent_proto::sim::method::BOT_STATS, json!({}));
        assert!(r["result"]["bots"].is_array(), "{r}");
        let r = call(method::RENDER_STATS, Value::Null);
        assert!(r["result"].as_object().is_some_and(|o| o.contains_key("counters")), "{r}");
        let r = call(method::PROFILE_FALLBACKS, json!({"top": 3}));
        assert!(r["result"]["top"].is_array(), "{r}");
        let r = call(method::PROFILE_FALLBACKS, json!({"top": 0}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        let rt = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
        add_runtime_methods(&a, &rt);
        let r = call(method::PROFILE_FAULTS, json!({"top": 3}));
        assert_eq!(r["result"]["generated"], 0, "{r}");
        assert_eq!(r["result"]["precise_exits_needed"], false, "{r}");
        assert_eq!(r["result"]["clock"], "accesses", "{r}");
        assert_eq!(r["result"]["imprecise_faults"], false, "{r}");
        assert!(r["result"]["top"].is_array(), "{r}");
        let r = call(method::PROFILE_FAULTS, json!({"top": 3, "x": 1}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        {
            let mut g = rt.memory();
            // Residue 0x1000 under a residue-0 page: outside on any host
            // with 16 KiB pages, inside on 4 KiB ones.
            g.map(0x0002_0000, 0x1000, 0x0020_1000, runtime::guest::Permissions::RW).unwrap();
        }
        let r = call(method::PROFILE_MEMORY_PAGES, json!({"top": 5, "census": false}));
        assert_eq!(r["result"]["census"]["armed"], false, "{r}");
        assert!(r["result"]["census"]["top"].is_array(), "{r}");
        assert!(r["result"]["outside"]["largest"].is_array(), "{r}");
        let outside = r["result"]["outside"]["pages"].as_u64().unwrap();
        assert!(outside <= 1, "{r}");
        if outside == 1 {
            assert_eq!(r["result"]["outside"]["by_reason"]["non_congruent"], 1, "{r}");
            assert_eq!(r["result"]["outside"]["largest"][0]["va"], "0x00020000", "{r}");
        }
        let r = call(method::PROFILE_MEMORY_PAGES, json!({"census": 1}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        let r = call(method::PROFILE_MEMORY_PAGES, json!({"x": 1}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        let r = call(method::RENDER_FRAME, Value::Null);
        assert_eq!(r["result"]["armed"], false, "{r}");
        let r = call(method::INPUT_STATE, Value::Null);
        assert_eq!(r["result"]["pad"]["active"], false, "{r}");
        let r = call(method::SAMPLE_STATUS, Value::Null);
        assert!(r["result"]["available"].is_boolean(), "{r}");
        let r = call(method::INPUT_PAD, json!({"steps": [{"buttons": ["nope"]}]}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        let r = call(
            method::INPUT_PAD,
            json!({"steps": [{"buttons": ["a"], "ms": 50}]}),
        );
        assert_eq!(r["result"]["queued"], 1, "{r}");
    }
}
