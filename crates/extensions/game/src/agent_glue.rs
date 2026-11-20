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

use std::path::PathBuf;
use std::sync::Mutex;

use agent::Access;
use agent_proto::{Endpoint, RpcError, code, method};
use anyhow::{Context, Result};
use serde_json::{Value, json};

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
    if !cfg!(unix) || agent_proto::discover::off_from_env() {
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
    ) -> Result<AgentGlue> {
        Self::with_endpoint(registry, Some(Endpoint::from(path)), xapi, replaying, allow_bot_add)
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
        // render.scene: the frame step's FrameScene (docs/graphics/framescene.md).
        #[cfg(feature = "gpu")]
        crate::scene_glue::install();
        let sim = install_sim(&agent, allow_bot_add);
        // The game's own functions for sim (teleports, give, spawn): a
        // `before` on the tick that makes the calls queued for it.
        let calls = agent::sim::calls::shared();
        match registry.add(&agent::sim::calls::CallsFeature(calls.clone())) {
            Ok(true) => sim.set_calls(calls),
            Ok(false) => eprintln!(
                "agent: agent-calls switched off by H2_FEATURES; sim teleports, give and spawn are off"
            ),
            Err(e) => anyhow::bail!("agent-calls: {e}"),
        }
        agent::ai::install(&agent);
        agent::havok::install(&agent);
        // The native collision world's counters (impl.sim.native.collision).
        agent::collision::install(&agent);
        agent::raw::install(&agent, &sim);
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
        match registry.add(&agent::AgentFeature(agent.clone())) {
            Ok(true) => {}
            Ok(false) => {
                agent.set_no_tick_hook();
                eprintln!(
                    "agent: switched off by H2_FEATURES; answering between slices only, no pausing"
                )
            }
            Err(e) => anyhow::bail!("agent: {e}"),
        }
        // render.capture's tick marks and sim.hashes' rows.
        match registry.add(&crate::agent_frames::AgentFrames) {
            Ok(true) => {}
            Ok(false) => eprintln!(
                "agent: agent-frames switched off by H2_FEATURES; render.capture and sim.hashes see no ticks"
            ),
            Err(e) => anyhow::bail!("agent-frames: {e}"),
        }
        agent::sim::hashes::install(&agent, &agent::sim::hashes::HASHES);
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

    /// The installed plan, for `features.list`.
    pub fn set_plan(&self, plan: &hooks::Plan, installed: &hooks::Installed) {
        let mut lines: Vec<String> = plan.text().lines().map(str::to_owned).collect();
        lines.extend(installed.notes.iter().cloned());
        self.agent.set_plan(lines);
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

/// The methods that read the runtime itself: `profile.faults`, the fault
/// census (`runtime::fault_census`).
pub fn add_runtime_methods(agent: &agent::Agent, rt: &std::sync::Arc<runtime::Runtime>) {
    let rt = std::sync::Arc::clone(rt);
    agent.add_method(method::PROFILE_FAULTS, Access::Read, move |v| {
        let top = top_param(&v, 20)?;
        Ok(faults_json(&rt.fault_census().snapshot(), rt.module_features(), top))
    });
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
    agent.add_method(method::LIVE_JOIN_OPEN, Access::Act, |params| {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Open {
            url: String,
        }
        let open: Open = serde_json::from_value(params)
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
        let link = open.url.parse::<live_proto::JoinLink>()
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
        let id = live_hle::join_link::submit(link).map_err(|e| {
            use live_hle::join_link::SubmitError;
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
        |_| match crate::ui_watch::last() {
            Some((age, top, dialog)) => Ok(crate::ui_watch::screen_json(age, top, dialog)),
            None => Err(RpcError::new(
                code::UNAVAILABLE,
                "no screen read yet (H2_UI_WATCH=1; read on each game tick)",
            )),
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
struct Mem<'a>(&'a dyn engine::GuestMemory);

impl crate::layout_glue::Gm for Mem<'_> {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.0.read(addr, out).is_ok()
    }
    fn wr(&mut self, _: u32, _: &[u8]) -> bool {
        false
    }
}

/// The session kind from the simulation mode, and the squad session
/// (`coop_probe::read_sim`, `read_session`).
pub fn session_view(m: &dyn engine::GuestMemory) -> agent_proto::sim::SessionView {
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
    fn the_sim_session_kind_comes_from_the_simulation_mode() {
        use agent_proto::sim::SessionKind;
        let mut g = engine::testing::FakeGuest::new();
        assert_eq!(session_view(&g).kind, SessionKind::Unknown);
        let globals = 0x9000_0000;
        g.map(engine::points::SIM_GLOBALS.base, 4);
        g.put_u32(engine::points::SIM_GLOBALS.base, globals);
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
