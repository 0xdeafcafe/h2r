//! What a switch does (docs/game/party-lobby.md, "Switching"): the GAME
//! MODE picker's pick and the Xbox Live menu's Custom Games, Forge and
//! Firefight rows ([`run_to`]). In this order: the start state checked (no switch while
//! the game loads or counts down, nor once a search has hosted its lobby),
//! every refusal the switch can meet checked before anything changes, our
//! parts set ([`set_ours`]: the lobby's mode, the `firefight` setting held
//! on for FIREFIGHT), the retail lobby mode set when it differs
//! ([`to_custom`], [`to_co_op`], [`to_matchmaking`]), then the map put
//! inside the mode's category through `LOBBY_SET_MAP` ([`map_for`]).
//!
//! **The retail routines** are the game's own, run from the picker's
//! action or the lobby's panel update with no Game Setup open, less the
//! dialog close each press ends with (read from the XBE, 2026-11-22,
//! static):
//!
//! - CUSTOM GAMES, FORGE, FIREFIGHT from another mode: Switch To: Custom
//!   Game's press (`0x2bba66`, Game Setup action 7).
//! - CO-OP: Switch To: Co-Op Game's press after its gate (`0x2bba01`,
//!   action 6), and on Xbox Live `coop_live` armed as the Co-op Campaign
//!   row arms it (`coop_live::Lobby::host_retail`).
//! - MULTIPLAYER: the OptiMatch playlist press (`0x2b2181`) for the
//!   playlist the mode opens on ([`opening_hopper`]): the last one this
//!   profile played when it is listed, else the first listed. The
//!   playlists are listed on one page, ordinary and Clanker ones
//!   ([`Scope::All`]).
//!
//! Every refusal goes to the status line ([`super::notice`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::points;
use settings::{Store, Value};

use super::{Place, Reason, RetailMode, Tab, notice};
use crate::clankermatch::{self, Scope};
use crate::firefight::options as ff;
use crate::lobby_start::{self, Mode};
use crate::map_select::Catalogue;

// ---- the retail routines' parts ----

/// Clears the lobby's choices (the hopper, campaign, map and difficulty
/// globals at `0x54e7b8..0x54e7c8`) and, on the leader's console,
/// publishes the cleared choice. Every Switch To press and the playlist
/// press call it first. No arguments.
pub const LOBBY_CHOICES_CLEAR: u32 = 0x0019_9a57;
/// The custom game's setup: the lobby's multiplayer map from the profile
/// (`[0x51098c]`) through `LOBBY_SET_MAP`. Switch To: Custom Game's press
/// calls it after `LOBBY_MODE_SET(2)`. No arguments.
pub const CUSTOM_SETUP: u32 = 0x0019_a864;
/// Reads the profile's campaign difficulty (`[0x510998]`, held to 0..3)
/// into the dword at EAX.
pub const CAMPAIGN_DIFFICULTY_READ: u32 = 0x0012_1100;
/// Holds EAX as the lobby's campaign difficulty (`[0x54e7c8]`, held to
/// 0..3); EAX comes back as it went in, the value Switch To: Co-Op Game
/// gives `LOBBY_SET_DIFFICULTY`.
pub const LOBBY_DIFFICULTY_HOLD: u32 = 0x0014_8d42;
/// The byte Switch To: Co-Op Game clears after the difficulty.
pub const CO_OP_CLEARED: u32 = 0x0054_e7cc;
/// The byte that gates Switch To: Co-Op Game: set, the press raises error
/// `0x32` and changes nothing. Its meaning is not traced
/// (docs/live/live-coop.md §11.1); the switch refuses the same way.
pub const CO_OP_GATE: u32 = 0x0054_d5a0;
/// The playlist press's call after `LOBBY_MODE_SET(3)`. No arguments.
pub const MATCHMAKING_COMMIT: u32 = 0x0006_3fb0;
/// Holds EAX as the lobby's hopper (`[0x54e7b8]` set, `[0x54e7bc]` the
/// hopper); its EAX goes on to [`LOBBY_CHOICES_PUBLISH`].
pub const LOBBY_HOPPER_SET: u32 = 0x0014_8cfc;
/// Publishes the lobby's choices, EAX as the caller leaves it.
pub const LOBBY_CHOICES_PUBLISH: u32 = 0x0019_a0af;
/// The lobby's hopper: a byte set while one is held, then its index.
pub const LOBBY_HOPPER_HELD: u32 = 0x0054_e7b8;
pub const LOBBY_HOPPER: u32 = 0x0054_e7bc;
/// The hoppers the playlist list asks about (`HOPPER_LISTED`, 0..16).
pub const HOPPERS: u32 = 16;
/// The local player whose saved playlist MULTIPLAYER opens on, and who the
/// choice is saved for: the first, the party leader's own profile.
pub const LEADER_PLAYER: u32 = 0;

