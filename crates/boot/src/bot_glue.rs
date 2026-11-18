//! Local bots (docs/game/bots.md, sections 4 and 10): the hooks around the
//! `bots` crate.
//!
//! - The process-wide [`bots::local::global`] bots are an input backend,
//!   added after the host's own pads, so they can be added at any time
//!   (`H2_BOTS=<n>` adds `n` brains at boot; the agent API can add more).
//! - On the players screen the glue signs each bot in itself, with the
//!   game's own sign-in and a profile made from the human's
//!   ([`bots::guest::sign_in_then`]), before `SOUND_UPDATE`. A bot never
//!   presses a button to join (see "The join gate"). A bot's A that still
//!   reaches the screen's handler for a controller with no profile
//!   (`PLAYERS_SCREEN_UNJOINED`, a sim script's) is answered the same way
//!   ([`bots::guest::sign_in`]).
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
//! **The join gate.** A bot is signed in only while the players screen
//! takes player one's input, nothing sits on windows 0 to 4 (a dialog, or
//! the profile chooser: [`crate::ui_watch::dialog`]), its own slot has no
//! profile, a human has one to copy and no game runs ([`to_sign_in`]).
//! Each port's gate is logged when it changes, and each sign-in.
//!
//! Bots used to tap A through that gate, and every tap the game took
//! anywhere but the players screen's own handler opened a profile chooser
//! for the bot's controller: the pregame lobby's handler (`0x250b1e`) sends
//! A or START from a controller with no profile to the chooser
//! (`0x250cc3`), and a tap that reached a chooser picked the first profile,
//! the human's (the user's runs: the human's player four times on the
//! players screen; "pick a profile" popping up in an Xbox Live lobby).
//!
//! **Leaving.** A bot taken off ([`bots::local::Local::remove`]) keeps an
//! idle pad. Before `SOUND_UPDATE`, once no game runs, the hook answers
//! for the frame: it signs the leaving bots out of their slots with the
//! game's `PROFILE_SIGN_OUT`, then runs the frame itself; a leaving bot
//! whose slot is empty is unplugged ([`bots::local::leave_step`]). A game
//! never sees a bot's pad go.
//!
//! With no bots every hook returns at once. The bots stay signed in until
//! they are taken off; going back to the players screen finds them joined.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// Whether the bot on a port may tap A to join: the players screen takes
/// player one's input (`top`, [`crate::ui_watch::read`]), no dialog or
/// profile chooser is open over it (`dialog`, [`crate::ui_watch::dialog`]),
/// and the port's slot has no profile yet (`signed_in`: `None` when the
/// slot could not be read, which is a no).
pub fn join_gate(
    top: Option<crate::ui_watch::Top>,
    dialog: Option<crate::ui_watch::Dialog>,
    signed_in: Option<bool>,
) -> bool {
    top.is_some_and(|t| t.id == crate::ui_watch::JOIN_SCREEN)
        && dialog.is_none()
        && signed_in == Some(false)
}

/// The log line for one port's gate: what it read and what it decided,
/// with the slot's flags (`flags`, slot `+0`: `0x10` a profile, `0x20`
/// signed in to Xbox Live), for what the lobby makes of the bot.
pub fn gate_line(
    port: u8,
    top: Option<crate::ui_watch::Top>,
    dialog: Option<crate::ui_watch::Dialog>,
    signed_in: Option<bool>,
    flags: Option<u32>,
) -> String {
    let verdict = if join_gate(top, dialog, signed_in) {
        "signs in"
    } else {
        "waits"
    };
    let top = top.map_or_else(|| "none".to_owned(), |t| format!("0x{:02x}", t.id));
    let dialog = dialog.map_or_else(
        || "none".to_owned(),
        |d| {
            let id =
                d.id.map_or_else(|| "?".to_owned(), |i| format!("0x{i:02x}"));
            format!("{id} (window {}, slot {})", d.window, d.slot)
        },
    );
    let signed = signed_in.map_or_else(|| "?".to_owned(), |b| b.to_string());
    let flags = flags.map_or_else(|| "?".to_owned(), |f| format!("{f:#x}"));
    format!(
        "bots: join gate port {port}: top {top}, dialog {dialog}, signed_in {signed} (slot {flags}): {verdict}"
    )
}

