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
//! With no bots every hook returns at once.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::points;
use hooks::{BuildError, Builder, Flow, Manifest};

pub const ID: &str = "bots";

/// The feature over the process's phantom bots.
pub struct Bots {
    film: Option<Arc<crate::theater::Session>>,
    phantoms: bots::phantom::Phantoms,
    /// Where each map's data is written (`h2 bots nav`, `bots_nav_auto`).
    dir: std::path::PathBuf,
    map: Arc<Mutex<String>>,
    frame: Arc<AtomicU64>,
    /// The last lobby facts line said.
    lobby_said: Arc<Mutex<String>>,
    /// The lobby's player count last said (`bots::lobby::players_line`).
    players_said: Arc<Mutex<Option<u32>>>,
}

impl Bots {
    pub fn new(phantoms: bots::phantom::Phantoms, data_root: &std::path::Path) -> Self {
        Bots {
            film: None,
            phantoms,
            dir: bots::cache::dir(data_root),
            map: Arc::default(),
            frame: Arc::default(),
            lobby_said: Arc::default(),
            players_said: Arc::default(),
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
        let film = self.film.clone();
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
                let film = film.clone();
                let cpu = *c.cpu;
                c.answer_async(move |cx| {
                    Box::pin(async move {
                        phantoms
                            .slot_action(cx.clone(), slot, |sample| {
                                retail_input::gather(
                                    &input,
                                    &rt,
                                    workspace,
                                    cpu,
                                    sample,
                                    &cx,
                                    film.as_deref().map(|s| s as &dyn film::tape::Tap),
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
        let (phantoms, frame, map, dir) = (
            self.phantoms.clone(),
            Arc::clone(&self.frame),
            Arc::clone(&self.map),
            self.dir.clone(),
        );
        let lobby_said = Arc::clone(&self.lobby_said);
        let players_said = Arc::clone(&self.players_said);
        let film = self.film.clone();
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
                if film
                    .as_ref()
                    .is_none_or(|s| !film::tape::Tap::replaying(s.as_ref()))
                {
                    crate::clankers::frame(f, || {
                        let mem = rt.memory();
                        // In the lobby this console hosts, its session
                        // player's team (no player datum before a game, and a
                        // stale one after); else its player's.
                        let s = bots::phantom::session::lobby_host_team(&*mem)
                            .ok()
                            .flatten();
                        s.or_else(|| bots::phantom::session::host_team(&*mem).ok().flatten())
                    });
                }
                if let Some(session) = &film
                    && let Err(error) = session.bot_boundary()
                {
                    film::tape::Tap::fault(session.as_ref(), error.to_string());
                }
                if phantoms.is_empty() {
                    return Ok(Flow::Continue);
                }
                let mut mem = rt.memory();
                load_map_data(&map, &dir, &mem);
                match phantoms.tick(&mut *mem, f) {
                    Ok(lines) => lines.iter().for_each(|l| eprintln!("{l}")),
                    Err(e) => eprintln!("bots: phantoms: fault at {:#x}", e.addr),
                }
                // What the lobby read gave, once per change, and its own
                // player count: what a Clankermatch's start counts.
                if let Ok(f) = bots::lobby::facts(&*mem) {
                    let line = bots::lobby::pregame_line(&f);
                    let mut said = lock(&lobby_said);
                    if *said != line {
                        eprintln!("{line}");
                        *said = line;
                    }
                    let n = bots::lobby::lobby_players(&*mem, &f).ok().flatten();
                    let mut said = lock(&players_said);
                    if *said != n {
                        if let Some(n) = n {
                            eprintln!("{}", bots::lobby::players_line(n));
                        }
                        *said = n;
                    }
                }
                Ok(Flow::Continue)
            },
        );

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

        if let Some(session) = &self.film {
            session.enable_bot_tape();
            film_state::install(b, &self.frame, session);
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

/// When the loaded map changes, loads its bot data on a background thread
/// and, for a multiplayer map, asks the nav builder
/// (`crate::bots_nav_auto`) to look at it next. Until then, and without a
/// file, the bots walk straight and see through walls; a mesh built while
/// the map runs goes in when it lands ([`bots::autonav::offer`]). The main
/// menu and the shared resource maps get nothing: no bot plays there
/// ([`bots::autonav::map_use`]).
fn load_map_data(seen: &Mutex<String>, dir: &std::path::Path, mem: &runtime::Guest) {
    use bots::autonav::{self, MapUse};
    use engine::memory::GuestMemory;
    let Ok(raw) = mem.cstr(MAP_NAME, 32) else {
        return;
    };
    let name = String::from_utf8_lossy(&raw).into_owned();
    let mut last = lock(seen);
    if *last == name {
        return;
    }
    last.clone_from(&name);
    autonav::map_loaded(&name);
    let kind = mem
        .u16(MAP_KIND)
        .ok()
        .map(|k| map::CacheType::from(u32::from(k)));
    let usage = autonav::map_use(&name, kind);
    if usage == MapUse::Never {
        return;
    }
    let auto = autonav::handle().filter(|_| usage == MapUse::Build);
    if let Some(h) = auto {
        h.want(&name);
    }
    let file = dir.join(bots::cache::file_name(&name, 0));
    let dir = dir.to_path_buf();
    let thread_name = name.clone();
    let spawned = background::spawn("bots-map-data", move || {
        let read = std::fs::read(&file);
        let data = read
            .as_ref()
            .ok()
            .and_then(|b| bots::cache::MapData::from_bytes(b));
        match (data, read.is_ok(), auto.is_some()) {
            (Some((d, stamp)), ..) => {
                // The visibility grid beside it, or a line saying why not
                // (every sight ray is then cast).
                let vis = bots::vis::load(&dir, &name, 0, stamp, &d.walls);
                let (polys, links, walls) =
                    (d.nav.polys.len(), d.nav.edges.len(), d.walls.tris.len());
                if autonav::offer(&name, d, false) {
                    eprintln!(
                        "bots: {name}: {polys} polygons, {links} links, {walls} wall triangles"
                    );
                    eprintln!("{vis}");
                }
            }
            (None, false, true) => eprintln!(
                "bots: no nav mesh for {name} yet; building it (bots walk straight until it lands)"
            ),
            (None, true, true) => eprintln!(
                "bots: {name}: the nav mesh file is from an older version; building it (bots walk straight until it lands)"
            ),
            // Not a multiplayer map: a file only when built by hand.
            (None, false, false) if usage == MapUse::LoadOnly => {}
            (None, false, false) => eprintln!(
                "bots: no nav mesh for {name}; run `h2 bots nav {name}` (bots walk straight until then)"
            ),
            (None, true, false) => eprintln!(
                "bots: {}: not a bot data file of this version; run `h2 bots nav {name}`",
                file.display()
            ),
        }
    });
    if let Err(e) = spawned {
        eprintln!("bots: {thread_name}: bot data not loaded: {e}");
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
    let mut bots = Bots::new(bots::phantom::global().clone(), data_root);
    bots.film = film;
    let added = registry
        .add(&bots)
        .map_err(|e| anyhow::anyhow!("bots: {e}"))?;
    if !added {
        eprintln!("bots: off: switched off (H2_FEATURES)");
    }
    Ok(added)
}

mod retail_input;

mod film_state;
