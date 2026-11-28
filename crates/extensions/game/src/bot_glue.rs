//! Bots (docs/game/bots.md): the hooks around the `bots` crate.
//!
//! Every bot is a phantom peer ([`bots::phantom::global`],
//! docs/game/bots.md §5.1): before each `RENDER_FRAME`, while this console
//! hosts a pregame lobby (System Link or Xbox Live) or a multiplayer game,
//! pending phantoms are written into the session as joining peers, so the
//! lobby lists them and the game makes their players; each joined
//! phantom's pad becomes its action in a game. When this console hosts no
//! session any more they leave it and wait for the next
//! (`H2_PHANTOM_LOBBY=0`: they join only a running game and leave when it
//! ends).
//!
//! - A phantom's machine has no simulation view, so the game's "is this
//!   machine in the game" says no and its player never spawns: yes for a
//!   phantom with a peer in the session.
//! - A game starting from the session with phantoms planned makes this
//!   console the session's host when no other console is in it
//!   ([`bots::phantom::session::game_start`]), so the game starts as the
//!   distributed server and the phantoms can join.
//!
//! With no bots every hook returns at once.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::points;
use hooks::{BuildError, Builder, Flow, Manifest};

pub const ID: &str = "bots";

/// The feature over the process's phantom bots.
pub struct Bots {
    film: Option<Arc<crate::theater::Session>>,
    phantoms: bots::phantom::Phantoms,
    /// Where each map's data is written (`h2 bots nav`, `bots_nav_auto`).
    dir: std::path::PathBuf,
    map: Arc<Mutex<NavMap>>,
    frame: Arc<AtomicU64>,
    diagnostics: Arc<Mutex<LobbyDiagnostics>>,
}

#[derive(Default)]
struct LobbyDiagnostics {
    facts: Option<bots::lobby::Facts>,
    players: Option<u32>,
    fault: Option<u32>,
}