/// A guest dword of ours the difficulty read writes ([`install`]).
static SCRATCH: AtomicU32 = AtomicU32::new(0);

/// The page the switch's guest out-values go to (the party lobby feature,
/// `crate::lobby_mode`).
pub fn install(scratch: u32) {
    SCRATCH.store(scratch, Ordering::SeqCst);
}

// ---- the plan ----

/// What a switch to a mode sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    pub to: Tab,
    pub mode: Mode,
    /// The `firefight` setting held on for the lobby's games.
    pub firefight: bool,
    /// The retail lobby mode the switch sets, when it is not the lobby's.
    pub retail: Option<RetailMode>,
}

/// The switch from `from` to `to`.
pub fn plan(from: Tab, to: Tab) -> Plan {
    let parts = to.parts();
    Plan {
        to,
        mode: parts.mode,
        firefight: to == Tab::Firefight,
        retail: (from.parts().retail != parts.retail).then_some(parts.retail),
    }
}

/// The map `LOBBY_SET_MAP` puts the lobby on for `to` from its map
/// `current` (its list id), with `catalogue` the map list: `None` when the
/// lobby's own is inside the mode's category or the mode takes any map; or
/// why there is none. FIREFIGHT lands on the built-in kit maps first,
/// Coagulation's before Zanzibar's, then any kit map
/// ([`lobby_start::landing_map`]).
///
/// FIREFIGHT never refuses: with no map that carries the kit installed (or
/// the map list not read yet) the lobby keeps its map, and Start Game says
/// why it cannot start (`lobby_mode::start_refusal`); it lands on a kit map
/// when the built-in kits are built (`map_select`'s landing).
pub fn map_for(
    to: Tab,
    current: Option<u32>,
    catalogue: Option<&Catalogue>,
) -> Result<Option<u32>, Reason> {
    match to {
        Tab::Forge => Ok(None),
        Tab::Firefight => {
            let Some(c) = catalogue else { return Ok(None) };
            Ok(lobby_start::landing_map(c, current))
        }
        Tab::Multiplayer | Tab::CustomGames | Tab::CoOp => Ok(None),
    }
}

/// The playlist MULTIPLAYER opens on: `saved`, the one this profile last
/// played (`QUICKMATCH_HOPPER`, -1 for none), when it is `listed`, else the
/// first listed; `None` with none listed.
pub fn opening_hopper(saved: u32, listed: &[u32]) -> Option<u32> {
    if listed.contains(&saved) {
        Some(saved)
    } else {
        listed.first().copied()
    }
}

/// Why no switch now, from the lobby's start state (`LOBBY_START_STATE`):
/// the map loading (8) or the countdown running (10); and from whether a
/// matchmaking search has its lobby (`bots::lobby::matchmaking_cycle`).
pub fn locked(start_state: u32, searching: bool) -> Option<Reason> {
    if matches!(start_state, 8 | 10) {
        Some(Reason::Starting)
    } else if searching {
        Some(Reason::Searching)
    } else {
        None
    }
}

/// Sets our parts of `plan`: the `firefight` setting first (in `store`),
/// then the lobby's mode.
pub fn set_ours(plan: &Plan, store: Option<&Store>) -> Result<(), Reason> {
    if plan.firefight {
        let store = store.ok_or(Reason::NoSettings)?;
        store.set(ff::ENABLED.key, Value::Bool(true)).map_err(|e| {
            eprintln!("lobby tabs: {}: {e}", ff::ENABLED.key);
            Reason::NoSettings
        })?;
    }
    lobby_start::set_mode(plan.mode);
    Ok(())
}

// ---- what the playlists are known to be ----

static NO_PLAYLISTS: AtomicBool = AtomicBool::new(false);

/// Whether the last switch to MULTIPLAYER found no playlist listed: the mode
/// is then off with its reason until the party leaves the lobby.
pub fn no_playlists_seen() -> bool {
    NO_PLAYLISTS.load(Ordering::Relaxed)
}

/// The party left the lobby: the next one asks the server's playlists
/// afresh.
pub fn forget_playlists() {
    NO_PLAYLISTS.store(false, Ordering::Relaxed);
}

// ---- the routines ----

/// Switch To: Custom Game's press, less the dialog close (`0x2bba66`).
pub async fn to_custom(cx: &Cx) -> Result<(), EntryError> {
    cx.call(GuestCall::new(LOBBY_CHOICES_CLEAR)).await?;
    cx.call(GuestCall::new(points::LOBBY_MODE_SET.base).args([RetailMode::Custom.value()]))
        .await?;
    cx.call(GuestCall::new(CUSTOM_SETUP)).await?;
    Ok(())
}

