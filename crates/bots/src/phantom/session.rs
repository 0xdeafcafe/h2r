//! A phantom's machine and player, written into the host's session the way
//! the game's own join path writes them (docs/game/bots.md §5.1), and its
//! actions written into its simulation slot. Pure memory work on
//! [`GuestMemory`], so the tests run on synthetic sessions.
//!
//! The game does the rest from the session: each frame it rebuilds the
//! machine set from the peers (`0x83f00`), diffs the session players
//! (`0x83610`) and creates the player and its simulation slot (`0x85140`).

use engine::memory::{GuestMemory, MemFault};
use engine::points;

use crate::guest;

pub type R<T> = Result<T, MemFault>;

/// Session offsets (`*SQUAD_SESSION`).
pub mod at {
    pub const UPDATE: u32 = 0x4c;
    pub const PEER_COUNT: u32 = 0x54;
    pub const PEERS: u32 = 0x58;
    pub const PEER_SIZE: u32 = 0x10c;
    pub const PLAYER_COUNT: u32 = 0x1118;
    pub const PLAYER_MASK: u32 = 0x111c;
    pub const PLAYERS: u32 = 0x1120;
    pub const PLAYER_SIZE: u32 = 0x13c;
    /// Must be 4 for the player diff (`0x83610`).
    pub const CLASS: u32 = 0x4980;
    pub const MAX_PEERS: u32 = 0x4990;
    pub const MAX_PLAYERS: u32 = 0x4994;
    pub const CHANNELS: u32 = 0x72dc;
    pub const CHANNEL_SIZE: u32 = 0x14;
    pub const STATE: u32 = 0x741c;
    pub const CHANGES: u32 = 0x7618;
    /// Peer indices the removal (`0x5fe20`) moves down.
    pub const LOCAL_PEER: u32 = 0x40;
    pub const PEER_50: u32 = 0x50;
    pub const HOST_PEER: u32 = 0x72d8;

    /// In a peer.
    pub const PEER_MAC: u32 = 0xa;
    pub const PEER_NAME: u32 = 0x28;
    pub const PEER_NAME_UNITS: u32 = 15;
    pub const PEER_PLAYERS: u32 = 0xf8;
    pub const PEER_SLOTS: u32 = 0xfc;

    /// In a session player.
    pub const P_ID: u32 = 0;
    pub const P_PEER: u32 = 0xc;
    pub const P_USER: u32 = 0x10;
    pub const P_CONTROLLER: u32 = 0x14;
    pub const P_REQUESTED: u32 = 0x18;
    pub const P_EFFECTIVE: u32 = 0xa8;
    pub const P_138: u32 = 0x138;

    /// The simulation globals (`*0x4cf77c`): mode, and the player slots.
    pub const SIM_GLOBALS: u32 = 0x004c_f77c;
    pub const SIM_MODE: u32 = 8;
    pub const SIM_SLOTS: u32 = 0x7c;
    pub const SLOT_SIZE: u32 = 0x88;
    pub const SLOT_MAC: u32 = 0x18;
    pub const SLOT_STAMP: u32 = 0x28;
    pub const SLOT_ACTION: u32 = 0x2c;
    pub const ACTION_SIZE: usize = 0x5c;

    /// Player datum `+0xd4`: a copy of its configuration.
    pub const PLAYER_CONFIG: u32 = 0xd4;
    pub const CONFIG_SIZE: usize = 0x90;
    pub const CONFIG_TEAM: usize = 0x7c;
    pub const CONFIG_NAME_UNITS: usize = 16;
    /// Unit `+0x212`: the current weapon index.
    pub const UNIT_WEAPON: u32 = 0x212;
}

use at::*;

/// The lobby state, which a running game keeps too (states 6 to 8 tear the
/// simulation down; docs/game/bots.md §5.1).
pub const STATE_LIVE: u32 = 5;
/// The distributed server's simulation mode.
pub const MODE_DIST_SERVER: u32 = 5;
/// The local simulation mode (Split Screen and campaign alone).
pub const MODE_LOCAL: u32 = 1;
pub const MAX: u32 = 16;