impl Bots {
    pub fn new(phantoms: bots::phantom::Phantoms, data_root: &std::path::Path) -> Self {
        Bots {
            film: None,
            phantoms,
            dir: bots::cache::dir(data_root),
            map: Arc::default(),
            frame: Arc::default(),
            diagnostics: Arc::default(),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl hooks::Feature for Bots {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Bots (Clankers)").doc("docs/game/bots.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        party_leave::install(b, &self.phantoms);

        // A phantom's machine has no simulation view, so the game's "is
        // this machine in the game" says no and its player never spawns
        // (docs/game/bots.md §5.1, "Phantoms in the game"): yes for a
        // phantom with a peer in the session.
        let phantoms = self.phantoms.clone();
        b.raw().before(
            &points::MACHINE_IN_GAME,
            "phantoms: their machines are in the game",
            move |c| {
                let at = c.cpu.eax;
                let rt = Arc::clone(c.ctx.runtime());
                let id = rt.memory().read::<6>(at);
                if !id.is_ok_and(|id| phantoms.stands_for(id)) {
                    return Ok(Flow::Continue);
                }
                c.cpu.eax = (c.cpu.eax & !0xff) | 1;
                Ok(Flow::Return(c.ret(0)?))
            },
        );

        let phantoms = self.phantoms.clone();
        let workspace = b.guest_alloc(retail_input::WORK_BYTES, runtime::guest::Permissions::RW)?;
        let input = Arc::new(std::sync::Mutex::new(None));
        // impl.sim.phantom-alias picks the path; the input stage and the
        // builder's body always run in Rust (docs/perf/sim-phantom-build.md).
        let path = retail_input::path_for(b.switches());
        b.raw().before(
            &points::SIMULATION_SLOT_ACTION,
            "phantoms: sample the pad for this simulation tick",
            move |c| {
                let slot = c.arg_u32("slot").unwrap_or(0);
                let rt = Arc::clone(c.ctx.runtime());
                if !phantoms.owns_action_slot(&*rt.memory(), slot) {
                    return Ok(Flow::Continue);
                }
                let phantoms = phantoms.clone();
                let input = Arc::clone(&input);
                let cpu = *c.cpu;
                c.answer_async(move |cx| {
                    Box::pin(async move {
                        phantoms
                            .slot_action(cx.clone(), slot, |sample| {
                                retail_input::gather(
                                    &input,
                                    &rt,
                                    workspace,
                                    path,
                                    cpu,
                                    sample,
                                    &cx,
                                )
                            })
                            .await
                    })
                })
            },
        );

        // A game starting from the session with phantoms planned: this
        // console must start it as the distributed server, or phantoms
        // never join (docs/game/bots.md §5.1, "Hosting the game").
        let phantoms = self.phantoms.clone();
        b.raw()
            .before(
                &points::SESSION_GAME_START,
                "phantoms: this console hosts the game",
                move |c| {
                    let planned = phantoms
                        .list()
                        .iter()
                        .any(|p| !matches!(p.state, bots::phantom::State::Failed(_)));
                    if !planned {
                        return Ok(Flow::Continue);
                    }
                    let session = c.cpu.edx;
                    let rt = Arc::clone(c.ctx.runtime());
                    let mut mem = rt.memory();
                    for line in bots::phantom::session::game_start(&mut *mem, session, planned) {
                        eprintln!("{line}");
                    }
                    Ok(Flow::Continue)
                },
            )
            // The co-op probe's peer-count fix (co-op campaigns only, as
            // `coop-probe` or as Live's armed `coop-live`) goes first; the
            // two never both act on one start.
            .after_feature(crate::coop_probe::ID)
            .after_feature(crate::coop_probe::LIVE_ID);

        // Each frame: the phantoms' pads and the lobby they join.
        let (phantoms, frame, map) = (
            self.phantoms.clone(),
            Arc::clone(&self.frame),
            Arc::clone(&self.map),
        );
        let reader = MapLoader::new(self.dir.clone());
        let diagnostics = Arc::clone(&self.diagnostics);
        b.raw().before(
            &points::RENDER_FRAME,
            "phantom pads",
            move |c| {
                let _cpu = timeline::host::Subsystem::Bots.span();
                let f = frame.fetch_add(1, Ordering::Relaxed);
                // A Live session made after the Clankers page was set gets
                // its roster (docs/game/bots.md stage 6c), and a
                // Clankermatch lobby its bots.
                let rt = Arc::clone(c.ctx.runtime());
                crate::clankers::frame(
                    f,
                    || {
                        let mem = rt.memory();
                        // In the lobby this console hosts, its session
                        // player's team (no player datum before a game, and a
                        // stale one after); else its player's.
                        let s = bots::phantom::session::lobby_host_team(&*mem)
                            .ok()
                            .flatten();
                        s.or_else(|| bots::phantom::session::host_team(&*mem).ok().flatten())
                    },
                    // The party a Clankermatch's bots fill around
                    // (docs/game/bots.md §8.4).
                    || {
                        let mem = rt.memory();
                        let facts = bots::lobby::membership(&*mem).ok()?;
                        bots::lobby::party_players(&*mem, &facts).ok().flatten()
                    },
                    // Co-op and matchmaking lobbies take none of the
                    // Clanker Options bots (docs/game/party-lobby.md).
                    || crate::lobby_tabs::header::retail_now(&*rt.memory()),
                    // A game this console serves (its simulation world
                    // the distributed server, world mode 4): a
                    // Clankermatch's lobby advertisement is gone by
                    // then, its match not.
                    || bots::phantom::session::serves_game(&*rt.memory()),
                    // The match session this console hosts: through
                    // arbitration and the handoff (state 7) before the
                    // game is served, a Clankermatch's bots stay.
                    || bots::phantom::session::hosts_match(&*rt.memory()),
                    // Why a Clankermatch lobby sits (docs/game/bots.md
                    // §8.4): the host update's checks, read the same way.
                    || match bots::lobby::match_wait::read(&*rt.memory()).ok()? {
                        Ok(None) => None,
                        Ok(Some(w)) => Some((w.reason(), w.line())),
                        Err(why) => Some((why.clone(), why)),
                    },
                );
                if phantoms.is_empty() {
                    return Ok(Flow::Continue);
                }
                let mut mem = rt.memory();
                load_map_data(&map, &reader, &mem);
                let result = phantoms.tick(&mut *mem, f);
                let lobby = bots::lobby::membership(&*mem).ok().map(|facts| {
                    let n = bots::lobby::lobby_players(&*mem, &facts).ok().flatten();
                    (facts, n)
                });
                drop(mem);
                let mut said = lock(&diagnostics);
                match result {
                    Ok(lines) => {
                        said.fault = None;
                        for line in lines {
                            eprintln!("{line}");
                        }
                    }
                    Err(e) if said.fault != Some(e.addr) => {
                        said.fault = Some(e.addr);
                        eprintln!("bots: phantoms: fault at {:#x}", e.addr);
                    }
                    Err(_) => {}
                }
                if let Some((facts, players)) = lobby {
                    if said.facts.as_ref() != Some(&facts) {
                        eprintln!("{}", bots::lobby::pregame_line(&facts));
                        said.facts = Some(facts);
                    }
                    if said.players != players {
                        if let Some(n) = players {
                            eprintln!("{}", bots::lobby::players_line(n));
                        }
                        said.players = players;
                    }
                }
                Ok(Flow::Continue)
            },
        );

        audio::install(b);
        crate::bot_accuracy::install(b, self.phantoms.clone());

        // The same weapon-fire point used by Live telemetry, as a sound
        // cue. It can only draw a bot's gaze.
        let phantoms = self.phantoms.clone();
        b.raw()
            .observe_before(&points::WEAPON_FIRE, "bots hear shots", move |o| {
                if !phantoms.is_empty()
                    && let Some(weapon) = o.arg_u32("weapon")
                {
                    bots::observe_shot(&*o.guest(), weapon);
                }
            })
            .optional();
        b.game()
            .on_start("forget bot sound cues", |_| bots::clear_shots());

        let phantoms = self.phantoms.clone();
        b.raw()
            .observe_before(&points::ITEM_SET_UNIT, "date a weapon pickup", move |o| {
                if !phantoms.is_empty()
                    && let (Some(item), Some(unit)) = (o.arg_u32("item"), o.arg_u32("unit"))
                    && unit != u32::MAX
                {
                    bots::guest::pickups::weapon(&*o.guest(), item, unit);
                }
            })
            .optional();
        let phantoms = self.phantoms.clone();
        b.raw()
            .observe_before(&points::POWERUP_TAKEN, "date a power-up pickup", move |o| {
                if !phantoms.is_empty() {
                    bots::guest::pickups::equipment(&*o.guest(), o.cpu.esp, o.cpu.ecx, o.cpu.edx);
                }
            })
            .optional();
        b.game().on_start("forget previous placement pickups", |_| {
            bots::guest::pickups::clear()
        });

        // Deaths, for the emotes (bots::emote): the kill function's
        // attacker and victim player datums, read as live telemetry reads
        // them; a betrayal is told by the teams (the third argument is not
        // a betrayal flag).
        let phantoms = self.phantoms.clone();
        b.raw()
            .observe_before(&points::PLAYER_KILLED, "bots hear a death", move |o| {
                if phantoms.is_empty() {
                    return;
                }
                let esp = o.cpu.esp;
                let guest = o.guest();
                let arg = |n: u32| guest.u32(esp.wrapping_add(4 * n));
                let (Ok(attacker), Ok(victim)) = (arg(1), arg(2)) else {
                    return;
                };
                if let Ok(Some(k)) = bots::emote::kill_from_guest(&*guest, attacker, victim) {
                    bots::emote::kills().push(k);
                }
            });

        if self.film.is_some() {
            film_state::install(b, &self.frame);
        } else {
            b.snapshot(hooks::SnapshotPolicy::Stateless);
        }
        Ok(())
    }
}

/// The loaded map's short name (the cache header's, `0x547898`).
const MAP_NAME: u32 = 0x0054_7898;

/// The loaded map's cache type (the cache header's `+0x140`, `0x547840`,
/// an `i16`; docs/live/live-gamestate.md "Cache identity").
const MAP_KIND: u32 = 0x0054_7840;

/// The source checksum distinguishes edits and sandbox variants with the
/// same short name (inline cache header, `docs/runtime/cache-slots.md`).
const MAP_CHECKSUM: u32 = 0x0054_79d0;

#[derive(Default, PartialEq, Eq)]
struct MapIdentity {
    name: [u8; 32],
    checksum: u32,
}

#[derive(Default)]
struct NavMap {
    seen: MapIdentity,
    editor: Option<vfs::HostFile>,
}

/// When the loaded map changes, queue the cached data read and, for a
/// multiplayer map, the background build. Bots steer toward known goals
/// until the mesh arrives ([`bots::autonav::offer`]).
/// Where a map's bot data is read: on a worker (play), or on the game
/// thread before the frame's pads (`H2_BOTS_MAP_INLINE=1`, which sim sets),
/// so the tick the bots first see the mesh does not depend on the host's
/// load.
enum MapLoader {
    Worker(Option<std::sync::mpsc::Sender<MapRead>>),
    Inline(std::path::PathBuf),
}

impl MapLoader {
    fn new(dir: std::path::PathBuf) -> Self {
        if std::env::var(MAP_INLINE_ENV).is_ok_and(|v| v == "1") {
            MapLoader::Inline(dir)
        } else {
            MapLoader::Worker(map_reader(dir))
        }
    }
}

/// Read map bot data on the game thread (`1`).
pub const MAP_INLINE_ENV: &str = "H2_BOTS_MAP_INLINE";

fn load_map_data(
    seen: &Mutex<NavMap>,
    reader: &MapLoader,
    mem: &runtime::Guest,
) {
    use bots::autonav::{self, MapUse};
    let Ok(mut raw) = mem.read::<32>(MAP_NAME) else {
        return;
    };
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    raw[end..].fill(0);
    let Ok(checksum) = mem.u32(MAP_CHECKSUM) else { return };
    let identity = MapIdentity { name: raw, checksum };
    let mut last = lock(seen);
    if last.seen == identity {
        return;
    }
    let _dispatch = timeline::setup::dispatch();
    last.seen = identity;
    let editor = last.editor.clone();
    drop(last);
    let name = String::from_utf8_lossy(&raw[..end]).into_owned();
    let (generation, retired) = autonav::map_loaded(&name);
    let kind = mem
        .u16(MAP_KIND)
        .ok()
        .map(|k| map::CacheType::from(u32::from(k)));
    let usage = autonav::map_use(&name, kind);
    if usage == MapUse::Never && retired.is_none() {
        return;
    }
    // A Forge session's generated nav for this map, while the document is
    // still the one it was made from (docs/game/forge-nav.md, "The bots").
    let staged = (usage == MapUse::Build)
        .then(|| crate::forge_glue::nav::staged(&name))
        .flatten();
    let auto = autonav::handle().filter(|_| usage == MapUse::Build && staged.is_none());
    if let Some(h) = &auto && editor.is_none() {
        h.want_active(&name, crate::map_slots::active_file(mem), generation);
        if retired.is_none() {
            return;
        }
    }
    let read = MapRead {
        name,
        usage,
        generation,
        retired,
        auto: auto.is_some(),
        editor,
        staged,
    };
    match reader {
        MapLoader::Worker(Some(tx)) => {
            let _ = tx.send(read);
        }
        MapLoader::Worker(None) => {}
        MapLoader::Inline(dir) => read.load(dir),
    }
}

struct MapRead {
    name: String,
    usage: bots::autonav::MapUse,
    generation: u64,
    retired: Option<Arc<bots::cache::MapData>>,
    auto: bool,
    editor: Option<vfs::HostFile>,
    /// A Forge session's generated nav for this map: the bots take it in
    /// place of the one built from the BSP alone.
    staged: Option<Arc<bots::cache::MapData>>,
}

/// The receiver sleeps while the map is stable. Rapid transitions retire
/// skipped maps on this worker and read only the newest queued map.
fn map_reader(dir: std::path::PathBuf) -> Option<std::sync::mpsc::Sender<MapRead>> {
    let (tx, rx) = std::sync::mpsc::channel::<MapRead>();
    let spawned = background::spawn("bots-map-data", move || {
        while let Ok(first) = rx.recv() {
            let request = rx.try_iter().last().unwrap_or(first);
            request.load(&dir);
        }
    });
    match spawned {
        Ok(_) => Some(tx),
        Err(e) => {
            eprintln!("bots: cannot start map data reader: {e}");
            None
        }
    }
}

impl MapRead {
    fn load(self, dir: &std::path::Path) {
        use bots::autonav::{self, MapUse};
        let MapRead { name, usage, generation, retired, auto, editor, staged } = self;
        // Free the retired mesh and its many polygon, edge and visibility
        // allocations away from the frame that observed the transition.
        drop(retired);
        if let Some(mesh) = staged {
            autonav::offer(&name, mesh, generation);
            return;
        }
        if let Some(file) = editor {
            // The ordinary per-map cache can match the retail name and even its
            // XOR checksum. Build from the pinned play copy, in memory only.
            if auto {
                let mut src = crate::bots_nav_auto::host_region(&name, file);
                match src.build_with(bots::vis::Threads::background()) {
                    Ok(bsps) => {
                        if let Some(data) = bsps.into_iter().next() {
                            autonav::offer(&name, Arc::new(data), generation);
                        }
                    }
                    Err(why) => eprintln!("bots: nav mesh for edited {name} unavailable: {why}"),
                }
            }
            return;
        }
        if usage == MapUse::Never || auto {
            return;
        }
        match autonav::load_cached(dir, &name, generation, None) {
            Ok(()) => {}
            Err(autonav::Status::Missing) if usage == MapUse::LoadOnly => {}
            Err(why) => eprintln!(
                "bots: {name}: nav mesh {} (bots walk straight until built with `h2 bots nav {name}`)",
                why.why()
            ),
        }
    }
}

/// Adds the bots feature over the process-wide bots to `registry`.
pub fn install(
    registry: &mut hooks::Registry,
    data_root: &std::path::Path,
) -> anyhow::Result<bool> {
    install_film(registry, data_root, None)
}

pub fn install_film(
    registry: &mut hooks::Registry,
    data_root: &std::path::Path,
    film: Option<Arc<crate::theater::Session>>,
) -> anyhow::Result<bool> {
    install_for_maps(registry, data_root, film, None)
}

pub fn install_for_maps(
    registry: &mut hooks::Registry,
    data_root: &std::path::Path,
    film: Option<Arc<crate::theater::Session>>,
    sources: Option<&crate::loaded_map::EditorSources>,
) -> anyhow::Result<bool> {
    let mut bots = Bots::new(bots::phantom::global().clone(), data_root);
    bots.film = film;
    let added = registry
        .add(&bots)
        .map_err(|e| anyhow::anyhow!("bots: {e}"))?;
    if added && let Some(sources) = sources {
        let map = Arc::downgrade(&bots.map);
        sources.on_change(move |file| {
            if let Some(map) = map.upgrade() {
                *lock(&map) = NavMap { seen: MapIdentity::default(), editor: file.cloned() };
            }
        });
    }
    if !added {
        eprintln!("bots: off: switched off (H2_FEATURES)");
    }
    Ok(added)
}

pub(crate) mod audio;
mod retail_input;
pub(crate) use retail_input::build_costs as phantom_build_costs;

mod film_state;
mod party_leave;