/// Switch To: Co-Op Game's press after its gate, less the dialog close
/// (`0x2bba2a..0x2bba4b`), with `scratch` a guest dword for the
/// difficulty read.
pub async fn to_co_op(cx: &Cx, scratch: u32) -> Result<(), EntryError> {
    cx.call(GuestCall::new(LOBBY_CHOICES_CLEAR)).await?;
    cx.call(GuestCall::new(points::LOBBY_MODE_SET.base).args([RetailMode::CoOp.value()]))
        .await?;
    cx.call(GuestCall::new(CAMPAIGN_DIFFICULTY_READ).eax(scratch))
        .await?;
    let read = cx.read(|m| m.u32(scratch))?;
    let difficulty = cx
        .call(GuestCall::new(LOBBY_DIFFICULTY_HOLD).eax(read))
        .await?
        .eax;
    cx.write(|m| m.set_u8(CO_OP_CLEARED, 0))?;
    cx.call(GuestCall::new(points::LOBBY_SET_DIFFICULTY.base).eax(difficulty))
        .await?;
    Ok(())
}

/// The OptiMatch playlist press for `hopper` on the leader's console, less
/// the hopper block read (the hopper is a listed one) and the dialog close
/// (`0x2b21c2..0x2b21e3`): the mode, the hopper, and the choice saved for
/// [`LEADER_PLAYER`] (which `crate::clankermatch` skips for a Clanker
/// playlist).
pub async fn to_matchmaking(cx: &Cx, hopper: u32) -> Result<(), EntryError> {
    cx.call(GuestCall::new(LOBBY_CHOICES_CLEAR)).await?;
    cx.call(GuestCall::new(points::LOBBY_MODE_SET.base).args([RetailMode::Matchmaking.value()]))
        .await?;
    cx.call(GuestCall::new(MATCHMAKING_COMMIT)).await?;
    let held = cx
        .call(GuestCall::new(LOBBY_HOPPER_SET).eax(hopper))
        .await?
        .eax;
    cx.call(GuestCall::new(LOBBY_CHOICES_PUBLISH).eax(held))
        .await?;
    cx.call(GuestCall::new(points::PLAYLIST_REMEMBER.base).args([LEADER_PLAYER, hopper]))
        .await?;
    Ok(())
}

/// The hoppers the playlist screen would list on its one page, ordinary
/// and Clanker ones ([`Scope::All`] set first, so `crate::clankermatch`'s
/// filter lets each through to the game's own check).
pub async fn listed_hoppers(cx: &Cx) -> Result<Vec<u32>, EntryError> {
    clankermatch::set_scope(Scope::All);
    let mut out = Vec::new();
    for index in 0..HOPPERS {
        let listed = cx
            .call(GuestCall::new(points::HOPPER_LISTED.base).ecx(index))
            .await?
            .eax
            & 0xff
            != 0;
        if listed {
            out.push(index);
        }
    }
    Ok(out)
}

/// The playlist MULTIPLAYER opens on, or why none.
async fn opening(cx: &Cx) -> Result<Result<u32, Reason>, EntryError> {
    let listed = listed_hoppers(cx).await?;
    let saved = cx
        .call(GuestCall::new(points::QUICKMATCH_HOPPER.base).ecx(LEADER_PLAYER))
        .await?
        .eax;
    NO_PLAYLISTS.store(listed.is_empty(), Ordering::Relaxed);
    Ok(opening_hopper(saved, &listed).ok_or(Reason::NoPlaylists))
}

static IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Why a snapshot waits: a switch's guest calls are in flight.
pub fn in_flight() -> Option<String> {
    IN_FLIGHT
        .load(Ordering::SeqCst)
        .then(|| "a lobby mode switch's guest calls are in flight".to_owned())
}

/// What a switch to `to` does from the lobby `shown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    /// Nothing: a member's, or the lobby is in that mode already.
    Nothing,
    /// The mode is off here: its reason on the status line.
    Refuse(Reason),
    /// The switch from the lobby's mode.
    Switch { from: Tab, to: Tab, place: Place },
}

/// A switch to `to` asked for in the lobby `shown`: the leader's only, to
/// a mode present and on here.
pub fn ask(shown: &super::header::Shown, to: Tab) -> Ask {
    if !shown.leader || shown.selected == to {
        return Ask::Nothing;
    }
    match shown.avail(to) {
        None => Ask::Nothing,
        Some(super::Avail::Off(r)) => Ask::Refuse(r),
        Some(super::Avail::On) => Ask::Switch {
            from: shown.selected,
            to,
            place: shown.place,
        },
    }
}

