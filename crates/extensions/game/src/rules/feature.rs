//! The `rules` feature (docs/game/rules-engine.md, "The host runtime"): the
//! runtime at the game's start, at each kill and tick boundary and at the
//! game's end, and the agent API's view of it.
//!
//! Off unless `H2_FEATURES` names it. Its hooks, all on game-thread points:
//!
//! - `GAME_ACTIVATE`, an observer after a successful start: start or refuse
//!   the script ([`Runtime::start`]); a Live host then puts the running
//!   blob's hash on its session (`Hle::set_session_rules`, none when no
//!   script runs) and names the guests whose build lacks `rules/1`.
//! - `GAME_TICK`, a `before` in the default class, inside Firefight's tick
//!   hook and the hooks Firefight sits inside: when calls are pending, or
//!   the authority is not yet known, answer with an async body that makes
//!   them and then runs the tick (re-entering its chain, which this hook
//!   then lets pass).
//! - `PLAYER_KILLED`, an observer before: `on kill` ([`Runtime::kill`]).
//!   The point is opaque, so nothing can answer it.
//! - `GAME_TICK`, an observer after: timers and `on tick`
//!   ([`Runtime::post_tick`]).
//! - `GAME_END`, an observer after: the record is removed.
//!
//! The superbounce mode's tick observer feeds it each bounce its medal
//! chain files ([`bounces`]), and asks whether the running script wants
//! the mode ([`running_asks_superbounce`]).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use agent_proto::rules as wire;
use engine::GuestMemory;
use engine::entry::EntryFuture;

use super::runtime::{self, Runtime};
use crate::layout_glue::{Gm, ReadGm};
use crate::variant_ext::{self, Part};

/// The feature id in the plan and `H2_FEATURES`.
pub const FEATURE: &str = "rules";
/// How long a Live host's start waits for its guests' presence.
pub const OLDER_WAIT: Duration = Duration::from_millis(500);

/// The runtime and the tick hook's re-entry guard.
pub struct Shared {
    runtime: Mutex<Runtime>,
    /// Set while the async body runs the tick it answered for.
    pub reentry: AtomicBool,
}

