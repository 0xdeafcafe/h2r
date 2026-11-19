//! Local bots (docs/game/bots.md, sections 4 and 10): the hooks around the
//! `bots` crate.
//!
//! - The process-wide [`bots::local::global`] bots are an input backend,
//!   added after the host's own pads, so they can be added at any time
//!   (`H2_BOTS=<n>` adds `n` brains at boot; the agent API can add more).
//! - On the Split Screen players screen or in a pregame lobby the glue
//!   signs each bot in itself, with the game's own sign-in and a profile made from the
//!   human's ([`bots::guest::sign_in_then`]), before `SOUND_UPDATE`. A
//!   bot never presses a button to join (see "The join gate"). A bot's A
//!   that still reaches the players screen's handler for a controller with
//!   no profile (`PLAYERS_SCREEN_UNJOINED`, a sim script's) is answered
//!   the same way ([`bots::guest::sign_in`]).
//! - With a human on Xbox Live, out of a game, each signed-in bot goes on
//!   Live as its own bot account once the server leased it one
//!   ([`bots::local::Local::account_on`], [`bots::guest::go_live_as`],
//!   docs/game/bots.md §9.5), so the guest numbers stay free for real
//!   guests; until then (or with no roster) as the human's guest
//!   ([`bots::guest::go_live`], [`to_go_live`]), as an Xbox signs in a
//!   guest. A guest bot moves to its account when one comes
//!   ([`to_switch_account`]). When no human is on Live any more, a bot on
//!   Live as its account leaves Live ([`to_leave_live`],
//!   [`bots::guest::leave_live`]). Offline and in System Link, nothing.
//! - A Start press in the pregame lobby (`PREGAME_LOBBY_INPUT`, or A on
//!   the list's Start Game row, `LOBBY_ROW_PRESS`) while bots are in logs
//!   the game's start state and what holds it back
//!   ([`bots::lobby::start_state_then`]), before the next `SOUND_UPDATE`.
//! - A game start from the session (`SESSION_GAME_START`) with phantoms
//!   planned makes this console the session's host when no other console
//!   is in it ([`bots::phantom::session::game_start`]), so the game starts
//!   as the distributed server and the phantoms can join.
//! - Before each `RENDER_FRAME` (the game thread) each bot's source gives
//!   its pad for the frame ([`bots::local::Local::frame`]).
//! - Around `RENDER_FRAME` and `SOUND_UPDATE`, the bots' local users are
//!   taken out of the local player map ([`bots::guest::hide`]), so the
//!   views, HUDs and listeners are the humans' only. With the pipelined
//!   render the snapshot is taken inside `RENDER_FRAME`, so it has them
//!   hidden too; both points run on the game thread. The listener update
//!   (`SOUND_LISTENERS_UPDATE`), which the per-frame update also calls
//!   outside `SOUND_UPDATE`, gets the same window, and the bots' listeners
//!   are marked inactive ([`bots::guest::quiet_listeners`]).
//! - Around the players screen's update (`PLAYERS_SCREEN_UPDATE`) the
//!   bots' controller handles read as none ([`bots::guest::hide_pads`]),
//!   so its panes show the humans only ("CONNECT CONTROLLER" on a bot's
//!   port), joined or not.
//!
//! - Phantom peers ([`bots::phantom::global`], docs/game/bots.md §5.1):
//!   before each `RENDER_FRAME`, while this console hosts a pregame lobby
//!   (System Link or Xbox Live) or a multiplayer game, pending phantoms are
//!   written into the session as joining peers, so the lobby lists them
//!   and the game makes their players; each joined phantom's pad becomes
//!   its action in a game. When this console hosts no session any more
//!   they leave it and wait for the next (`H2_PHANTOM_LOBBY=0`: they join
//!   only a running game and leave when it ends).
//!
//! **The join gate.** A bot is signed in only while its own slot has no
//! profile, a human has one to copy, no game runs ([`to_sign_in`]) and
//! one of two screens is up ([`screen`]):
//!
//! - the players screen in Split Screen (mode 1; in Xbox Live, System
//!   Link or an invitation's mode it is no join screen, so the human has
//!   it alone) takes player one's input and nothing sits on windows 0 to
//!   4 (a dialog, or the profile chooser: [`crate::ui_watch::dialog`]);
//!   the bot's port also gets its bit in the screen's join mask;
//! - a pregame lobby (Live or System Link custom game), read from its
//!   squad session ([`bots::lobby::pregame`]), since `ui_watch` reads the
//!   lobby as no screen. The sign-in is all: the session's per-frame
//!   local-player pass (`0x65770`) adds the new local user, as for a
//!   second controller's profile chosen in the lobby. On Live it waits
//!   for the user's Xbox Live identity (a local user with the offline id's
//!   flags `0xBAD00000` counts as none there, `0x646df`), which the bot
//!   gets from [`bots::guest::go_live_as`] within 30 frames.
//!
//! Each port's gate is logged when it changes, naming the screen it
//! thinks is up (`players`, `lobby`, the top screen's id or `none`), and
//! each sign-in.
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
//! for the frame: it signs the leaving bots' slots out with the game's
//! `SLOT_SIGN_OUT` (off Xbox Live, the local user freed, the profile out:
//! the profile UI's own sign-out), then runs the frame itself; the lobby's
//! session drops a player whose local user went (`0x65770`, status 0). A
//! leaving bot whose slot holds no profile and no Live is unplugged
//! ([`bots::local::leave_step`], [`bots::guest::slot_held`]). A game never
//! sees a bot's pad go.
//!
//! With no bots every hook returns at once. The bots stay signed in until
//! they are taken off; going back to the players screen finds them joined,
//! though its panes do not show them.

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

