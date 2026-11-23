//! The `firefight` feature (docs/game/firefight.md, "The host runtime"):
//! the runtime at the game's start and at each tick boundary, and the
//! agent API's view of it.
//!
//! On by default; `-firefight` turns it off. On, it starts the mode at each
//! multiplayer game's start where the map carries the kit, and otherwise
//! keeps the refusal and its reason for `firefight.state`. Its hooks:
//!
//! - `GAME_ACTIVATE`, a `before` while the options are not yet the
//!   game's: two Firefight teams on a kit map turn the variant's team play
//!   on ([`Runtime::activate`]).
//! - `GAME_ACTIVATE`, an observer after a successful start: start or
//!   refuse the mode ([`Runtime::start`]).
//! - `GAME_TICK`, a `before` in the default class: apply the due entries;
//!   when they need engine calls, or a sight pass is due, answer with an
//!   async body that makes them and then runs the tick (re-entering its
//!   chain, which this hook then lets pass). It sits inside the co-op
//!   hooks' lobby setups and our skulls' calls, which re-enter the tick
//!   too; the first-class hooks (the agent's calls, our skulls' calls, the
//!   map offers) run outside it, the last-class ones (the sim harness, the
//!   Forge monitor) inside.
//! - `GAME_TICK`, an observer after: step the director ([`Runtime::post_tick`]).

use std::collections::BTreeSet;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use ::firefight::Skull;
use agent_proto::firefight as wire;
use engine::GuestMemory;
use engine::entry::EntryFuture;

use super::runtime::{self, Runtime};
use crate::layout_glue::Gm;

/// The feature id in the plan and `H2_FEATURES`.
pub const FEATURE: &str = "firefight";

/// The runtime and the tick hook's re-entry guard.
pub struct Shared {
    runtime: Mutex<Runtime>,
    /// Set while the async body runs the tick it answered for.
    pub reentry: AtomicBool,
    /// This run plays a film: the machine replays the host's entries.
    film: bool,
}

impl Shared {
    pub fn new(film: bool, sink: Box<runtime::SkullSink>) -> Self {
        Self {
            runtime: Mutex::new(Runtime::new(sink)),
            reentry: AtomicBool::new(false),
            film,
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Runtime> {
        self.runtime.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Guest memory as `skulls_mp`'s readers and writers take it.
struct WriteGm<'a>(&'a mut dyn GuestMemory);

impl Gm for WriteGm<'_> {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.0.read(addr, out).is_ok()
    }
    fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
        self.0.write(addr, data).is_ok()
    }
}

/// The director's skulls into the engine: `skulls_mp`'s Firefight set for
/// this machine's role, then its AI bytes in the skull array.
pub fn write_skulls(on: &BTreeSet<Skull>, m: &mut dyn GuestMemory) -> anyhow::Result<()> {
    let mp = crate::skulls_mp::shared().ok_or_else(|| {
        anyhow::anyhow!("the multiplayer skulls are not installed (no settings store)")
    })?;
    let role = crate::skulls_mp::role_in(&crate::layout_glue::ReadGm(&*m));
    mp.set_firefight(&role, on);
    anyhow::ensure!(
        mp.write_firefight_array(&mut WriteGm(m)),
        "the skull array did not take the write"
    );
    Ok(())
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// The feature.
pub struct Firefight(pub Arc<Shared>);

fn manifest() -> hooks::Manifest {
    hooks::Manifest::new(
        FEATURE,
        "Firefight: the director and its host log on a kit map",
    )
    .doc("docs/game/firefight.md")
}

/// Whether the `firefight` feature is on under `switches`, by its
/// manifest's default: the main menu's FIREFIGHT row shows with it.
pub fn enabled(switches: &hooks::Switches) -> bool {
    let m = manifest();
    switches.feature(m.id, m.default_on)
}

impl hooks::Feature for Firefight {
    fn manifest(&self) -> hooks::Manifest {
        manifest()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::Flow;
        use std::sync::atomic::Ordering;
        // The two points of a line-of-sight test.
        let scratch = b.guest_alloc(0x20, ::runtime::guest::Permissions::RW)?;
        let s = Arc::clone(&self.0);
        b.raw().before(
            &hooks::points::GAME_ACTIVATE,
            "Team Play on for two Firefight teams",
            move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let options = c.cpu.eax;
                let line = {
                    let mut mem = rt.memory();
                    s.lock().activate(&mut *mem, options, s.film)
                };
                if let Some(line) = line {
                    eprintln!("{line}");
                }
                Ok(Flow::Continue)
            },
        );
        let s = Arc::clone(&self.0);
        b.game().on_start("start Firefight on a kit map", move |o| {
            let mut mem = o.guest();
            let line = s.lock().start(&mut *mem, s.film);
            eprintln!("{line}");
        });
        let s = Arc::clone(&self.0);
        b.raw()
            .before(
                &engine::points::GAME_TICK,
                "apply Firefight's entries",
                move |c| {
                    if s.reentry.load(Ordering::SeqCst) {
                        return Ok(Flow::Continue);
                    }
                    let work = {
                        let rt = Arc::clone(c.ctx.runtime());
                        let mut mem = rt.memory();
                        s.lock().pre_tick(&mut *mem)
                    };
                    let Some(work) = work else {
                        return Ok(Flow::Continue);
                    };
                    let s = Arc::clone(&s);
                    c.answer_async(move |cx| -> EntryFuture {
                        Box::pin(runtime::run_work(cx, s, work, scratch))
                    })
                },
            )
            // Inside the co-op lobby setups and our skulls' calls: each
            // re-enters the tick, and this runs in the tick they run.
            .after_feature(crate::skulls_custom::FEATURE)
            .after_feature(crate::coop_probe::ID)
            .after_feature(crate::coop_probe::LIVE_ID)
            .after_feature(crate::coop_live::FEATURE);
        let s = Arc::clone(&self.0);
        b.frame().on_tick("step the Firefight director", move |o| {
            let mut mem = o.guest();
            sim_state::events(|events| s.lock().post_tick(&mut *mem, events));
        });
        let live = Arc::clone(&self.0);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            live.lock().running().map(|m| {
                format!(
                    "a Firefight match on {} keeps its director and host log outside guest memory",
                    m.map
                )
            })
        }));
        Ok(())
    }
}

