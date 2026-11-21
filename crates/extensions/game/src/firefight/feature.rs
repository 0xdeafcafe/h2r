//! The `firefight` feature (docs/game/firefight.md, "The host runtime"):
//! the runtime at the game's start and at each tick boundary, and the
//! agent API's view of it.
//!
//! Off unless `H2_FEATURES` names it. On, it starts the mode at each
//! multiplayer game's start where the map carries the kit, and otherwise
//! keeps the refusal and its reason for `firefight.state`. Its hooks:
//!
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
    let role = crate::skulls_mp::role(
        crate::coop_probe::read_session(&crate::layout_glue::ReadGm(&*m)).as_ref(),
    );
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

impl hooks::Feature for Firefight {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            FEATURE,
            "Firefight: the director and its host log on a kit map",
        )
        .off_by_default()
        .doc("docs/game/firefight.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::Flow;
        use std::sync::atomic::Ordering;
        // The two points of a line-of-sight test.
        let scratch = b.guest_alloc(0x20, ::runtime::guest::Permissions::RW)?;
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
/// a film back.
pub fn install(registry: &mut hooks::Registry, film: bool) -> anyhow::Result<bool> {
    let shared = Arc::new(Shared::new(film, Box::new(write_skulls)));
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