/// Where a joining bot would join, as the gate sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    /// The players screen (`0x1e`) takes player one's input: a bot joins
    /// with the game's sign-in and its port's bit in the screen's join
    /// mask (`+8`) at `screen`.
    Players { screen: u32 },
    /// A pregame lobby (Live or System Link custom game,
    /// [`bots::lobby::pregame`]) and no game runs. The lobby is read from
    /// its squad session, not from the screens: it shares its window and
    /// slot with the screens over it, and `ui_watch` reads it as no screen
    /// (the user's run: `top none`). A bot joins with the game's sign-in
    /// alone: the session's per-frame local-player pass (`0x65770`) adds
    /// every local user the session has no player for (`0x646b0` status 2,
    /// `0x65a60`), as it does for a second controller's profile chosen
    /// in the lobby. No screen mask is written.
    Lobby,
    /// Anything else: the top screen's id, if one was read.
    Other(Option<u32>),
}

impl Screen {
    /// The name the gate line gives it: `players`, `lobby`, the screen's
    /// id, or `none`.
    pub fn name(&self) -> String {
        match self {
            Screen::Players { .. } => "players".to_owned(),
            Screen::Lobby => "lobby".to_owned(),
            Screen::Other(Some(id)) => format!("0x{id:02x}"),
            Screen::Other(None) => "none".to_owned(),
        }
    }

    /// The players screen's address, for its join mask; `None` in the
    /// lobby (and anywhere else).
    pub fn mask_screen(&self) -> Option<u32> {
        match self {
            Screen::Players { screen } => Some(*screen),
            _ => None,
        }
    }
}

/// The players screen's mode for Split Screen (`+0x610`, `0x25240c`):
/// the only one whose bots join on it.
pub const SPLIT_SCREEN: u32 = 1;

/// Which screen the gate is on: the players screen in Split Screen when it
/// takes player one's input (`top`, [`crate::ui_watch::read`], mode
/// [`SPLIT_SCREEN`]); otherwise the pregame lobby when its session is up
/// (`pregame`, [`bots::lobby::in_pregame`]) and no game runs (`in_game`);
/// otherwise the top screen read, if any.
///
/// The players screen in any other mode (Xbox Live 3, System Link 2, an
/// invitation 4, or a mode not read) is not a join screen: there the bots
/// would show as the human's local players, and the user wants that screen
/// to be theirs alone. It reads as the lobby when a pregame session is up
/// under it, else as itself (`0x1e`), where no one joins; the bots join in
/// the pregame lobby that follows, as opponents. Split Screen has no such
/// lobby (its local session is not networked), so there the players screen
/// is still where they join. Either way the screen's panes never show a
/// bot (`PLAYERS_SCREEN_UPDATE`, [`bots::guest::hide_pads`]).
pub fn screen(top: Option<crate::ui_watch::Top>, pregame: bool, in_game: bool) -> Screen {
    match top {
        Some(t) if t.id == crate::ui_watch::JOIN_SCREEN && t.mode == Some(SPLIT_SCREEN) => {
            Screen::Players { screen: t.screen }
        }
        _ if pregame && !in_game => Screen::Lobby,
        _ => Screen::Other(top.map(|t| t.id)),
    }
}

/// Whether the bot on a port may be signed in now, by the screen
/// ([`screen`]) and the port's slot (`signed_in`: `None` when the slot
/// could not be read, which is a no; a slot with a profile already, no).
///
/// - On the players screen, only with nothing on windows 0 to 4 (`dialog`,
///   [`crate::ui_watch::dialog`]): the screen ignores input under a
///   dialog, and the profile chooser it opens for a controller is one.
/// - In the lobby, whatever sits over it: the Clankers page and Game
///   Setup are dialogs over the lobby, and the sign-in presses nothing.
pub fn join_gate(
    screen: Screen,
    dialog: Option<crate::ui_watch::Dialog>,
    signed_in: Option<bool>,
) -> bool {
    let screen_open = match screen {
        Screen::Players { .. } => dialog.is_none(),
        Screen::Lobby => true,
        Screen::Other(_) => false,
    };
    screen_open && signed_in == Some(false)
}

/// The log line for one port's gate: the screen it thinks is up
/// ([`Screen::name`]) and the top screen read, what it read and what it
/// decided, with the slot's flags (`flags`, slot `+0`: `0x10` a profile,
/// `0x20` signed in to Xbox Live), for what the lobby makes of the bot.
pub fn gate_line(
    port: u8,
    screen: Screen,
    top: Option<crate::ui_watch::Top>,
    dialog: Option<crate::ui_watch::Dialog>,
    signed_in: Option<bool>,
    flags: Option<u32>,
) -> String {
    let verdict = if join_gate(screen, dialog, signed_in) {
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
        "bots: join gate port {port}: screen {} (top {top}), dialog {dialog}, signed_in {signed} (slot {flags}): {verdict}",
        screen.name()
    )
}

/// Frames between sign-in tries (a try that did not take, as with no
/// human profile yet, is not made every frame).
pub const SIGN_IN_EVERY: u64 = 30;

/// A try made at most every [`SIGN_IN_EVERY`] render frames, from a hook
/// that may not run on every render frame. `frame % 30 == 0` was the test
/// before: `SOUND_UPDATE` does not run once per `RENDER_FRAME`, so on a
/// run whose sound updates never fell on a multiple of 30 nothing was ever
/// tried. Now the first call is due, then the first one 30 frames or more
/// after the last that was.
#[derive(Debug)]
pub struct Every(AtomicU64);

impl Default for Every {
    fn default() -> Self {
        Every(AtomicU64::new(u64::MAX))
    }
}