/// Why the host cannot take a phantom now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotNow {
    /// No game running: wait (phantoms join only once a game runs).
    Wait,
    /// A game runs, but this console is not (or not yet) its host, and
    /// what the check read. A phantom waits and tries again: in the first
    /// frames of a game, and in a game this console joined, this is so.
    NotHost(String),
    /// No room for another peer or player.
    Full,
}

/// The host's session, when a phantom can join now: a multiplayer game
/// running, this console its distributed server, the session live.
pub fn host_session(m: &dyn GuestMemory) -> Result<u32, NotNow> {
    let running = guest::game_tick(m).is_some() && multiplayer(m);
    if !running {
        return Err(NotNow::Wait);
    }
    let read = |a: u32| m.u32(a).ok();
    let ready = m.u8(points::NETWORK_READY.base).ok().unwrap_or(0) != 0;
    let s = read(points::SQUAD_SESSION.base).unwrap_or(0);
    let sim = read(SIM_GLOBALS).unwrap_or(0);
    if !ready || s == 0 || sim == 0 {
        // A game with no network session: a local game, no host.
        return Err(NotNow::NotHost(format!(
            "no network session (network ready {ready}, session {s:#x}, simulation {sim:#x})"
        )));
    }
    let shown = |v: Option<u32>| v.map_or_else(|| "?".to_owned(), |v| v.to_string());
    let mode = read(sim + SIM_MODE);
    if mode == Some(MODE_LOCAL) {
        // Split Screen: its session is of the local type (session `+0x18`
        // 0, made by `0x59890(0, …)` from the players screen in mode 1),
        // which `0x64060` gives protocol 0, and `0x6ec80` simulation mode
        // 1. No game of it has peers; a hosted System Link or Live game is
        // distributed (protocol 2 for every shipped engine), one console
        // or many.
        return Err(NotNow::NotHost(
            "a local game (simulation mode 1, Split Screen): phantoms join System Link and Xbox Live games only"
                .into(),
        ));
    }
    if mode != Some(MODE_DIST_SERVER) {
        return Err(NotNow::NotHost(format!(
            "the simulation is not the distributed server (mode {}, the server's is {MODE_DIST_SERVER})",
            shown(mode)
        )));
    }
    let state = read(s + STATE);
    if state != Some(STATE_LIVE) {
        return Err(NotNow::NotHost(format!(
            "the session is not live (state {}, live is {STATE_LIVE})",
            shown(state)
        )));
    }
    let class = read(s + CLASS);
    if class != Some(4) {
        return Err(NotNow::NotHost(format!(
            "the session's class is {}, not 4",
            shown(class)
        )));
    }
    Ok(s)
}

/// The live network session, game or lobby: the session whose peers a
/// game's end takes phantoms out of.
pub fn live_session(m: &dyn GuestMemory) -> Option<u32> {
    if m.u8(points::NETWORK_READY.base).ok()? == 0 {
        return None;
    }
    let s = m.u32(points::SQUAD_SESSION.base).ok().filter(|&s| s != 0)?;
    (m.u32(s + STATE).ok()? == STATE_LIVE).then_some(s)
}

fn multiplayer(m: &dyn GuestMemory) -> bool {
    let Ok(g) = m.u32(points::GAME_GLOBALS.base) else {
        return false;
    };
    g != 0 && m.u32(g + guest::OPTIONS).ok() == Some(guest::MODE_MULTIPLAYER)
}

fn bump(m: &mut dyn GuestMemory, s: u32) -> R<()> {
    let u = m.u32(s + UPDATE)?;
    m.set_u32(s + UPDATE, u.wrapping_add(1))?;
    let c = m.u32(s + CHANGES)?;
    m.set_u32(s + CHANGES, c.wrapping_add(1))
}

fn peer(s: u32, p: u32) -> u32 {
    s + PEERS + p * PEER_SIZE
}

fn player(s: u32, slot: u32) -> u32 {
    s + PLAYERS + slot * PLAYER_SIZE
}

fn channel(s: u32, p: u32) -> u32 {
    s + CHANNELS + p * CHANNEL_SIZE
}

fn zero(m: &mut dyn GuestMemory, a: u32, n: u32) -> R<()> {
    m.write(a, &vec![0; n as usize])
}

fn mac_at(m: &dyn GuestMemory, a: u32) -> R<[u8; 6]> {
    let mut id = [0; 6];
    m.read(a, &mut id)?;
    Ok(id)
}