impl Shared {
    pub fn new(runtime: Runtime) -> Self {
        Self {
            runtime: Mutex::new(runtime),
            reentry: AtomicBool::new(false),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Runtime> {
        self.runtime.lock().unwrap_or_else(|e| e.into_inner())
    }
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// The feature. `hle`: the Live client, for the session's `rules` field;
/// `scripts`: the game-variant store and the Live fetch behind the runtime.
pub struct Rules {
    pub shared: Arc<Shared>,
    pub hle: Option<Arc<live_hle::Hle>>,
    pub scripts: Arc<crate::game_variants::GameScripts>,
}

/// The record's script prefix when its blob is neither built in nor on
/// this machine: the one a host's start must fetch first.
pub fn to_fetch(m: &dyn GuestMemory, scripts: &crate::game_variants::GameScripts) -> Option<[u8; 8]> {
    let block = super::guest::block(m).ok().flatten()?;
    let script = variants::ext::Ext::in_effect(&block).script;
    if script.mode != variants::ext::Mode::Rules {
        return None;
    }
    let p = script.prefix;
    let built = runtime::builtins().iter().any(|b| b.id.0[..8] == p);
    (!built && scripts.local(p).is_none()).then_some(p)
}

fn manifest() -> hooks::Manifest {
    hooks::Manifest::new(
        FEATURE,
        "Rules scripts: the VM of a script game on the authoritative machine",
    )
    .doc("docs/game/rules-engine.md")
    .off_by_default()
}

/// Whether the `rules` feature is on under `switches`.
pub fn enabled(switches: &hooks::Switches) -> bool {
    let m = manifest();
    switches.feature(m.id, m.default_on)
}

/// The kill function's attacker and victim player datums (stack arguments
/// 1 and 2, as live telemetry and the bots read them) as roster slots.
fn slot_of(datum: u32) -> Option<u8> {
    (datum != u32::MAX).then_some((datum & 0xffff) as u8)
}

/// The host's lines about guests whose build can't run a rules script's
/// display part: they play against the host's script with the carrier
/// engine's labels.
pub fn older_lines(lacking: &live_hle::Lacking) -> Vec<String> {
    match lacking {
        live_hle::Lacking::Known(tags) => tags
            .iter()
            .map(|t| format!("rules: {t} is on an older build and shows the carrier engine's labels"))
            .collect(),
        live_hle::Lacking::Unknown(why) => {
            vec![format!("rules: the guests' builds are not known: {why}")]
        }
    }
}

impl hooks::Feature for Rules {
    fn manifest(&self) -> hooks::Manifest {
        manifest()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use hooks::Flow;
        use std::sync::atomic::Ordering;
        let s = Arc::clone(&self.shared);
        let hle = self.hle.clone();
        let scripts = Arc::clone(&self.scripts);
        b.game().on_start("start the game's rules script", move |o| {
            // A host (or a game alone) whose record names a script that is
            // not on this machine fetches it from Live first, with no guest
            // memory held and a bounded wait; the start below then finds it
            // in the store or refuses with the fetch's reason.
            let fetch = {
                let mem = o.guest();
                let m: &dyn GuestMemory = &*mem;
                let read = ReadGm(m);
                let session = crate::coop_probe::read_session(&read);
                let class = session
                    .as_ref()
                    .and_then(|s| read.u32(s.addr.wrapping_add(variant_ext::SESSION_NETWORK_CLASS)));
                let part = variant_ext::part(session.as_ref().zip(class));
                matches!(part, Part::Alone | Part::LiveHost)
                    .then(|| to_fetch(m, &scripts))
                    .flatten()
            };
            if let Some(p) = fetch {
                scripts.prefetch(p, crate::game_variants::START_WAIT);
            }
            let mem = o.guest();
            let m: &dyn GuestMemory = &*mem;
            let read = ReadGm(m);
            let session = crate::coop_probe::read_session(&read);
            let class = session
                .as_ref()
                .and_then(|s| read.u32(s.addr.wrapping_add(variant_ext::SESSION_NETWORK_CLASS)));
            let part = variant_ext::part(session.as_ref().zip(class));
            // A matchmade game is a hopper's (with no deny list: a hopper
            // keeps none); Live holds a ranked session's script to the
            // ranked policy when the session names it.
            let context = match hle.as_ref().and_then(|h| h.matchmade()) {
                Some(_) => ::rules::policy::Context::Hopper { deny: &[] },
                None => ::rules::policy::Context::Custom,
            };
            let (line, hash) = {
                let mut rt = s.lock();
                rt.set_context(context);
                let line = rt.start(m, part == Part::SystemLink);
                (line, rt.hash())
            };
            let guests = session
                .as_ref()
                .map(|s| variant_ext::session_guests(&read, s))
                .unwrap_or_default();
            // The Live calls below wait on the network; no guest memory.
            drop(mem);
            eprintln!("{line}");
            if part == Part::LiveHost
                && let Some(h) = &hle
            {
                // Every start, script or not: a stale hash must not linger.
                h.set_session_rules(hash.clone());
                if hash.is_some() {
                    let lacking =
                        h.players_lacking(live_proto::capability::RULES, &guests, OLDER_WAIT);
                    for line in older_lines(&lacking) {
                        eprintln!("{line}");
                    }
                }
            }
        });
        let s = Arc::clone(&self.shared);
        b.raw()
            .before(
                &engine::points::GAME_TICK,
                "make the rules script's calls",
                move |c| {
                    if s.reentry.load(Ordering::SeqCst) {
                        return Ok(Flow::Continue);
                    }
                    let work = {
                        let rt = Arc::clone(c.ctx.runtime());
                        let mem = rt.memory();
                        let m: &dyn GuestMemory = &*mem;
                        s.lock().pre_tick(m)
                    };
                    let Some(work) = work else {
                        return Ok(Flow::Continue);
                    };
                    let s = Arc::clone(&s);
                    c.answer_async(move |cx| -> EntryFuture {
                        Box::pin(runtime::run_work(cx, s, work))
                    })
                },
            )
            // Inside Firefight's tick hook and the hooks it sits inside:
            // each re-enters the tick, and this runs in the tick they run.
            .after_feature(crate::firefight::feature::FEATURE)
            .after_feature(crate::skulls_custom::FEATURE)
            .after_feature(crate::coop_probe::ID)
            .after_feature(crate::coop_probe::LIVE_ID)
            .after_feature(crate::coop_live::FEATURE);
        let s = Arc::clone(&self.shared);
        b.raw()
            .observe_before(&engine::points::PLAYER_KILLED, "rules: on kill", move |o| {
                let esp = o.cpu.esp;
                let mem = o.guest();
                let m: &dyn GuestMemory = &*mem;
                let (Ok(attacker), Ok(victim)) =
                    (m.u32(esp.wrapping_add(4)), m.u32(esp.wrapping_add(8)))
                else {
                    return;
                };
                let Some(victim) = slot_of(victim) else {
                    return;
                };
                s.lock().kill(m, victim, slot_of(attacker));
            });
        let s = Arc::clone(&self.shared);
        b.frame().on_tick("run the rules script's tick", move |o| {
            let mem = o.guest();
            let m: &dyn GuestMemory = &*mem;
            s.lock().post_tick(m);
        });
        let s = Arc::clone(&self.shared);
        b.game().on_end("end the rules script", move |_| {
            s.lock().ended();
        });
        let live = Arc::clone(&self.shared);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            live.lock().snapshot_blocker()
        }));
        Ok(())
    }
}