/// Adds the feature. Says why when it is left out. `film`: this run plays
/// a film back. The match is the settings' defaults; [`install_with`] reads
/// Game Setup's choices.
pub fn install(registry: &mut hooks::Registry, film: bool) -> anyhow::Result<bool> {
    install_with(registry, film, None, None)
}

/// [`install`] with the source of Game Setup's choices
/// ([`super::options::source`]), read when each game starts, and where the
/// host's scores and result go ([`runtime::Report`]).
pub fn install_with(
    registry: &mut hooks::Registry,
    film: bool,
    options: Option<Box<super::options::Source>>,
    reports: Option<Box<runtime::ReportSink>>,
) -> anyhow::Result<bool> {
    let shared = Arc::new(Shared::new(film, Box::new(write_skulls)));
    if let Some(options) = options {
        shared.lock().set_options(options);
    }
    if let Some(reports) = reports {
        shared.lock().set_reports(reports);
    }
    let added = registry
        .add(&Firefight(Arc::clone(&shared)))
        .map_err(|e| anyhow::anyhow!("{FEATURE}: {e}"))?;
    if !added {
        return Ok(false);
    }
    let _ = SHARED.set(shared);
    Ok(true)
}

/// Why `firefight.*` has no runtime to answer from.
pub const OFF: &str = "the firefight feature is off (H2_FEATURES=firefight turns it on)";

/// The agent API's view of the runtime ([`agent::firefight::Source`]).
/// Without the feature it answers that it is off.
pub struct AgentSource(pub Option<Arc<Shared>>);

impl AgentSource {
    /// The run's runtime, once installed.
    pub fn installed() -> Self {
        Self(SHARED.get().cloned())
    }
}

impl agent::firefight::Source for AgentSource {
    fn state(&self) -> wire::State {
        match &self.0 {
            Some(s) => s.lock().state(),
            None => wire::State {
                refused: Some(OFF.into()),
                ..Default::default()
            },
        }
    }
    fn command(
        &self,
        params: wire::CommandParams,
    ) -> Result<wire::CommandReply, agent::firefight::Refused> {
        match &self.0 {
            Some(s) => s.lock().command(params),
            None => Err(agent::firefight::Refused::NotRunning(OFF.into())),
        }
    }
    fn kit(&self, m: &dyn GuestMemory) -> Result<wire::Kit, agent::firefight::Refused> {
        runtime::kit_reply(m).map_err(|e| {
            agent::firefight::Refused::Unavailable(format!("guest memory at {:#x} faulted", e.addr))
        })
    }
}
