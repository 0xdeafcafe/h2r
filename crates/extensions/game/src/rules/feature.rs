//! The `rules` feature (docs/game/rules-engine.md, "The host runtime"): the
//! runtime at the game's start, at each kill and tick boundary and at the
//! game's end, and the agent API's view of it.
//!
//! On by default; `H2_FEATURES=-rules` turns it off. Its hooks, all on
//! game-thread points:
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
//! - `ENGINE_ROUND_RESET`, an observer after: `on round_start` for each
//!   round after the first ([`Runtime::round_start`]); the first round's
//!   reset runs inside `GAME_ACTIVATE`, and [`Runtime::start`] delivers it.
//! - `ENGINE_PLAYER_LEFT`, an observer before: `on quit` with the player
//!   in ESI, already off the roster ([`Runtime::quit`]).
//! - `GAME_TICK`, an observer after: timers and `on tick`
//!   ([`Runtime::post_tick`]).
//! - `GAME_END`, an observer after: the record is removed.
//! - `PLAYER_SPAWN_LOCATION`, observers before and after, and
//!   `SPAWN_LOCATION_FREE`, a `before` in the default class: a team's
//!   spawn rule keeps its spawns to the rule's starting locations
//!   ([`register_spawns`], `super::world::spawns`).
//! - `PLAYER_SPAWN`, a `before` and an `after` in the default class: a
//!   team's body is the spawning player's model byte for the length of
//!   the spawn ([`register_bodies`], `super::world::body`).
//! - `VARIANT_CLEAN_UP`, `VARIANT_ENCODE`, `VARIANT_DECODE`,
//!   `VARIANT_PROTOCOL` and `VARIANT_CATEGORY`, `before`s in the default
//!   class: a record of the script index is kept by the clean-up, crosses
//!   the wire with no type area, plays distributed and is filed as
//!   Slayer's ([`super::record`]).
//! - `GAME_ACTIVATE`, two `before`s in the last class: the script index's
//!   engine object goes in the engine table, and a record that names a
//!   script, or has none on a type with a built-in, moves onto the
//!   script index ([`super::record::move_onto_script_index`]); and `SPAWN_TYPE_MASK`, a
//!   `before` in the default class: the spawn gather's unchecked jump on
//!   the engine index is answered as Slayer's case for the script index
//!   ([`super::engine_object`]).
//!
//! A script with a `uses` line (docs/game/rules-variants.md, "A script
//! that keeps its carrier's rules") runs the built-in script of the type it
//! uses under the record ([`runtime::used_builtin`]): every hook above treats
//! it as that script, and the Live session names the built-in's hash. Its
//! `requires` turn their modes on through the record's script reference
//! (`super::header`), as for any script.
//!
//! The superbounce mode's tick observer feeds it what its medal chain saw
//! of each player's physics and each bounce the chain files ([`bounces`]).

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use agent_proto::rules as wire;
use guest_layout::GuestMemory;
use guest_layout::entry::EntryFuture;

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
    /// The last `rules.reload`, for `rules.state`.
    last_reload: Mutex<Option<wire::LastReload>>,
    /// The live starting locations of the spawn pick in progress, set as
    /// `PLAYER_SPAWN_LOCATION` starts and cleared as it returns.
    spawn_pick: Mutex<Option<super::world::spawns::Filter>>,
    /// The model byte each spawn in progress was given, to put back after
    /// it: one entry per `PLAYER_SPAWN` running, innermost last, so a
    /// spawn that re-enters itself (`coop_players`) puts back only its own.
    body_swap: Mutex<Vec<Option<super::world::body::Swap>>>,
    /// What a reload reaches beyond the runtime, set at install.
    links: OnceLock<Links>,
}

/// The Live client and the game-variant store, for `rules.reload`.
pub struct Links {
    pub hle: Option<Arc<live_hle::Hle>>,
    pub scripts: Arc<crate::game_variants::GameScripts>,
}