/// The switch to `to` from the lobby as it stands now, with `present` the
/// modes present ([`ask`], then [`run`]): a refusal goes to the status
/// line.
pub async fn run_to(
    cx: &Cx,
    to: Tab,
    present: super::Present,
    store: Option<Arc<Store>>,
) -> Result<(), EntryError> {
    let read = |m: &dyn guest_layout::memory::GuestMemory| {
        super::header::read(m)
            .map(|r| super::header::shown(&r, present, crate::lobby_start::mode()))
    };
    let Some(shown) = cx.read(read) else {
        eprintln!("lobby tabs: a switch to {} with the lobby unreadable", to.label());
        return Ok(());
    };
    match ask(&shown, to) {
        Ask::Nothing => Ok(()),
        Ask::Refuse(r) => {
            notice(r);
            Ok(())
        }
        Ask::Switch { from, to, place } => run(cx, from, to, place, store).await,
    }
}

/// Runs the switch from `from` to `to` on the leader's console, in a lobby
/// at `place`.
pub async fn run(
    cx: &Cx,
    from: Tab,
    to: Tab,
    place: Place,
    store: Option<Arc<Store>>,
) -> Result<(), EntryError> {
    IN_FLIGHT.store(true, Ordering::SeqCst);
    let out = run_inner(cx, from, to, place, store).await;
    IN_FLIGHT.store(false, Ordering::SeqCst);
    match out {
        Ok(Err(r)) => {
            notice(r);
            Ok(())
        }
        Ok(Ok(())) => {
            eprintln!("lobby tabs: {} to {}", from.label(), to.label());
            Ok(())
        }
        Err(e) => Err(e),
    }
}

async fn run_inner(
    cx: &Cx,
    from: Tab,
    to: Tab,
    place: Place,
    store: Option<Arc<Store>>,
) -> Result<Result<(), Reason>, EntryError> {
    let state = cx
        .call(GuestCall::new(points::LOBBY_START_STATE.base).args([0]))
        .await?
        .eax;
    let searching = cx.read(|m| bots::lobby::matchmaking_cycle(m));
    if let Some(r) = locked(state, searching) {
        return Ok(Err(r));
    }
    let plan = plan(from, to);
    let catalogue = crate::map_select::current();
    let current = cx.read(|m| m.u32(crate::h1_options_row::LOBBY_MAP).ok());
    // Every refusal before anything changes. A mode of the custom lobby
    // entered from another mode is checked against its whole category: the
    // custom setup picks the map it lands on.
    let probe = if plan.retail.is_some() { None } else { current };
    if let Err(r) = map_for(to, probe, catalogue.as_deref()) {
        return Ok(Err(r));
    }
    if plan.retail == Some(RetailMode::CoOp) && cx.read(|m| m.u8(CO_OP_GATE))? != 0 {
        return Ok(Err(Reason::CoOpRefused));
    }
    let hopper = match plan.retail {
        Some(RetailMode::Matchmaking) => match opening(cx).await? {
            Ok(h) => Some(h),
            Err(r) => return Ok(Err(r)),
        },
        _ => None,
    };
    let before = lobby_start::mode();
    if let Err(r) = set_ours(&plan, store.as_deref()) {
        return Ok(Err(r));
    }
    if from == Tab::CoOp
        && let Some(lobby) = crate::coop_live::lobby()
    {
        lobby.leave();
    }
    match (plan.retail, hopper) {
        (Some(RetailMode::Custom), _) => to_custom(cx).await?,
        (Some(RetailMode::CoOp), _) => {
            if place == Place::XboxLive
                && let Some(lobby) = crate::coop_live::lobby()
            {
                lobby.host_retail();
            }
            to_co_op(cx, SCRATCH.load(Ordering::SeqCst)).await?
        }
        (Some(RetailMode::Matchmaking), Some(h)) => to_matchmaking(cx, h).await?,
        (Some(RetailMode::Matchmaking), None) | (None, _) => {}
    }
    let current = if plan.retail.is_some() {
        cx.read(|m| m.u32(crate::h1_options_row::LOBBY_MAP).ok())
    } else {
        current
    };
    let map = match map_for(to, current, catalogue.as_deref()) {
        Ok(m) => m,
        Err(r) => return Ok(Err(r)),
    };
    if let Some(map) = map {
        let set = cx
            .call(
                GuestCall::new(points::LOBBY_SET_MAP.base)
                    .esi(map)
                    .args([u32::MAX]),
            )
            .await?
            .eax as u8;
        if set == 0 {
            lobby_start::set_mode(before);
            return Ok(Err(Reason::MapRefused));
        }
    }
    Ok(Ok(()))
}

#[cfg(test)]
mod tests;
