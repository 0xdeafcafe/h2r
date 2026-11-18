//! Local bots (docs/game/bots.md, sections 4 and 10): the hooks around the
//! `bots` crate.
//!
//! - The process-wide [`bots::local::global`] bots are an input backend,
//!   added after the host's own pads, so they can be added at any time
//!   (`H2_BOTS=<n>` adds `n` brains at boot; the agent API can add more).
//! - On the players screen, a bot's A reaches the screen's handler for a
//!   controller with no profile (`PLAYERS_SCREEN_UNJOINED`). For a bot the
//!   hook answers in its place: the game's own sign-in with a profile made
//!   from the human's ([`bots::guest::sign_in`]).
//! - Before each `RENDER_FRAME` (the game thread) each bot's source gives
//!   its pad for the frame ([`bots::local::Local::frame`]).
//! - Around `RENDER_FRAME` and `SOUND_UPDATE`, the bots' local users are
//!   taken out of the local player map ([`bots::guest::hide`]), so the
//!   views, HUDs and listeners are the humans' only. With the pipelined
//!   render the snapshot is taken inside `RENDER_FRAME`, so it has them
//!   hidden too; both points run on the game thread.
//!
//! - Phantom peers ([`bots::phantom::global`], docs/game/bots.md §5.1):
//!   before each `RENDER_FRAME`, once a multiplayer game this console hosts
//!   runs, pending phantoms are written into the session and the game makes
//!   their players; each joined phantom's pad becomes its action. When the
//!   game ends they leave the session and wait for the next.
//!
//! With no bots every hook returns at once. The bots stay signed in until
//! the game exits; going back to the players screen finds them joined.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bots::guest::{self, Hidden};
use bots::local::{self, Local};
use engine::points;
use feature::{BuildError, Builder, Flow, Manifest};

pub const ID: &str = "bots";

/// Adds the `H2_BOTS` brains to the process-wide bots, on ports 1 up.
pub fn add_from_env() {
    let v = std::env::var(bots::ENV).ok();
    match bots::from_env_value(v.as_deref()) {
        Ok(0) => {}
        Ok(n) => {
            for port in 1..=n as u8 {
                let skill = std::env::var(bots::SKILL_ENV)
                    .ok()
                    .and_then(|v| bots::mind::Skill::parse(&v))
                    .unwrap_or_default();
                let brain = bots::Brain::new(skill, 0x5eed ^ u64::from(port));
                match local::add(port, Box::new(brain)) {
                    Ok(h) => eprintln!("bots: Clanker {port} (bot {}) wants port {port}", h.0),
                    Err(e) => eprintln!("bots: {e}"),
                }
            }
        }
        Err(e) => eprintln!("bots: none: {e}"),
    }
}

/// The feature over a set of local bots.
pub struct Bots {
    local: Local,
    phantoms: bots::phantom::Phantoms,
    /// Where each map's data is written (`h2 bots nav`, `bots_nav_auto`).
    dir: std::path::PathBuf,
    map: Arc<Mutex<String>>,
    frame: Arc<AtomicU64>,
    render: Arc<Mutex<Option<Hidden>>>,
    sound: Arc<Mutex<Option<Hidden>>>,
}