impl Shared {
    pub fn new(runtime: Runtime) -> Self {
        Self {
            runtime: Mutex::new(runtime),
            reentry: AtomicBool::new(false),
            last_reload: Mutex::new(None),
            spawn_pick: Mutex::new(None),
            body_swap: Mutex::new(Vec::new()),
            links: OnceLock::new(),
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Runtime> {
        self.runtime.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sets the links once; later calls keep the first.
    pub fn set_links(&self, links: Links) {
        let _ = self.links.set(links);
    }

    pub fn links(&self) -> Option<&Links> {
        self.links.get()
    }

    /// The spawn pick in progress (`super::world::spawns`).
    pub fn spawn_pick(&self) -> MutexGuard<'_, Option<super::world::spawns::Filter>> {
        self.spawn_pick.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The model bytes of the spawns in progress (`super::world::body`).
    pub fn body_swap(&self) -> MutexGuard<'_, Vec<Option<super::world::body::Swap>>> {
        self.body_swap.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_last_reload(&self, r: wire::LastReload) {
        *self.last_reload.lock().unwrap_or_else(|e| e.into_inner()) = Some(r);
    }

    pub fn last_reload(&self) -> Option<wire::LastReload> {
        self.last_reload.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// `rules.state`: the runtime's, with the last reload and the lobby's
    /// verdict ([`super::gate::annotate`]).
    pub fn state(&self) -> wire::State {
        let mut s = self.lock().state();
        s.last_reload = self.last_reload();
        super::gate::annotate(&mut s);
        s
    }
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

/// The feature. `hle`: the Live client, for the session's `rules` field;
/// `scripts`: the game-variant store and the Live fetch behind the runtime.
pub struct Rules {
    pub shared: Arc<Shared>,
    pub hle: Option<Arc<live_hle::Hle>>,
    pub scripts: Arc<crate::game_variants::GameScripts>,
    /// The VFS the loaded map's sound tags are read through
    /// ([`super::world::audio::activate`]).
    pub vfs: Option<Arc<std::sync::Mutex<vfs::Vfs>>>,
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
    let built = runtime::compiled_in(p).is_some();
    (!built && scripts.local(p).is_none()).then_some(p)
}

fn manifest() -> hooks::Manifest {
    hooks::Manifest::new(
        FEATURE,
        "Rules scripts: the VM of a script game on the authoritative machine",
    )
    .doc("docs/game/rules-engine.md")
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
        // The placement `set_loadout` builds each weapon in
        // (`super::world::loadout`).
        let scratch = b.guest_alloc(
            super::world::loadout::SCRATCH_SIZE,
            ::runtime::guest::Permissions::RW,
        )?;
        super::world::loadout::set_scratch(scratch);
        let sight_scratch = b.guest_alloc(
            super::world::physics::SIGHT_SCRATCH_SIZE,
            ::runtime::guest::Permissions::RW,
        )?;
        let s = Arc::clone(&self.shared);
        let hle = self.hle.clone();
        let scripts = Arc::clone(&self.scripts);
        let vfs = self.vfs.clone();
        b.game().on_start("start the game's rules script", move |o| {
            // The sounds the loaded map holds, by path: read from its file
            // before the start, which refuses a script naming one it lacks.
            // The map file is read with no guest memory held.
            if let Some(vfs) = &vfs {
                let loaded = super::world::audio::loaded(&o.guest());
                super::world::audio::activate(vfs, loaded);
            }
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
            // A Forge session's map is what is being built: the start gate
            // refuses its script without ending the game
            // (docs/game/rules-start-gate.md, "What a blocked start shows").
            let lobby = crate::lobby_start::state();
            let forge = lobby.lobby && lobby.mode == crate::lobby_start::Mode::Forge;
            let (line, hash) = {
                let mut rt = s.lock();
                rt.set_context(context);
                rt.set_forge_session(forge);
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
                &guest_layout::points::GAME_TICK,
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
                        Box::pin(runtime::run_work(cx, s, work, sight_scratch))
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
            .observe_before(&guest_layout::points::PLAYER_KILLED, "rules: on kill", move |o| {
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
        // A later round's start. The first round's reset runs inside
        // `GAME_ACTIVATE`, before the start observer above: `Runtime::start`
        // delivers that one. This observer runs before the script starts:
        // the last game's runtime is idle from its end, or replaced by the
        // start with whatever the handler queued.
        let s = Arc::clone(&self.shared);
        b.raw().observe_after(
            &guest_layout::points::ENGINE_ROUND_RESET,
            "rules: on round_start",
            move |o| {
                let mem = o.guest();
                let m: &dyn GuestMemory = &*mem;
                s.lock().round_start(m);
            },
        );
        let s = Arc::clone(&self.shared);
        b.raw().observe_before(
            &guest_layout::points::ENGINE_PLAYER_LEFT,
            "rules: on quit",
            move |o| {
                let Some(player) = slot_of(o.cpu.esi) else {
                    return;
                };
                let mem = o.guest();
                let m: &dyn GuestMemory = &*mem;
                s.lock().quit(m, player);
            },
        );
        let s = Arc::clone(&self.shared);
        b.frame().on_tick("run the rules script's tick", move |o| {
            let mem = o.guest();
            let m: &dyn GuestMemory = &*mem;
            sim_state::events(|events| s.lock().post_tick(m, events));
        });
        let s = Arc::clone(&self.shared);
        super::start_line::replace_carrier(
            b,
            move || s.lock().has_authority(),
            Some(crate::firefight::feature::FEATURE),
        );
        super::roles::register(b, &self.shared);
        super::record::register(b);
        super::engine_object::register(b)?;
        register_spawns(b, &self.shared);
        register_bodies(b, &self.shared);
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

/// The spawn rules (`spawn_areas`, `spawn_near`; `super::world::spawns`):
/// as `PLAYER_SPAWN_LOCATION` starts, the spawning player's (ECX) team
/// rule becomes the pick's filter, cleared as it returns; the free-spot
/// test `SPAWN_LOCATION_FREE`, which only that chooser calls, answers AL 0
/// ("something stands there") for a location the filter does not keep,
/// so the chooser keeps its best of the rest. No other feature answers
/// the test; `coop_players`' answer on the chooser only runs in a co-op
/// campaign.
fn register_spawns(b: &mut hooks::Builder, shared: &Arc<Shared>) {
    use hooks::Flow;
    let s = Arc::clone(shared);
    b.raw().observe_before(
        &guest_layout::points::PLAYER_SPAWN_LOCATION,
        "rules: the spawning team's starting locations",
        move |o| {
            let pick = {
                let mem = o.guest();
                let m: &dyn GuestMemory = &*mem;
                s.lock().spawn_filter(m, o.cpu.ecx)
            };
            *s.spawn_pick() = pick;
        },
    );
    let s = Arc::clone(shared);
    b.raw().observe_after(
        &guest_layout::points::PLAYER_SPAWN_LOCATION,
        "rules: the spawn pick ends",
        move |_| {
            *s.spawn_pick() = None;
        },
    );
    let s = Arc::clone(shared);
    b.raw().before(
        &guest_layout::points::SPAWN_LOCATION_FREE,
        "rules: a starting location off the team's rule is taken",
        move |c| {
            let blocked = s.spawn_pick().as_ref().is_some_and(|f| !f.passes(c.cpu.ecx));
            if !blocked {
                return Ok(Flow::Continue);
            }
            c.cpu.eax &= !0xff;
            c.ret(0).map(Flow::Return)
        },
    );
}

/// The teams' bodies (`set_body`; `super::world::body`): before
/// `PLAYER_SPAWN` (the player handle its stack argument), the spawning
/// player's team body becomes its model byte; after it, the player's own
/// is put back. Both continue the chain. `sim_glue`'s `before` runs first
/// (its own class), and `coop_players`' (campaign co-op, which runs no
/// script) sits outside this one.
fn register_bodies(b: &mut hooks::Builder, shared: &Arc<Shared>) {
    use hooks::Flow;
    let s = Arc::clone(shared);
    b.raw().before(
        &guest_layout::points::PLAYER_SPAWN,
        "rules: the spawning team's body",
        move |c| {
            // One entry per spawn, written or not, so the after pops its own.
            let mut swap = None;
            if let Some(handle) = c.arg_u32("player") {
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                let m: &mut dyn GuestMemory = &mut *mem;
                let body = s.lock().spawn_body(&*m, handle);
                if let Some((slot, body)) = body {
                    match super::world::body::put(m, slot, body) {
                        Ok(w) => swap = w,
                        Err(e) => eprintln!("rules: player {slot}'s body was not set: {e:?}"),
                    }
                }
            }
            s.body_swap().push(swap);
            Ok(Flow::Continue)
        },
    )
    // Inside `coop_players`' spawn, which re-enters the spawn it answers.
    .after_feature(crate::coop_players::FEATURE);
    let s = Arc::clone(shared);
    b.raw().after(
        &guest_layout::points::PLAYER_SPAWN,
        "rules: the player's own body again",
        move |c| {
            let Some(swap) = s.body_swap().pop().flatten() else {
                return Ok(());
            };
            let rt = Arc::clone(c.ctx.runtime());
            let mut mem = rt.memory();
            if let Err(e) = super::world::body::restore(&mut *mem, swap) {
                eprintln!("rules: a player's own body was not put back: {e:?}");
            }
            Ok(())
        },
    );
}

/// Adds the feature. Says why when it is left out. `film`: this run plays
/// a film back (Theater), so the start gate does not run: the film's host
/// passed it (docs/game/rules-start-gate.md, "Hoppers, System Link,
/// offline and Theater").
pub fn install(
    registry: &mut hooks::Registry,
    hle: Option<Arc<live_hle::Hle>>,
    film: bool,
    vfs: Option<Arc<std::sync::Mutex<vfs::Vfs>>>,
) -> anyhow::Result<bool> {
    let scripts = Arc::new(crate::game_variants::GameScripts::in_game(hle.clone()));
    let shared = Arc::new(Shared::new(
        Runtime::in_game(Arc::clone(&scripts)).with_film(film),
    ));
    shared.set_links(Links {
        hle: hle.clone(),
        scripts: Arc::clone(&scripts),
    });
    let added = registry
        .add(&Rules {
            shared: Arc::clone(&shared),
            hle,
            scripts,
            vfs,
        })
        .map_err(|e| anyhow::anyhow!("{FEATURE}: {e}"))?;
    if !added {
        return Ok(false);
    }
    // The objective HUDs a script feeds (`super::world::hud`), hidden
    // until it does.
    super::world::hud::register();
    // The scripted objective HUD (`crate::objective_hud`), hidden until a
    // script requires `hud.objective`.
    crate::objective_hud::register();
    let _ = SHARED.set(shared);
    Ok(true)
}

/// The host's role membership and traits for an actor-channel frame.
pub fn network_roles(m: &dyn GuestMemory) -> Option<crate::actor_net::roles::State> {
    let rt = SHARED.get()?.lock();
    let state = super::roles::network_state(m, &rt)?;
    // An empty Juggernaut mask clears the native role of a former holder
    // while its replicated engine globals catch up.
    (state.mask != 0 || rt.running()?.record_engine == super::roles::JUGGERNAUT).then_some(state)
}

/// One tick of the superbounce medal chain (`superbounce_chain::Medals::step`):
/// what it saw of the players' physics, as the running script's `on
/// touchdown`, `on launch` and `on grounded` ([`Runtime::physics`]), then
/// its awards, as `on bounce` and `on bounced` ([`Runtime::bounces`]).
/// Nothing without the feature.
pub fn bounces(
    m: &dyn GuestMemory,
    bodies: &[crate::superbounce_chain::Body],
    physics: &[(u8, crate::superbounce_chain::Physics)],
    awards: &[crate::superbounce_chain::Award],
) {
    if let Some(s) = SHARED.get() {
        let mut rt = s.lock();
        // Set the biped reads from every chain step, even without events.
        rt.bodies(bodies);
        if awards.is_empty() && physics.is_empty() {
            return;
        }
        rt.physics(m, physics);
        rt.bounces(m, awards);
    }
}

/// What the running script calls a point (its header's `points`), for the
/// base HUD ([`super::base_hud`]); none while no script runs here.
pub fn points_label() -> Option<String> {
    let rt = SHARED.get()?.lock();
    rt.running().map(|r| r.vm.ir().header.points.clone())
}

/// The problems the host's start gate blocked the last game on, while the
/// block holds (until the next game starts); none without the feature.
/// The lobby's verdict (`rules::gate`) reads it, so the status line names
/// them after the game ends.
pub fn blocked() -> Option<Vec<String>> {
    SHARED.get()?.lock().blocked().map(<[String]>::to_vec)
}

/// Whether the feature is installed in this run: the lobby's start gate
/// (`rules::gate`) checks nothing without it, since no script runs.
pub fn installed() -> bool {
    SHARED.get().is_some()
}

/// The blob on this machine whose hash starts with `prefix`: compiled in,
/// else the game-variant store's. The lobby's start gate checks only these;
/// a script Live would serve is the host's to check at the start.
pub fn local_blob(prefix: [u8; 8]) -> Option<Vec<u8>> {
    if let Some(b) = runtime::compiled_in(prefix) {
        return Some(b.blob.clone());
    }
    SHARED.get()?.links()?.scripts.local(prefix)
}

/// Why `rules.state` has no runtime to answer from.
pub const OFF: &str = "the rules feature is off (H2_FEATURES=-rules turned it off)";

/// The agent API's view of the runtime ([`agent::rules::Source`]). Without
/// the feature it answers that it is off.
pub struct AgentSource {
    pub shared: Option<Arc<Shared>>,
    /// This run plays a film back (Theater): no reload.
    pub playback: bool,
}

impl AgentSource {
    /// The run's runtime, once installed.
    pub fn installed() -> Self {
        Self {
            shared: SHARED.get().cloned(),
            playback: false,
        }
    }

    /// The same source for a run that plays a film back, or not.
    pub fn playback(mut self, playback: bool) -> Self {
        self.playback = playback;
        self
    }
}

impl agent::rules::Source for AgentSource {
    fn state(&self) -> wire::State {
        match &self.shared {
            Some(s) => s.state(),
            None => wire::State {
                refused: Some(OFF.into()),
                ..Default::default()
            },
        }
    }

    fn zones(&self) -> wire::Zones {
        self.shared.as_ref().map(|s| s.lock().zones()).unwrap_or_default()
    }

    fn reload(
        &self,
        m: &mut dyn GuestMemory,
        params: wire::ReloadParams,
        restart: Option<&'static str>,
    ) -> Result<agent::rules::Reload, agent_proto::RpcError> {
        let Some(s) = &self.shared else {
            return Err(agent_proto::RpcError::new(agent_proto::code::UNAVAILABLE, OFF));
        };
        super::reload::reload(s, m, params, restart, self.playback)
    }

    fn restart_outcome(&self, outcome: Result<&wire::ReloadResult, &str>) {
        if let Some(s) = &self.shared {
            super::reload::restart_outcome(s, outcome);
        }
    }
}