/// Adds the feature. Says why when it is left out.
pub fn install(
    registry: &mut hooks::Registry,
    hle: Option<Arc<live_hle::Hle>>,
) -> anyhow::Result<bool> {
    let scripts = Arc::new(crate::game_variants::GameScripts::in_game(hle.clone()));
    let shared = Arc::new(Shared::new(Runtime::in_game(Arc::clone(&scripts))));
    let added = registry
        .add(&Rules {
            shared: Arc::clone(&shared),
            hle,
            scripts,
        })
        .map_err(|e| anyhow::anyhow!("{FEATURE}: {e}"))?;
    if !added {
        return Ok(false);
    }
    let _ = SHARED.set(shared);
    Ok(true)
}

/// One tick's superbounce awards (`superbounce_chain::Medals::step`), as
/// the running script's `on bounce` ([`Runtime::bounces`]). Nothing
/// without the feature.
pub fn bounces(m: &dyn GuestMemory, awards: &[crate::superbounce_chain::Award]) {
    if awards.is_empty() {
        return;
    }
    if let Some(s) = SHARED.get() {
        s.lock().bounces(m, awards);
    }
}

/// Whether the running script asks for the superbounce mode
/// ([`super::header::asks_superbounce`]); false without the feature or a
/// running script.
pub fn running_asks_superbounce() -> bool {
    SHARED.get().is_some_and(|s| {
        s.lock()
            .running()
            .is_some_and(|r| super::header::asks_superbounce(r.vm.ir()))
    })
}

/// Why `rules.state` has no runtime to answer from.
pub const OFF: &str = "the rules feature is off (H2_FEATURES=rules turns it on)";

/// The agent API's view of the runtime ([`agent::rules::Source`]). Without
/// the feature it answers that it is off.
pub struct AgentSource(pub Option<Arc<Shared>>);

impl AgentSource {
    /// The run's runtime, once installed.
    pub fn installed() -> Self {
        Self(SHARED.get().cloned())
    }
}

impl agent::rules::Source for AgentSource {
    fn state(&self) -> wire::State {
        match &self.0 {
            Some(s) => s.lock().state(),
            None => wire::State {
                refused: Some(OFF.into()),
                ..Default::default()
            },
        }
    }
}