/// The peer with machine id `mac`.
pub fn find_peer(m: &dyn GuestMemory, s: u32, mac: [u8; 6]) -> R<Option<u32>> {
    let n = m.u32(s + PEER_COUNT)?.min(MAX);
    for p in 0..n {
        if mac_at(m, peer(s, p) + PEER_MAC)? == mac {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// A phantom's 12-byte session player id: its xuid, then its machine id's
/// last four bytes (its handle).
pub fn player_id(xuid: u64, mac: [u8; 6]) -> [u8; 12] {
    let mut id = [0; 12];
    id[..8].copy_from_slice(&xuid.to_le_bytes());
    id[8..].copy_from_slice(&mac[2..]);
    id
}

/// A phantom's configuration: the host's own player's (`+0xd4` of the
/// first local player) with the phantom's name, and its team when it has
/// one (else the host's: balancing waits for the lobby teams).
pub fn config(m: &dyn GuestMemory, name: &str, team: Option<u8>) -> R<Option<[u8; CONFIG_SIZE]>> {
    let Some(array) = engine::view::players::array(m)? else {
        return Ok(None);
    };
    for (_, p) in array.live(m, 16)? {
        if p.listed(m)? && p.local_user(m)? >= 0 {
            let mut c = [0; CONFIG_SIZE];
            m.read(p.addr + PLAYER_CONFIG, &mut c)?;
            set_name(&mut c, name);
            if let Some(t) = team {
                c[CONFIG_TEAM] = t;
            }
            return Ok(Some(c));
        }
    }
    Ok(None)
}

fn set_name(c: &mut [u8; CONFIG_SIZE], name: &str) {
    c[..CONFIG_NAME_UNITS * 2].fill(0);
    for (i, u) in name.encode_utf16().take(CONFIG_NAME_UNITS - 1).enumerate() {
        c[2 * i..2 * i + 2].copy_from_slice(&u.to_le_bytes());
    }
}

/// Adds a peer for machine `mac`, unconnected and with no channel, as
/// `0x5fbd0(S, n, &xnaddr, 0, -1, NULL)` does, and bumps the membership
/// counters as the host's join (`0x5c720`) does. Returns its index.
pub fn add_peer(m: &mut dyn GuestMemory, s: u32, mac: [u8; 6], name: &str) -> R<Option<u32>> {
    let n = m.u32(s + PEER_COUNT)?;
    if n >= MAX || n >= m.u32(s + MAX_PEERS)? {
        return Ok(None);
    }
    m.set_u32(s + PEER_COUNT, n + 1)?;
    let a = peer(s, n);
    zero(m, a, PEER_SIZE)?;
    // The XNADDR: only its MAC (the machine id) is ours to set.
    m.write(a + PEER_MAC, &mac)?;
    for (i, u) in (0u32..).zip(name.encode_utf16().take(PEER_NAME_UNITS as usize)) {
        m.set_u16(a + PEER_NAME + 2 * i, u)?;
    }
    for u in 0..4 {
        m.set_u32(a + PEER_SLOTS + 4 * u, u32::MAX)?;
    }
    let c = channel(s, n);
    zero(m, c, CHANNEL_SIZE)?;
    m.set_u8(c, 1)?; // valid; +1 connected stays 0
    m.set_u32(c + 4, u32::MAX)?; // no channel
    m.set_u32(c + 8, u32::MAX)?;
    m.set_u32(c + 0xc, u32::MAX)?;
    bump(m, s)?;
    Ok(Some(n))
}

/// Adds user 0 of peer `p` as a session player, as `0x60000`/`0x600f0`
/// and the add-player message (`0x5f05d`) do: controller 0, the
/// configuration both requested and effective. Returns its slot.
pub fn add_player(
    m: &mut dyn GuestMemory,
    s: u32,
    p: u32,
    id: [u8; 12],
    config: &[u8; CONFIG_SIZE],
) -> R<Option<u32>> {
    let mask = m.u32(s + PLAYER_MASK)?;
    let count = m.u32(s + PLAYER_COUNT)?;
    if count >= m.u32(s + MAX_PLAYERS)? {
        return Ok(None);
    }
    for slot in (0..MAX).filter(|i| mask & 1 << i != 0) {
        let mut other = [0; 12];
        m.read(player(s, slot) + P_ID, &mut other)?;
        if other == id {
            return Ok(None);
        }
    }
    let Some(slot) = (0..MAX).find(|i| mask & 1 << i == 0) else {
        return Ok(None);
    };
    let a = player(s, slot);
    m.write(a + P_ID, &id)?;
    m.set_u32(a + P_PEER, p)?;
    m.set_u32(a + P_USER, 0)?;
    m.set_u32(a + P_CONTROLLER, 0)?;
    m.write(a + P_REQUESTED, config)?;
    m.write(a + P_EFFECTIVE, config)?;
    m.set_u32(a + P_138, 0)?;
    let pa = peer(s, p);
    m.set_u32(pa + PEER_SLOTS, slot)?;
    let on = m.u32(pa + PEER_PLAYERS)?;
    m.set_u32(pa + PEER_PLAYERS, on + 1)?;
    m.set_u32(s + PLAYER_MASK, mask | 1 << slot)?;
    m.set_u32(s + PLAYER_COUNT, count + 1)?;
    bump(m, s)?;
    Ok(Some(slot))
}

/// Takes peer `p` and its players out, as `0x5fe20` does: the players go
/// (`0x60200`), later players' peer indices and the session's peer
/// indices move down, and the peers and channels above `p` move down one.
pub fn remove_peer(m: &mut dyn GuestMemory, s: u32, p: u32) -> R<()> {
    let n = m.u32(s + PEER_COUNT)?.min(MAX);
    if p >= n {
        return Ok(());
    }
    let pa = peer(s, p);
    for u in 0..4 {
        let slot = m.u32(pa + PEER_SLOTS + 4 * u)?;
        if slot >= MAX {
            continue;
        }
        m.set_u32(pa + PEER_SLOTS + 4 * u, u32::MAX)?;
        let on = m.u32(pa + PEER_PLAYERS)?;
        m.set_u32(pa + PEER_PLAYERS, on.saturating_sub(1))?;
        let mask = m.u32(s + PLAYER_MASK)?;
        m.set_u32(s + PLAYER_MASK, mask & !(1 << slot))?;
        let count = m.u32(s + PLAYER_COUNT)?;
        m.set_u32(s + PLAYER_COUNT, count.saturating_sub(1))?;
        bump(m, s)?;
    }
    let mask = m.u32(s + PLAYER_MASK)?;
    for slot in (0..MAX).filter(|i| mask & 1 << i != 0) {
        let a = player(s, slot) + P_PEER;
        let q = m.u32(a)?;
        if q > p && q < MAX {
            m.set_u32(a, q - 1)?;
        }
    }
    if m.u32(s + PEER_50)? == p {
        m.set_u32(s + PEER_50, 0)?;
    }
    for (src, size) in [(PEERS, PEER_SIZE), (CHANNELS, CHANNEL_SIZE)] {
        let tail = (n - p - 1) * size;
        if tail > 0 {
            let mut buf = vec![0; tail as usize];
            m.read(s + src + (p + 1) * size, &mut buf)?;
            m.write(s + src + p * size, &buf)?;
        }
        zero(m, s + src + (n - 1) * size, size)?;
    }
    for f in [LOCAL_PEER, HOST_PEER, PEER_50] {
        let q = m.u32(s + f)?;
        if q > p && q < MAX {
            m.set_u32(s + f, q - 1)?;
        }
    }
    m.set_u32(s + PEER_COUNT, n - 1)?;
    bump(m, s)
}

/// The simulation slot of machine `mac`'s player, once the game made it.
pub fn slot_of(m: &dyn GuestMemory, mac: [u8; 6]) -> R<Option<u32>> {
    let sim = m.u32(SIM_GLOBALS)?;
    if sim == 0 {
        return Ok(None);
    }
    for i in 0..MAX {
        let a = sim + SIM_SLOTS + i * SLOT_SIZE;
        if m.u32(a)? != u32::MAX && mac_at(m, a + SLOT_MAC)? == mac {
            return Ok(Some(a));
        }
    }
    Ok(None)
}

/// One tick's `0x5c` action record (docs/game/bots.md §5.1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Action {
    pub flags: u32,
    /// Absolute world angles, radians: yaw in [0, 2π).
    pub yaw: f32,
    pub pitch: f32,
    /// -1 to 1.
    pub forward: f32,
    pub strafe: f32,
    /// 0 to 1: fire, and the second trigger (grenades).
    pub trigger: f32,
    pub trigger2: f32,
    /// The current weapon index, -1 for none.
    pub weapon: i8,
}

impl Action {
    /// The record's bytes, every field the validator (`0x14d0a0`) checks in
    /// range: the second weapon, grenade and `+0x24` -1, the target and aim
    /// block zero.
    pub fn bytes(&self) -> [u8; ACTION_SIZE] {
        let mut b = [0u8; ACTION_SIZE];
        let f = |b: &mut [u8; ACTION_SIZE], o: usize, v: f32| {
            b[o..o + 4].copy_from_slice(&v.to_le_bytes())
        };
        b[0..4].copy_from_slice(&self.flags.to_le_bytes());
        f(&mut b, 0x4, self.yaw.rem_euclid(std::f32::consts::TAU));
        f(&mut b, 0x8, self.pitch.clamp(-1.5, 1.5));
        f(&mut b, 0xc, self.forward.clamp(-1.0, 1.0));
        f(&mut b, 0x10, self.strafe.clamp(-1.0, 1.0));
        f(&mut b, 0x14, self.trigger.clamp(0.0, 1.0));
        f(&mut b, 0x18, self.trigger2.clamp(0.0, 1.0));
        b[0x20] = if (0..4).contains(&self.weapon) {
            self.weapon as u8
        } else {
            0xff
        };
        b[0x21] = 0xff;
        b[0x22..0x24].copy_from_slice(&u16::MAX.to_le_bytes());
        b[0x24..0x26].copy_from_slice(&u16::MAX.to_le_bytes());
        b
    }
}

/// Writes `action` into the slot at `slot`, stamped with this tick, as the
/// server's path of `0x847d0` stores it (without its rounding).
pub fn write_action(m: &mut dyn GuestMemory, slot: u32, action: &Action) -> R<()> {
    let Some(tick) = guest::game_tick(m) else {
        return Ok(());
    };
    m.write(slot + SLOT_ACTION, &action.bytes())?;
    m.set_u32(slot + SLOT_STAMP, tick)
}

/// Turn rate at full right stick, radians a second (\[guess\]: about the
/// game's at the bots' look sensitivity).
pub const TURN_RATE: f32 = 3.0;

/// A pad as an action, for a player facing `aim` (a unit vector) with
/// weapon `weapon`, over `dt` seconds. The right stick turns from the
/// current facing as the brain expects (+x turns right, +y looks up); the
/// left stick is (right, forward); the right trigger fires and the left
/// throws. Buttons are not mapped yet (jump, crouch, melee: §5.1).
pub fn action_from_pad(
    pad: &input::host::HostPadState,
    aim: [f32; 3],
    weapon: i8,
    dt: f32,
) -> Action {
    let (yaw, pitch) = (aim[1].atan2(aim[0]), aim[2].atan2(aim[0].hypot(aim[1])));
    let (rx, ry) = pad.right_stick;
    let (lx, ly) = pad.left_stick;
    Action {
        flags: 0,
        yaw: yaw - rx * TURN_RATE * dt,
        pitch: pitch + ry * TURN_RATE * dt,
        forward: ly,
        // [guess] the action's strafe is positive to the left.
        strafe: -lx,
        trigger: pad.right_trigger,
        trigger2: pad.left_trigger,
        weapon,
    }
}

/// Player `p`'s unit's facing and current weapon, if it has a unit.
pub fn facing(m: &dyn GuestMemory, player_addr: u32) -> R<Option<([f32; 3], i8)>> {
    let p = engine::view::players::Player::at(player_addr);
    let Some(unit) = p.unit(m)? else {
        return Ok(None);
    };
    let Some((_, body)) = engine::view::objects::get(m, unit)? else {
        return Ok(None);
    };
    let aim = m.vec3(body.addr + guest::UNIT_AIM)?;
    let weapon = m.u8(body.addr + UNIT_WEAPON)? as i8;
    Ok(Some((aim, weapon)))
}

#[cfg(test)]
mod tests;