impl Every {
    /// Whether a try is due at render frame `frame`; a due one counts as
    /// made.
    pub fn due(&self, frame: u64) -> bool {
        let last = self.0.load(Ordering::Relaxed);
        if last == u64::MAX || frame < last || frame - last >= SIGN_IN_EVERY {
            self.0.store(frame, Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

/// Why the glue signs no one in although a joining bot's gate is open
/// (`open`), or `None`: a game runs (`in_game`, [`bots::guest::in_game`]),
/// or no port without a bot has a profile to copy (`human`). Logged once
/// per change.
pub fn sign_in_skip(open: bool, human: bool, in_game: bool) -> Option<&'static str> {
    if !open {
        None
    } else if in_game {
        Some("a game runs")
    } else if !human {
        Some("no human profile to copy (no port without a bot has a profile)")
    } else {
        None
    }
}

/// The ports whose bot the glue signs in now: each joining bot's port
/// (`joining`, a mask) whose [`join_gate`] is open, while no game runs
/// (`in_game`) and a human has a profile to copy (`human`).
pub fn to_sign_in(
    screen: Screen,
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
        .filter(|&p| joining & 1 << p != 0 && join_gate(screen, dialog, signed_in(p)))
        .collect()
}

/// The log line for one bot the glue signs in, and where: on the players
/// screen (with its join mask) or in the pregame lobby (the session adds
/// the player itself).
pub fn sign_in_line(name: &str, port: u8, screen: Screen) -> String {
    let at = match screen {
        Screen::Players { .. } => "on the players screen",
        Screen::Lobby => "in the pregame lobby",
        Screen::Other(_) => "off any join screen",
    };
    format!("bots: signing {name} in on port {port} {at}")
}

/// The ports whose bot the glue signs in to Xbox Live as a guest of the
/// human now ([`bots::guest::go_live`]): each bot on the books (`bots`, a
/// mask) whose slot has a profile and is not on Xbox Live (`slot`, its
/// flags; `None` unread), while a human is on Xbox Live (`host_live`,
/// [`bots::guest::live_host`]) and no game runs (`in_game`). Offline and in
/// System Link with no one on Live, none: nothing changes there.
pub fn to_go_live(
    bots: u32,
    slot: impl Fn(u8) -> Option<u32>,
    host_live: bool,
    in_game: bool,
) -> Vec<u8> {
    if in_game || !host_live {
        return Vec::new();
    }
    (0..4u8)
        .filter(|&p| bots & 1 << p != 0)
        .filter(|&p| {
            slot(p).is_some_and(|f| f & guest::SLOT_PROFILE != 0 && f & guest::SLOT_LIVE == 0)
        })
        .collect()
}

/// The ports whose bot is on Xbox Live (`live_xuid`: the slot's record's
/// user id, `None` when off Live or unread) as something other than its
/// bot account (`account`: the xuid the driver gave it), so it goes on as
/// that account now ([`bots::guest::go_live_as`]): a guest bot whose
/// account came, or a bot whose roster changed. Only with a human on Xbox
/// Live and out of a game.
pub fn to_switch_account(
    bots: u32,
    live_xuid: impl Fn(u8) -> Option<u64>,
    account: impl Fn(u8) -> Option<u64>,
    host_live: bool,
    in_game: bool,
) -> Vec<u8> {
    if in_game || !host_live {
        return Vec::new();
    }
    (0..4u8)
        .filter(|&p| bots & 1 << p != 0)
        .filter(|&p| match (live_xuid(p), account(p)) {
            (Some(on), Some(want)) => on != want,
            _ => false,
        })
        .collect()
}

/// The ports whose bot is on Xbox Live as its own bot account (`slot`:
/// the flags and record's guest number; a guest is signed out by the game
/// itself, `0x190186`) once no human is on Live (`host_live`), out of a
/// game: they leave Live ([`bots::guest::leave_live`]).
pub fn to_leave_live(
    bots: u32,
    slot: impl Fn(u8) -> Option<(u32, u32)>,
    host_live: bool,
    in_game: bool,
) -> Vec<u8> {
    if in_game || host_live {
        return Vec::new();
    }
    (0..4u8)
        .filter(|&p| bots & 1 << p != 0)
        .filter(|&p| slot(p).is_some_and(|(f, guest)| f & guest::SLOT_LIVE != 0 && guest == 0))
        .collect()
}

/// The log line for what [`bots::guest::go_live`] did for the bot on
/// `port`.
pub fn go_live_line(port: u8, name: &str, r: guest::GoLive) -> String {
    use guest::GoLive::*;
    match r {
        Guest { host, number } => format!(
            "bots: {name} on port {port} is on Xbox Live as guest {number} of port {host}'s account"
        ),
        Account { host } => format!(
            "bots: {name} on port {port} is on Xbox Live as its own bot account, beside port {host}'s"
        ),
        NoHost => format!("bots: {name} on port {port} stays offline: no one is on Xbox Live"),
        NoProfile => format!("bots: {name} on port {port} has no profile to take on Xbox Live"),
        Already => format!("bots: {name} on port {port} is on Xbox Live already"),
        Full => format!(
            "bots: {name} on port {port} stays offline: the account has three guests already"
        ),
        NoLocalUser => {
            format!("bots: {name} on port {port} stays offline: its slot has no local user")
        }
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
    /// Each port's last join-gate line, so it is logged on a change.
    gates: Arc<Mutex<[String; 4]>>,
    /// A sign-in or sign-out is running the frame itself: its own
    /// `SOUND_UPDATE` call must not start another.
    signing_out: Arc<AtomicBool>,
    /// Each port's last Xbox Live guest line, so it is logged on a change.
    live: Arc<Mutex<[String; 4]>>,
    /// Each bot port's player id from before it went on Xbox Live (its
    /// profile sign-in's), for [`bots::guest::leave_live`].
    offline_ids: Arc<Mutex<[Option<[u8; guest::ID_LEN]>; 4]>>,
    /// A Start press in the lobby waits for its start-state line.
    pressed: Arc<AtomicBool>,
    /// The bot controllers hidden around the players screen's update.
    panes: Arc<Mutex<Option<guest::HiddenPads>>>,
    /// The bot users hidden around the listener update.
    listeners: Arc<Mutex<Option<Hidden>>>,
    /// The last lobby facts line said.
    lobby_said: Arc<Mutex<String>>,
    /// When the Xbox Live and the sign-in tries were last made.
    live_every: Arc<Every>,
    sign_in_every: Arc<Every>,
    /// The last "signs no one in" reason said ([`sign_in_skip`]).
    skipped: Arc<Mutex<Option<&'static str>>>,
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
            live: Arc::default(),
            offline_ids: Arc::default(),
            pressed: Arc::default(),
            panes: Arc::default(),
            listeners: Arc::default(),
            lobby_said: Arc::default(),
            live_every: Arc::default(),
            sign_in_every: Arc::default(),
            skipped: Arc::default(),
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

        // The players screen's panes: a bot port reads as no controller
        // while the screen lays them out, so the screen shows the humans
        // only, joined or not (docs/game/bots.md §4.3).
        let (local, panes) = (self.local.clone(), Arc::clone(&self.panes));
        b.raw().before(
            &points::PLAYERS_SCREEN_UPDATE,
            "hide bot controllers from the players screen",
            move |c| {
                let bots = local.port_mask();
                if bots == 0 {
                    return Ok(Flow::Continue);
                }
                let rt = Arc::clone(c.ctx.runtime());
                *lock(&panes) = guest::hide_pads(&mut *rt.memory(), bots).ok().flatten();
                Ok(Flow::Continue)
            },
        );
        let panes = Arc::clone(&self.panes);
        b.raw().after(
            &points::PLAYERS_SCREEN_UPDATE,
            "bot controllers back",
            move |c| {
                if let Some(h) = lock(&panes).take() {
                    let rt = Arc::clone(c.ctx.runtime());
                    guest::restore_pads(&mut *rt.memory(), &h).map_err(|e| fault(e.addr))?;
                }
                Ok(())
            },
        );

        // The listeners, wherever they are updated: SOUND_UPDATE's own call
        // runs inside its window, but the per-frame update `0x18c060` calls
        // the listener update outside it, and there the bots' users were
        // made listeners, so a bot's own sounds started as heard by it
        // (docs/game/bots.md §4.3, "Listeners").
        let (local, listeners) = (self.local.clone(), Arc::clone(&self.listeners));
        b.raw().before(
            &points::SOUND_LISTENERS_UPDATE,
            "hide bot listeners",
            move |c| {
                let bots = local.port_mask();
                if bots == 0 {
                    return Ok(Flow::Continue);
                }
                let rt = Arc::clone(c.ctx.runtime());
                *lock(&listeners) = hide_quiet(&mut *rt.memory(), bots);
                Ok(Flow::Continue)
            },
        );
        let listeners = Arc::clone(&self.listeners);
        b.raw().after(
            &points::SOUND_LISTENERS_UPDATE,
            "bot listeners back",
            move |c| {
                if let Some(h) = lock(&listeners).take() {
                    let rt = Arc::clone(c.ctx.runtime());
                    guest::restore(&mut *rt.memory(), &h).map_err(|e| fault(e.addr))?;
                }
                Ok(())
            },
        );

        // A Start press in the lobby while bots are in: the start state is
        // logged before the next SOUND_UPDATE (once per press).
        let (local, pressed) = (self.local.clone(), Arc::clone(&self.pressed));
        b.raw().observe_before(
            &points::PREGAME_LOBBY_INPUT,
            "bots: note a Start press",
            move |o| {
                if local.port_mask() == 0 {
                    return;
                }
                let Some(event) = o.arg_u32("event") else {
                    return;
                };
                if let Ok(Some(_)) = bots::lobby::start_press(&*o.guest(), event) {
                    pressed.store(true, Ordering::Relaxed);
                }
            },
        );

        // A on the lobby list's Start Game row: the list takes it before
        // the screen's handler, so it is noted here too.
        let (local, pressed) = (self.local.clone(), Arc::clone(&self.pressed));
        b.raw().observe_before(
            &points::LOBBY_ROW_PRESS,
            "bots: note a Start Game press",
            move |o| {
                if local.port_mask() == 0 {
                    return;
                }
                let (Some(item), Some(row)) = (o.arg_u32("item"), o.arg_u32("row")) else {
                    return;
                };
                if let Ok(Some(_)) = bots::lobby::start_row_press(&*o.guest(), item, row) {
                    pressed.store(true, Ordering::Relaxed);
                }
            },
        );

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
        let lobby_said = Arc::clone(&self.lobby_said);
        b.raw().before(
            &points::RENDER_FRAME,
            "bot pads; hide bot views",
            move |c| {
                let bots = local.port_mask();
                let any_phantoms = !phantoms.is_empty();
                let f = frame.fetch_add(1, Ordering::Relaxed);
                // A Live session made after the Clankers page was set gets
                // its roster (docs/game/bots.md stage 6c), and a
                // Clankermatch lobby its bots: before any bot is in, so
                // this runs with none.
                let rt = Arc::clone(c.ctx.runtime());
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
                if bots == 0 && !any_phantoms {
                    return Ok(Flow::Continue);
                }
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
                // The local bots' names carry the bot mark, as phantoms'
                // do (docs/game/bots.md §8.2.2), on the session's host.
                match bots::phantom::session::mark_local_bots(&mut *mem, bots) {
                    Ok(slots) if !slots.is_empty() => {
                        eprintln!(
                            "bots: the bot mark on local bots' names (session players {slots:?})"
                        )
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("bots: bot mark: fault at {:#x}", e.addr),
                }
                // The join gate, per port, logged when it changes; a bot it
                // lets through presses nothing (the sign-in is before
                // SOUND_UPDATE).
                let top = crate::ui_watch::read(&*mem).top;
                let dialog = crate::ui_watch::dialog(&*mem);
                let joining = local.joining_mask();
                let g: &runtime::Guest = &mem;
                let facts = bots::lobby::facts(g).ok();
                let on = screen(
                    top,
                    facts.as_ref().is_some_and(bots::lobby::pregame),
                    guest::in_game(g),
                );
                // What the lobby read gave, once per change.
                if let Some(f) = &facts {
                    let line = bots::lobby::pregame_line(f);
                    let mut said = lock(&lobby_said);
                    if *said != line {
                        eprintln!("{line}");
                        *said = line;
                    }
                }
                let mut open = [false; 4];
                {
                    let mut said = lock(&gates);
                    for port in 0..4u8 {
                        if joining & 1 << port == 0 {
                            said[usize::from(port)].clear();
                            continue;
                        }
                        let signed_in = guest::signed_in(g, u32::from(port)).ok();
                        open[usize::from(port)] = join_gate(on, dialog, signed_in);
                        let flags = g.u32(guest::slot(u32::from(port))).ok();
                        let line = gate_line(port, on, top, dialog, signed_in, flags);
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
        let (local, sound, busy, frame, live, pressed, offline_ids) = (
            self.local.clone(),
            Arc::clone(&self.sound),
            Arc::clone(&self.signing_out),
            Arc::clone(&self.frame),
            Arc::clone(&self.live),
            Arc::clone(&self.pressed),
            Arc::clone(&self.offline_ids),
        );
        let (live_every, sign_in_every, skipped) = (
            Arc::clone(&self.live_every),
            Arc::clone(&self.sign_in_every),
            Arc::clone(&self.skipped),
        );
        b.raw().before(
            &points::SOUND_UPDATE,
            "sign bots in and leaving bots out; hide bot listeners",
            move |c| {
                let rt = Arc::clone(c.ctx.runtime());
                // A Start press: the game's start state and what holds it.
                if !busy.load(Ordering::Relaxed) && pressed.swap(false, Ordering::Relaxed) {
                    let mask = local.port_mask();
                    let ports: Vec<u32> = (0..4u32).filter(|&p| mask & 1 << p != 0).collect();
                    busy.store(true, Ordering::Relaxed);
                    let busy = Arc::clone(&busy);
                    let then = points::SOUND_UPDATE.base;
                    return c.answer_async(move |cx| -> engine::entry::EntryFuture {
                        Box::pin(async move {
                            let r = bots::lobby::start_state_then(cx, ports, then).await;
                            busy.store(false, Ordering::Relaxed);
                            r.map(|(line, ret)| {
                                eprintln!("{line}");
                                ret
                            })
                        })
                    });
                }
                // Bots with a profile go on Xbox Live as their bot accounts
                // (or, with none yet, as the human's guests).
                let joined = local.joining_mask();
                if joined != 0 && live_every.due(frame.load(Ordering::Relaxed)) {
                    let mut mem = rt.memory();
                    let bots = local.port_mask();
                    let host_live = guest::live_host(&*mem, bots).ok().flatten().is_some();
                    let in_game = guest::in_game(&*mem);
                    let account = |p: u8| local.account_on(u32::from(p));
                    let mut ports = to_go_live(
                        joined,
                        |p| guest::slot_flags(&*mem, u32::from(p)).ok(),
                        host_live,
                        in_game,
                    );
                    let live_xuid = |p: u8| {
                        let p = u32::from(p);
                        guest::on_live(&*mem, p)
                            .ok()
                            .filter(|on| *on)
                            .and_then(|_| guest::account(&*mem, p).ok())
                            .map(|a| guest::account_xuid(&a))
                    };
                    ports.extend(to_switch_account(
                        joined,
                        live_xuid,
                        |p| account(p).map(|a| a.xuid),
                        host_live,
                        in_game,
                    ));
                    // No one on Live any more: bot accounts leave it.
                    for p in to_leave_live(
                        joined,
                        |p| {
                            let p = u32::from(p);
                            let f = guest::slot_flags(&*mem, p).ok()?;
                            let g = guest::account(&*mem, p).ok()?;
                            Some((f, guest::guest_number(&g)))
                        },
                        host_live,
                        in_game,
                    ) {
                        let name = local.name_on(u32::from(p)).unwrap_or_default();
                        let Some(id) = lock(&offline_ids)[usize::from(p)] else {
                            continue;
                        };
                        if let Ok(true) = guest::leave_live(&mut *mem, u32::from(p), &id, &name) {
                            eprintln!("bots: {name} on port {p} leaves Xbox Live: no one is on it");
                        }
                    }
                    let mut said = lock(&live);
                    for p in 0..4u8 {
                        if !ports.contains(&p) {
                            said[usize::from(p)].clear();
                        }
                    }
                    for p in ports {
                        let name = local.name_on(u32::from(p)).unwrap_or_default();
                        // The player id it had offline, kept for leaving.
                        if let Ok(false) = guest::on_live(&*mem, u32::from(p))
                            && let Ok(id) = guest::slot_id(&*mem, u32::from(p))
                        {
                            lock(&offline_ids)[usize::from(p)] = Some(id);
                        }
                        let r = match account(p) {
                            Some(a) => guest::go_live_as(&mut *mem, u32::from(p), bots, &a),
                            None => guest::go_live(&mut *mem, u32::from(p), bots, &name),
                        };
                        let line = match r {
                            Ok(r) => go_live_line(p, &name, r),
                            Err(e) => format!(
                                "bots: {name} on port {p}: Xbox Live: fault at {:#x}",
                                e.addr
                            ),
                        };
                        if said[usize::from(p)] != line {
                            eprintln!("{line}");
                            said[usize::from(p)] = line;
                        }
                    }
                }
                let joining = local.joining_mask();
                if joining != 0
                    && !busy.load(Ordering::Relaxed)
                    && sign_in_every.due(frame.load(Ordering::Relaxed))
                {
                    let bots = local.port_mask();
                    let (ports, screen, skip) = {
                        let mem = rt.memory();
                        let in_game = guest::in_game(&*mem);
                        let on = screen(
                            crate::ui_watch::read(&*mem).top,
                            bots::lobby::in_pregame(&*mem),
                            in_game,
                        );
                        let dialog = crate::ui_watch::dialog(&*mem);
                        let signed = |p: u8| guest::signed_in(&*mem, u32::from(p)).ok();
                        let human = guest::human_port(&*mem, bots).ok().flatten().is_some();
                        let open = (0..4u8)
                            .any(|p| joining & 1 << p != 0 && join_gate(on, dialog, signed(p)));
                        let ports = to_sign_in(on, dialog, joining, signed, human, in_game);
                        (ports, on, sign_in_skip(open, human, in_game))
                    };
                    let mut said = lock(&skipped);
                    if *said != skip {
                        if let Some(why) = skip {
                            eprintln!("bots: the join gate is open, but no one signs in: {why}");
                        }
                        *said = skip;
                    }
                    drop(said);
                    if !ports.is_empty() {
                        let joins: Vec<(u32, String)> = ports
                            .iter()
                            .map(|&p| {
                                let name = local.name_on(u32::from(p)).unwrap_or_default();
                                eprintln!("{}", sign_in_line(&name, p, screen));
                                (u32::from(p), name)
                            })
                            .collect();
                        let screen = screen.mask_screen();
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
                            let in_game = guest::in_game(&*mem);
                            local::leave_step(in_game, &leaving, |p| {
                                guest::slot_held(&*mem, u32::from(p)).unwrap_or(false)
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
                *lock(&sound) = hide_quiet(&mut *rt.memory(), bots);
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

/// Hides the bots' users ([`bots::guest::hide`]) and makes their listeners
/// inactive ([`bots::guest::quiet_listeners`]), for the sound frame and
/// the listener update.
fn hide_quiet(mem: &mut runtime::Guest, bots: u32) -> Option<Hidden> {
    let h = guest::hide(&mut *mem, bots).ok().flatten()?;
    let _ = guest::quiet_listeners(&mut *mem, &h);
    Some(h)
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

    /// The players screen, as [`screen`] reads it from `on(JOIN_SCREEN)`.
    const PLAYERS: Screen = Screen::Players {
        screen: 0x8000_0000,
    };

    #[test]
    fn the_screen_is_the_players_screen_then_the_pregame_lobby_then_whatever_was_read() {
        assert_eq!(screen(on(JOIN_SCREEN), false, false), PLAYERS);
        assert_eq!(
            screen(on(JOIN_SCREEN), true, false),
            PLAYERS,
            "the players screen up over a lobby session: its mask"
        );
        // The user's run: a Live custom game lobby reads as no screen.
        assert_eq!(screen(None, true, false), Screen::Lobby);
        assert_eq!(
            screen(on(0xe7), true, false),
            Screen::Lobby,
            "a screen read over the lobby"
        );
        assert_eq!(
            screen(None, true, true),
            Screen::Other(None),
            "a game keeps the session in its lobby states: not the lobby"
        );
        assert_eq!(screen(on(0xba), false, false), Screen::Other(Some(0xba)));
        assert_eq!(screen(None, false, false), Screen::Other(None));
        assert_eq!(PLAYERS.name(), "players");
        assert_eq!(Screen::Lobby.name(), "lobby");
        assert_eq!(Screen::Other(Some(0xba)).name(), "0xba");
        assert_eq!(Screen::Other(None).name(), "none");
        assert_eq!(PLAYERS.mask_screen(), Some(0x8000_0000));
        assert_eq!(Screen::Lobby.mask_screen(), None, "no mask in the lobby");
    }

    /// The players screen in `mode` (1 Split Screen, 2 System Link, 3 Xbox
    /// Live, 4 an invitation).
    fn players_in(mode: Option<u32>) -> Option<Top> {
        on(JOIN_SCREEN).map(|t| Top { mode, ..t })
    }

    #[test]
    fn only_split_screen_bots_join_on_the_players_screen() {
        let empty = |_: u8| Some(false);
        // The user's ask: the Xbox Live players screen is theirs alone.
        for mode in [Some(2), Some(3), Some(4), None] {
            let at = screen(players_in(mode), false, false);
            assert_eq!(at, Screen::Other(Some(JOIN_SCREEN)), "mode {mode:?}");
            assert_eq!(at.mask_screen(), None, "no join mask in mode {mode:?}");
            assert!(
                to_sign_in(at, None, 0b1110, empty, true, false).is_empty(),
                "mode {mode:?}: no bot signs in on the players screen"
            );
            // A pregame session up under it: they join as in the lobby
            // (no mask), and the panes hide them anyway.
            assert_eq!(screen(players_in(mode), true, false), Screen::Lobby);
        }
        // Split Screen: no lobby to join in, so the players screen.
        let split = screen(players_in(Some(SPLIT_SCREEN)), false, false);
        assert_eq!(split, PLAYERS);
        assert_eq!(
            to_sign_in(split, None, 0b1110, empty, true, false),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn a_bot_joins_only_on_the_bare_players_screen_with_no_profile() {
        assert!(join_gate(PLAYERS, None, Some(false)));
        assert!(!join_gate(PLAYERS, None, Some(true)), "signed in");
        assert!(!join_gate(PLAYERS, None, None), "slot unread");
        assert!(
            !join_gate(PLAYERS, CHOOSER, Some(false)),
            "a profile chooser open: A would pick the human's profile"
        );
        assert!(
            !join_gate(Screen::Other(Some(0x09)), None, Some(false)),
            "the main menu"
        );
        assert!(
            !join_gate(Screen::Other(None), None, Some(false)),
            "no screen"
        );
    }

    #[test]
    fn the_gate_opens_in_the_pregame_lobby_whatever_sits_over_it() {
        assert!(join_gate(Screen::Lobby, None, Some(false)));
        assert!(
            join_gate(Screen::Lobby, CHOOSER, Some(false)),
            "the Clankers page or Game Setup over the lobby: the sign-in presses nothing"
        );
        assert!(!join_gate(Screen::Lobby, None, Some(true)), "signed in");
        assert!(!join_gate(Screen::Lobby, None, None), "slot unread");
    }

    #[test]
    fn the_glue_signs_in_only_joining_bots_whose_gate_is_open() {
        let empty = |_: u8| Some(false);
        // Ports 1 and 3 joining, 2 an H2_BOTS bot already in.
        let slots = |p: u8| Some(p == 2);
        assert_eq!(
            to_sign_in(PLAYERS, None, 0b1110, slots, true, false),
            vec![1, 3]
        );
        assert_eq!(
            to_sign_in(PLAYERS, None, 0b0010, empty, true, false),
            vec![1],
            "a port with no joining bot is the human's"
        );
        assert!(
            to_sign_in(PLAYERS, None, 0b10, empty, false, false).is_empty(),
            "no human profile to copy yet"
        );
        assert!(
            to_sign_in(PLAYERS, None, 0b10, empty, true, true).is_empty(),
            "a game runs"
        );
        assert!(
            to_sign_in(PLAYERS, CHOOSER, 0b10, empty, true, false).is_empty(),
            "a profile chooser is open"
        );
        // The Xbox Live menu and anything else: never, so nothing a bot does
        // reaches a screen that would open a chooser for it.
        for id in [0x09, crate::custom_games::SCREEN_ID as u32] {
            assert!(
                to_sign_in(Screen::Other(Some(id)), None, 0b10, empty, true, false).is_empty(),
                "screen {id:#x}"
            );
        }
        assert!(to_sign_in(Screen::Other(None), None, 0b10, empty, true, false).is_empty());
    }

    #[test]
    fn in_the_pregame_lobby_each_joining_bot_is_signed_in_and_none_during_a_game() {
        // The user's run: Live, Clankers On, Count 5, hosted from the Live
        // menu. Ports 1 to 3 joining, no profile (slot 0x1), the lobby read
        // as no screen: each is signed in, the Clankers page open or not.
        let empty = |_: u8| Some(false);
        let lobby = screen(None, true, false);
        assert_eq!(
            to_sign_in(lobby, None, 0b1110, empty, true, false),
            vec![1, 2, 3]
        );
        assert_eq!(
            to_sign_in(lobby, CHOOSER, 0b1110, empty, true, false),
            vec![1, 2, 3]
        );
        // Each is signed in once: a slot with its profile is not again.
        let slots = |p: u8| Some(p != 3);
        assert_eq!(to_sign_in(lobby, None, 0b1110, slots, true, false), vec![3]);
        // The game: the session stays in its lobby states, but nothing.
        let in_game = screen(None, true, true);
        assert!(to_sign_in(in_game, None, 0b1110, empty, true, true).is_empty());
        assert!(
            to_sign_in(Screen::Lobby, None, 0b1110, empty, true, true).is_empty(),
            "a game runs, whatever the screen says"
        );
        assert!(
            to_sign_in(lobby, None, 0b1110, empty, false, false).is_empty(),
            "no human profile to copy"
        );
    }

    #[test]
    fn a_try_is_due_at_once_then_every_30_frames_whatever_frames_the_hook_sees() {
        // The h2 bots test run: SOUND_UPDATE never fell on a multiple of
        // 30, so `frame % 30 == 0` never tried.
        let e = Every::default();
        assert!(e.due(7), "the first call");
        assert!(!e.due(8));
        assert!(!e.due(36));
        assert!(e.due(37), "30 frames on");
        assert!(!e.due(65));
        assert!(e.due(101), "a hook that skipped frames");
        assert!(e.due(3), "the counter went back: due");
        let odd = Every::default();
        let tried = (0..200u64)
            .map(|f| 2 * f + 1)
            .filter(|&f| odd.due(f))
            .count();
        assert!(
            tried >= 13,
            "odd frames only: still about every 30 ({tried})"
        );
    }

    #[test]
    fn a_gate_open_with_no_sign_in_says_why() {
        assert_eq!(sign_in_skip(false, false, true), None, "no gate open");
        assert_eq!(sign_in_skip(true, true, false), None, "it signs in");
        assert_eq!(sign_in_skip(true, true, true), Some("a game runs"));
        assert_eq!(
            sign_in_skip(true, false, false),
            Some("no human profile to copy (no port without a bot has a profile)")
        );
    }

    #[test]
    fn the_sign_in_line_says_where() {
        assert_eq!(
            sign_in_line("Clanker 1", 1, Screen::Lobby),
            "bots: signing Clanker 1 in on port 1 in the pregame lobby"
        );
        assert_eq!(
            sign_in_line("Clanker 2", 2, PLAYERS),
            "bots: signing Clanker 2 in on port 2 on the players screen"
        );
    }

    const PROFILE: u32 = guest::SLOT_PROFILE;
    const LIVE: u32 = guest::SLOT_PROFILE | guest::SLOT_LIVE;

    #[test]
    fn bots_go_on_live_only_with_a_human_on_live_and_out_of_a_game() {
        // The human on port 0, three bots signed in to profiles.
        let slots = |p: u8| Some(if p == 0 { LIVE } else { PROFILE });
        assert_eq!(to_go_live(0b1110, slots, true, false), vec![1, 2, 3]);
        // Offline, or System Link with no one on Live: nothing changes.
        assert!(to_go_live(0b1110, slots, false, false).is_empty());
        // Never during a game.
        assert!(to_go_live(0b1110, slots, true, true).is_empty());
        // Only bots' ports: the human's is never touched.
        assert_eq!(to_go_live(0b0010, slots, true, false), vec![1]);
    }

    #[test]
    fn a_bot_goes_on_live_once_it_has_a_profile_and_once_only() {
        // Port 1 on Live already, port 2 still joining (no profile), port 3
        // unread.
        let slots = |p: u8| match p {
            1 => Some(LIVE),
            2 => Some(0),
            3 => None,
            _ => Some(LIVE),
        };
        assert!(to_go_live(0b1110, slots, true, false).is_empty());
        // Once port 2 has its profile, it goes on.
        let slots = |p: u8| Some(if p == 2 { PROFILE } else { LIVE });
        assert_eq!(to_go_live(0b1110, slots, true, false), vec![2]);
    }

    #[test]
    fn a_bot_moves_to_its_bot_account_when_one_comes_and_leaves_with_the_human() {
        const BOT: u64 = 0x000b_0000_0000_0042;
        const HOST: u64 = 0x0009_0000_0000_0afa;
        // Port 1 a guest (the host's id), port 2 already its account, port 3
        // off Live.
        let on = |p: u8| match p {
            1 => Some(HOST),
            2 => Some(BOT + 2),
            _ => None,
        };
        let account = |p: u8| Some(BOT + u64::from(p));
        assert_eq!(
            to_switch_account(0b1110, on, account, true, false),
            vec![1],
            "the guest moves; the one on its account stays; one off Live is to_go_live's"
        );
        assert!(
            to_switch_account(0b1110, on, |_| None, true, false).is_empty(),
            "no account yet"
        );
        assert!(to_switch_account(0b1110, on, account, false, false).is_empty());
        assert!(
            to_switch_account(0b1110, on, account, true, true).is_empty(),
            "not in a game"
        );
        assert!(to_switch_account(0b0100, on, account, true, false).is_empty());

        // The human leaves Live: the bot accounts leave too; a guest is the
        // game's own to sign out, an offline bot has nothing to leave.
        let slots = |p: u8| match p {
            1 => Some((LIVE, 1)),
            2 => Some((LIVE, 0)),
            _ => Some((PROFILE, 0)),
        };
        assert_eq!(to_leave_live(0b1110, slots, false, false), vec![2]);
        assert!(
            to_leave_live(0b1110, slots, true, false).is_empty(),
            "the human is still on"
        );
        assert!(
            to_leave_live(0b1110, slots, false, true).is_empty(),
            "not in a game"
        );
    }

    #[test]
    fn the_go_live_line_says_whose_guest() {
        use guest::GoLive;
        assert_eq!(
            go_live_line(1, "Clanker 1", GoLive::Guest { host: 0, number: 1 }),
            "bots: Clanker 1 on port 1 is on Xbox Live as guest 1 of port 0's account"
        );
        assert_eq!(
            go_live_line(2, "Rusty Lugnut", GoLive::Account { host: 0 }),
            "bots: Rusty Lugnut on port 2 is on Xbox Live as its own bot account, beside port 0's"
        );
        assert_eq!(
            go_live_line(3, "Clanker 3", GoLive::Full),
            "bots: Clanker 3 on port 3 stays offline: the account has three guests already"
        );
    }

    #[test]
    fn the_gate_line_says_what_it_read() {
        assert_eq!(
            gate_line(1, PLAYERS, on(JOIN_SCREEN), None, Some(false), Some(0)),
            "bots: join gate port 1: screen players (top 0x1e), dialog none, signed_in false (slot 0x0): signs in"
        );
        assert_eq!(
            gate_line(2, PLAYERS, on(JOIN_SCREEN), CHOOSER, Some(false), Some(0)),
            "bots: join gate port 2: screen players (top 0x1e), dialog 0x17 (window 3, slot 4), signed_in false (slot 0x0): waits"
        );
        assert_eq!(
            gate_line(3, Screen::Other(None), None, None, None, None),
            "bots: join gate port 3: screen none (top none), dialog none, signed_in ? (slot ?): waits"
        );
        assert_eq!(
            gate_line(
                1,
                Screen::Other(Some(0xba)),
                on(0xba),
                None,
                Some(true),
                Some(0x10)
            ),
            "bots: join gate port 1: screen 0xba (top 0xba), dialog none, signed_in true (slot 0x10): waits",
            "a profile, not on Xbox Live"
        );
        // The user's run, now: the Live lobby read as no screen is the lobby.
        assert_eq!(
            gate_line(1, Screen::Lobby, None, None, Some(false), Some(0x1)),
            "bots: join gate port 1: screen lobby (top none), dialog none, signed_in false (slot 0x1): signs in"
        );
    }
}