impl Bots {
    pub fn new(
        local: Local,
        phantoms: bots::phantom::Phantoms,
        data_root: &std::path::Path,
    ) -> Self {
        Bots {
            local,
            phantoms,
            dir: data_root.join("bots"),
            map: Arc::default(),
            frame: Arc::default(),
            render: Arc::default(),
            sound: Arc::default(),
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn fault(addr: u32) -> runtime::GuestFault {
    runtime::GuestFault {
        addr,
        len: 4,
        access: runtime::guest::Access::Write,
        reason: runtime::guest::FaultReason::Unmapped,
    }
}

impl feature::Feature for Bots {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Local bots (Clankers)").doc("docs/game/bots.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let buffer = b.guest_alloc(0x1000, runtime::guest::Permissions::RW)?;

        // A bot joining on the players screen.
        let local = self.local.clone();
        b.raw()
            .before(&points::PLAYERS_SCREEN_UNJOINED, "a bot joins", move |c| {
                let bots = local.port_mask();
                if bots == 0 {
                    return Ok(Flow::Continue);
                }
                let event = c.cpu.esi;
                let rt = Arc::clone(c.ctx.runtime());
                let port = guest::bot_join(&*rt.memory(), event, bots).ok().flatten();
                let Some(port) = port else {
                    return Ok(Flow::Continue);
                };
                let name = local.name_on(port).unwrap_or_default();
                eprintln!("bots: signing {name} in on port {port}");
                c.answer_async(move |cx| Box::pin(guest::sign_in(cx, port, bots, name, buffer)))
            });

        // Each frame: the pads, then the bots' views out.
        let (local, phantoms, frame, render, map, dir) = (
            self.local.clone(),
            self.phantoms.clone(),
            Arc::clone(&self.frame),
            Arc::clone(&self.render),
            Arc::clone(&self.map),
            self.dir.clone(),
        );
        b.raw().before(
            &points::RENDER_FRAME,
            "bot pads; hide bot views",
            move |c| {
                let bots = local.port_mask();
                let any_phantoms = !phantoms.is_empty();
                if bots == 0 && !any_phantoms {
                    return Ok(Flow::Continue);
                }
                let f = frame.fetch_add(1, Ordering::Relaxed);
                // A Live session made after the Clankers page was set gets
                // its roster (docs/game/bots.md stage 6c).
                crate::clankers::frame(f);
                let rt = Arc::clone(c.ctx.runtime());
                let mut mem = rt.memory();
                load_map_data(&map, &dir, &mem);
                if any_phantoms {
                    match phantoms.tick(&mut *mem, f) {
                        Ok(lines) => lines.iter().for_each(|l| eprintln!("{l}")),
                        Err(e) => eprintln!("bots: phantoms: fault at {:#x}", e.addr),
                    }
                }
                if bots == 0 {
                    return Ok(Flow::Continue);
                }
                let on_players_screen = crate::ui_watch::read(&*mem)
                    .top
                    .is_some_and(|t| t.id == crate::ui_watch::JOIN_SCREEN);
                let g: &runtime::Guest = &mem;
                local.frame(f, g, |port| {
                    on_players_screen && guest::signed_in(g, u32::from(port)) == Ok(false)
                });
                *lock(&render) = guest::hide(&mut *mem, bots).ok().flatten();
                Ok(Flow::Continue)
            },
        );
        let render = Arc::clone(&self.render);
        b.raw()
            .after(&points::RENDER_FRAME, "bot views back", move |c| {
                if let Some(h) = lock(&render).take() {
                    let rt = Arc::clone(c.ctx.runtime());
                    guest::restore(&mut *rt.memory(), &h).map_err(|e| fault(e.addr))?;
                }
                Ok(())
            });

        // Deaths, for the emotes (bots::emote): the kill function's
        // attacker and victim player datums and betrayal flag, read as
        // live telemetry reads them.
        let (local, phantoms) = (self.local.clone(), self.phantoms.clone());
        b.raw()
            .observe_before(&points::PLAYER_KILLED, "bots hear a death", move |o| {
                if local.is_empty() && phantoms.is_empty() {
                    return;
                }
                let esp = o.cpu.esp;
                let guest = o.guest();
                let arg = |n: u32| guest.u32(esp.wrapping_add(4 * n));
                let (Ok(attacker), Ok(victim), Ok(betrayal)) = (arg(1), arg(2), arg(3)) else {
                    return;
                };
                if let Ok(Some(k)) =
                    bots::emote::kill_from_guest(&*guest, attacker, victim, betrayal != 0)
                {
                    bots::emote::kills().push(k);
                }
            });

        // The listeners: the humans' ears only.
        let (local, sound) = (self.local.clone(), Arc::clone(&self.sound));
        b.raw()
            .before(&points::SOUND_UPDATE, "hide bot listeners", move |c| {
                let bots = local.port_mask();
                if bots == 0 {
                    return Ok(Flow::Continue);
                }
                let rt = Arc::clone(c.ctx.runtime());
                *lock(&sound) = guest::hide(&mut *rt.memory(), bots).ok().flatten();
                Ok(Flow::Continue)
            });
        let sound = Arc::clone(&self.sound);
        b.raw()
            .after(&points::SOUND_UPDATE, "bot listeners back", move |c| {
                if let Some(h) = lock(&sound).take() {
                    let rt = Arc::clone(c.ctx.runtime());
                    guest::restore(&mut *rt.memory(), &h).map_err(|e| fault(e.addr))?;
                }
                Ok(())
            });

        // The hidden users are put back before either point returns; the
        // pads are host input, which a snapshot does not carry.
        b.snapshot(feature::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// The loaded map's short name (the cache header's, `0x547898`).
const MAP_NAME: u32 = 0x0054_7898;

/// When the loaded map changes, loads its bot data on a thread of its own
/// and asks the nav builder (`crate::bots_nav_auto`) to look at the map
/// next. Until then, and without a file, the bots walk straight and see
/// through walls; a mesh built while the map runs goes in when it lands
/// ([`bots::autonav::offer`]).
fn load_map_data(seen: &Mutex<String>, dir: &std::path::Path, mem: &runtime::Guest) {
    use bots::autonav;
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
    if name.is_empty() {
        return;
    }
    let auto = autonav::handle();
    if let Some(h) = auto {
        h.want(&name);
    }
    let file = dir.join(bots::cache::file_name(&name, 0));
    std::thread::spawn(move || {
        let read = std::fs::read(&file);
        let data = read.as_ref().ok().and_then(|b| bots::cache::MapData::from_bytes(b));
        match (data, read.is_ok(), auto.is_some()) {
            (Some((d, _)), ..) => {
                let (polys, links, walls) = (d.nav.polys.len(), d.nav.edges.len(), d.walls.tris.len());
                if autonav::offer(&name, d, false) {
                    eprintln!("bots: {name}: {polys} polygons, {links} links, {walls} wall triangles");
                }
            }
            (None, false, true) => eprintln!(
                "bots: no nav mesh for {name} yet; building it (bots walk straight until it lands)"
            ),
            (None, true, true) => eprintln!(
                "bots: {name}: the nav mesh file is from an older version; building it (bots walk straight until it lands)"
            ),
            (None, false, false) => eprintln!(
                "bots: no nav mesh for {name}; run `h2 bots nav {name}` (bots walk straight until then)"
            ),
            (None, true, false) => eprintln!(
                "bots: {}: not a bot data file of this version; run `h2 bots nav {name}`",
                file.display()
            ),
        }
    });
}

/// Adds the bots feature over the process-wide bots to `registry`.
pub fn install(
    registry: &mut feature::Registry,
    data_root: &std::path::Path,
) -> anyhow::Result<bool> {
    let added = registry
        .add(&Bots::new(
            local::global().clone(),
            bots::phantom::global().clone(),
            data_root,
        ))
        .map_err(|e| anyhow::anyhow!("bots: {e}"))?;
    if !added {
        eprintln!("bots: off: switched off (H2_FEATURES)");
    }
    Ok(added)
}