/// Frames between sign-in tries (a try that did not take, as with no
/// human profile yet, is not made every frame).
pub const SIGN_IN_EVERY: u64 = 30;

/// The ports whose bot the glue signs in now: each joining bot's port
/// (`joining`, a mask) whose [`join_gate`] is open, while no game runs
/// (`in_game`) and a human has a profile to copy (`human`).
pub fn to_sign_in(
    top: Option<crate::ui_watch::Top>,
    dialog: Option<crate::ui_watch::Dialog>,
    joining: u32,
    signed_in: impl Fn(u8) -> Option<bool>,
    human: bool,
    in_game: bool,
) -> Vec<u8> {
    if in_game || !human {
        return Vec::new();
    }
    (0..4u8)
        .filter(|&p| joining & 1 << p != 0 && join_gate(top, dialog, signed_in(p)))
        .collect()
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
    /// Each port's last join-gate line, so it is logged on a change.
    gates: Arc<Mutex<[String; 4]>>,
    /// A sign-in or sign-out is running the frame itself: its own
    /// `SOUND_UPDATE` call must not start another.
    signing_out: Arc<AtomicBool>,
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
            gates: Arc::default(),
            signing_out: Arc::default(),
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
                let bots = local.joining_mask();
                if bots == 0 {
                    return Ok(Flow::Continue);
                }
                let event = c.cpu.esi;
                let rt = Arc::clone(c.ctx.runtime());
                let (port, seen) = {
                    let mem = rt.memory();
                    let seen = (
                        mem.u32(event + guest::EVENT_CONTROLLER).unwrap_or(u32::MAX),
                        mem.u32(event + guest::EVENT_BUTTON).unwrap_or(u32::MAX),
                    );
                    (guest::bot_join(&*mem, event, bots).ok().flatten(), seen)
                };
                let Some(port) = port else {
                    if seen.0 < 4 && bots & 1 << seen.0 != 0 {
                        eprintln!(
                            "bots: players screen: bot port {} button {:#x}, not a join",
                            seen.0, seen.1
                        );
                    }
                    return Ok(Flow::Continue);
                };
                let name = local.name_on(port).unwrap_or_default();
                eprintln!("bots: signing {name} in on port {port}");
                c.answer_async(move |cx| Box::pin(guest::sign_in(cx, port, bots, name, buffer)))
            });

        // Each frame: the pads, then the bots' views out.
        let (local, phantoms, frame, render, map, dir, gates) = (
            self.local.clone(),
            self.phantoms.clone(),
            Arc::clone(&self.frame),
            Arc::clone(&self.render),
            Arc::clone(&self.map),
            self.dir.clone(),
            Arc::clone(&self.gates),
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
                // The join gate, per port, logged when it changes; a bot it
                // lets through presses nothing (the sign-in is before
                // SOUND_UPDATE).
                let top = crate::ui_watch::read(&*mem).top;
                let dialog = crate::ui_watch::dialog(&*mem);
                let joining = local.joining_mask();
                let g: &runtime::Guest = &mem;
                let mut open = [false; 4];
                {
                    let mut said = lock(&gates);
                    for port in 0..4u8 {
                        if joining & 1 << port == 0 {
                            said[usize::from(port)].clear();
                            continue;
                        }
                        let signed_in = guest::signed_in(g, u32::from(port)).ok();
                        open[usize::from(port)] = join_gate(top, dialog, signed_in);
                        let flags = g.u32(guest::slot(u32::from(port))).ok();
                        let line = gate_line(port, top, dialog, signed_in, flags);
                        if said[usize::from(port)] != line {
                            eprintln!("{line}");
                            said[usize::from(port)] = line;
                        }
                    }
                }
                local.frame(f, g, |port| {
                    open.get(usize::from(port)).copied().unwrap_or(false)
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
        // And, out of a game, the leaving bots signed out, then unplugged.
        // And on the players screen, the joining bots signed in.
        let (local, sound, busy, frame) = (
            self.local.clone(),
            Arc::clone(&self.sound),
            Arc::clone(&self.signing_out),
            Arc::clone(&self.frame),
        );
        b.raw().before(
            &points::SOUND_UPDATE,
            "sign bots in and leaving bots out; hide bot listeners",
            move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                let joining = local.joining_mask();
                if joining != 0
                    && !busy.load(Ordering::Relaxed)
                    && frame.load(Ordering::Relaxed) % SIGN_IN_EVERY == 0
                {
                    let bots = local.port_mask();
                    let (ports, screen) = {
                        let mem = rt.memory();
                        let top = crate::ui_watch::read(&*mem).top;
                        let human = guest::human_port(&*mem, bots).ok().flatten().is_some();
                        let ports = to_sign_in(
                            top,
                            crate::ui_watch::dialog(&*mem),
                            joining,
                            |p| guest::signed_in(&*mem, u32::from(p)).ok(),
                            human,
                            guest::game_tick(&*mem).is_some(),
                        );
                        let screen = top
                            .filter(|t| t.id == crate::ui_watch::JOIN_SCREEN)
                            .map(|t| t.screen);
                        (ports, screen)
                    };
                    if !ports.is_empty() {
                        let joins: Vec<(u32, String)> = ports
                            .iter()
                            .map(|&p| {
                                let name = local.name_on(u32::from(p)).unwrap_or_default();
                                eprintln!("bots: signing {name} in on port {p}");
                                (u32::from(p), name)
                            })
                            .collect();
                        busy.store(true, Ordering::Relaxed);
                        let busy = Arc::clone(&busy);
                        let then = points::SOUND_UPDATE.base;
                        return c.answer_async(move |cx| -> engine::entry::EntryFuture {
                            Box::pin(async move {
                                let r = guest::sign_in_then(cx, joins, bots, buffer, screen, then)
                                    .await;
                                busy.store(false, Ordering::Relaxed);
                                r
                            })
                        });
                    }
                }
                if !busy.load(Ordering::Relaxed) {
                    let leaving = local.leaving();
                    if !leaving.is_empty() {
                        let step = {
                            let mem = rt.memory();
                            let in_game = guest::game_tick(&*mem).is_some();
                            local::leave_step(in_game, &leaving, |p| {
                                guest::signed_in(&*mem, u32::from(p)).unwrap_or(false)
                            })
                        };
                        for h in step.unplug {
                            if local.unplug(h) {
                                eprintln!("bots: bot {} left: its pad is unplugged", h.0);
                            }
                        }
                        if !step.sign_out.is_empty() {
                            eprintln!("bots: signing the bots on ports {:?} out", step.sign_out);
                            busy.store(true, Ordering::Relaxed);
                            let (busy, ports) = (Arc::clone(&busy), step.sign_out);
                            let then = points::SOUND_UPDATE.base;
                            return c.answer_async(move |cx| -> engine::entry::EntryFuture {
                                Box::pin(async move {
                                    let r = guest::sign_out_then(cx, ports, then).await;
                                    busy.store(false, Ordering::Relaxed);
                                    r
                                })
                            });
                        }
                    }
                }
                let bots = local.port_mask();
                if bots == 0 {
                    return Ok(Flow::Continue);
                }
                *lock(&sound) = guest::hide(&mut *rt.memory(), bots).ok().flatten();
                Ok(Flow::Continue)
            },
        );
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
        let data = read
            .as_ref()
            .ok()
            .and_then(|b| bots::cache::MapData::from_bytes(b));
        match (data, read.is_ok(), auto.is_some()) {
            (Some((d, _)), ..) => {
                let (polys, links, walls) =
                    (d.nav.polys.len(), d.nav.edges.len(), d.walls.tris.len());
                if autonav::offer(&name, d, false) {
                    eprintln!(
                        "bots: {name}: {polys} polygons, {links} links, {walls} wall triangles"
                    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_watch::{Dialog, JOIN_SCREEN, Top};

    fn on(id: u32) -> Option<Top> {
        Some(Top {
            window: 5,
            slot: 4,
            id,
            screen: 0x8000_0000,
            live_row: None,
            mode: Some(1),
        })
    }

    const CHOOSER: Option<Dialog> = Some(Dialog {
        window: 3,
        slot: 4,
        id: Some(0x17),
    });

    #[test]
    fn a_bot_taps_only_on_the_bare_players_screen_with_no_profile() {
        assert!(join_gate(on(JOIN_SCREEN), None, Some(false)));
        assert!(!join_gate(on(JOIN_SCREEN), None, Some(true)), "signed in");
        assert!(!join_gate(on(JOIN_SCREEN), None, None), "slot unread");
        assert!(
            !join_gate(on(JOIN_SCREEN), CHOOSER, Some(false)),
            "a profile chooser open: A would pick the human's profile"
        );
        assert!(!join_gate(on(0x09), None, Some(false)), "the main menu");
        assert!(!join_gate(None, None, Some(false)), "no screen");
    }

    #[test]
    fn the_glue_signs_in_only_joining_bots_whose_gate_is_open() {
        let empty = |_: u8| Some(false);
        // Ports 1 and 3 joining, 2 an H2_BOTS bot already in.
        let slots = |p: u8| Some(p == 2);
        assert_eq!(
            to_sign_in(on(JOIN_SCREEN), None, 0b1110, slots, true, false),
            vec![1, 3]
        );
        assert_eq!(
            to_sign_in(on(JOIN_SCREEN), None, 0b0010, empty, true, false),
            vec![1],
            "a port with no joining bot is the human's"
        );
        assert!(
            to_sign_in(on(JOIN_SCREEN), None, 0b10, empty, false, false).is_empty(),
            "no human profile to copy yet"
        );
        assert!(
            to_sign_in(on(JOIN_SCREEN), None, 0b10, empty, true, true).is_empty(),
            "a game runs"
        );
        assert!(
            to_sign_in(on(JOIN_SCREEN), CHOOSER, 0b10, empty, true, false).is_empty(),
            "a profile chooser is open"
        );
        // The Xbox Live menu and anything else: never, so nothing a bot does
        // reaches a screen that would open a chooser for it.
        for id in [0x09, crate::custom_games::SCREEN_ID as u32] {
            assert!(
                to_sign_in(on(id), None, 0b10, empty, true, false).is_empty(),
                "screen {id:#x}"
            );
        }
        assert!(to_sign_in(None, None, 0b10, empty, true, false).is_empty());
    }

    #[test]
    fn the_gate_line_says_what_it_read() {
        assert_eq!(
            gate_line(1, on(JOIN_SCREEN), None, Some(false), Some(0)),
            "bots: join gate port 1: top 0x1e, dialog none, signed_in false (slot 0x0): signs in"
        );
        assert_eq!(
            gate_line(2, on(JOIN_SCREEN), CHOOSER, Some(false), Some(0)),
            "bots: join gate port 2: top 0x1e, dialog 0x17 (window 3, slot 4), signed_in false (slot 0x0): waits"
        );
        assert_eq!(
            gate_line(3, None, None, None, None),
            "bots: join gate port 3: top none, dialog none, signed_in ? (slot ?): waits"
        );
        assert_eq!(
            gate_line(1, on(0xba), None, Some(true), Some(0x10)),
            "bots: join gate port 1: top 0xba, dialog none, signed_in true (slot 0x10): waits",
            "a profile, not on Xbox Live"
        );
    }
}
