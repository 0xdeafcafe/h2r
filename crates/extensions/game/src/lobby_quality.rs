//! Connection bars from the game's measured peer RTT, shared by the
//! scoreboard badge and the agent's pregame roster read.

use agent_proto::{RpcError, code};
use bots::{lobby, phantom};
use guest_layout::{GuestMemory, view::players};
use serde_json::{Value, json};

use phantom::session::{self, at::*};

const MAX_BARS: u8 = 6;

/// The physical network channels and their reliable packet queues (allocated
/// together at `0x817ef`, `0x81889` and `0x818a2`).
const TRANSPORT_COUNT: u32 = 0x004d_87d0;
const TRANSPORT_CHANNELS: u32 = 0x004d_87d4;
const RELIABLE_QUEUES: u32 = 0x004d_87d8;

/// Retail's RTT component of the scoreboard calculation at `0x68a90`.
/// Thresholds and bar count come from the initialization at `0x67950`.
fn rtt_bars(ms: u32) -> u8 {
    let fraction = (800 - ms.clamp(60, 800)) as f32 / (800 - 60) as f32;
    1 + (fraction * f32::from(MAX_BARS - 1)).round_ties_even() as u8
}

#[derive(Clone, Copy)]
struct Quality {
    bars: u8,
    rtt_ms: Option<u32>,
}

struct Membership {
    addr: u32,
    peers: u32,
    local: u32,
}

impl Membership {
    fn read(m: &dyn GuestMemory, addr: u32) -> Option<Self> {
        if addr == 0
            || addr.checked_add(STATE + 4).is_none()
            || !lobby::lobby_state(m.u32(addr + STATE).ok()?)
        {
            return None;
        }
        let peers = m.u32(addr + PEER_COUNT).ok()?;
        let local = m.u32(addr + LOCAL_PEER).ok()?;
        (local < peers && peers <= session::MAX).then_some(Self {
            addr,
            peers,
            local,
        })
    }

    fn quality(&self, m: &dyn GuestMemory, peer: u32) -> Option<Quality> {
        if peer >= self.peers {
            return None;
        }
        let mut machine = [0; 6];
        m.read(self.addr + PEERS + peer * PEER_SIZE + PEER_MAC, &mut machine)
            .ok()?;
        if peer == self.local || phantom::handle_of(machine).is_some() {
            return Some(Quality {
                bars: MAX_BARS,
                rtt_ms: None,
            });
        }
        let rtt_ms = peer_rtt(m, self.addr, peer);
        Some(Quality {
            bars: rtt_ms.map_or(1, rtt_bars),
            rtt_ms,
        })
    }
}

/// A session channel points into its network observer (`0x5a121`).
/// Follow the observer to the reliable channel as `0x767c9` does, then
/// read its last acknowledged round trip. Its timestamp distinguishes a
/// real zero-ms sample from an empty queue or the smoothed RTT's seed.
fn peer_rtt(m: &dyn GuestMemory, session: u32, peer: u32) -> Option<u32> {
    let channel = session + CHANNELS + peer * CHANNEL_SIZE;
    if m.u8(channel).ok()? == 0 || m.u8(channel + 1).ok()? == 0 {
        return None;
    }
    let index = m.u32(channel + 4).ok()?;
    let observer = m.u32(session + 8).ok()?;
    if index >= 15 || observer == 0 {
        return None;
    }
    let record = observer.checked_add(0xa8 + index * 0x528)?;
    if m.u32(record).ok()? == 0 {
        return None;
    }
    let count = m.u32(TRANSPORT_COUNT).ok()?.min(15);
    let index = m.u32(record.checked_add(0xc)?).ok()?;
    let channels = m.u32(TRANSPORT_CHANNELS).ok()?;
    if index >= count || channels == 0 {
        return None;
    }
    let channel = channels.checked_add(index * 0xf8)?;
    if m.u32(channel.checked_add(0x54)?).ok()? != 5 || m.u8(channel + 0x48).ok()? & 8 == 0 {
        return None;
    }
    let index = m.u32(channel + 0x10).ok()?;
    let queues = m.u32(RELIABLE_QUEUES).ok()?;
    if index >= count || queues == 0 {
        return None;
    }
    let queue = queues.checked_add(index * 0x97c)?;
    let rtt = queue.checked_add(0x960)?;
    if m.u32(queue + 0x95c).ok()? == 0 {
        return None;
    }
    m.u32(rtt).ok()
}

/// The active pregame membership, in slot order. A matchmaking search
/// starts with the party and displays the target session once it exists.
pub(crate) fn inspect(m: &dyn GuestMemory) -> Result<Value, RpcError> {
    let rows = Vec::<Value>::new();
    if !m.u8(lobby::NETWORKING).is_ok_and(|on| on != 0)
        || !m.u8(lobby::LOBBY_ON).is_ok_and(|on| on != 0)
    {
        return Ok(json!({ "players": rows }));
    }
    let ty = m.u32(lobby::SESSION_TYPE).unwrap_or_default();
    if !matches!(ty, 1 | 2 | 6 | 7) {
        return Ok(json!({ "players": rows }));
    }
    let membership = m
        .u32(lobby::game_session_at(ty))
        .ok()
        .and_then(|addr| Membership::read(m, addr))
        .or_else(|| {
            let addr = m.u32(lobby::SESSION).ok()?;
            Membership::read(m, addr)
        });
    if let Some(s) = membership { return membership_rows(m, &s); }
    Ok(json!({ "players": rows }))
}

fn membership_rows(m: &dyn GuestMemory, s: &Membership) -> Result<Value, RpcError> {
    let mut rows = Vec::new();
    let unavailable = || RpcError::new(code::UNAVAILABLE, "lobby membership is unreadable");
    let mask = m.u32(s.addr + PLAYER_MASK).map_err(|_| unavailable())?;
    for slot in (0..session::MAX).filter(|slot| mask & (1 << slot) != 0) {
        let at = s.addr + PLAYERS + slot * PLAYER_SIZE;
        let peer = m.u32(at + P_PEER).map_err(|_| unavailable())?;
        let mut raw = [0; CONFIG_NAME_UNITS * 2];
        m.read(at + P_EFFECTIVE, &mut raw).map_err(|_| unavailable())?;
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|&u| u != 0)
            .collect();
        let name = String::from_utf16_lossy(&units);
        let quality = s.quality(m, peer).ok_or_else(unavailable)?;
        rows.push(json!({
            "slot": slot, "peer": peer, "name": name,
            "bars": quality.bars, "rtt_ms": quality.rtt_ms
        }));
    }
    Ok(json!({ "players": rows }))
}

/// Per-player reliable RTT from the active simulation membership. No peer
/// addresses are exposed; an unsampled route stays null.
pub(crate) fn match_peer_rtts(m: &dyn GuestMemory) -> Result<Value, RpcError> {
    let session = m.u32(MACHINES).ok().filter(|&p| p != 0)
        .and_then(|p| p.checked_add(0xc))
        .and_then(|p| m.u32(p).ok()).and_then(|p| Membership::read(m, p))
        .ok_or_else(|| RpcError::new(code::UNAVAILABLE, "no active simulation membership"))?;
    membership_rows(m, &session)
}

fn scoreboard_bars(m: &dyn GuestMemory, stats: u32) -> Option<u8> {
    // `0x160837` passes this player's 16-byte stats record in EBX.
    let globals = m.u32(guest_layout::points::GAME_ENGINE.base).ok()?;
    if globals == 0 {
        return None;
    }
    let offset = stats.checked_sub(globals.checked_add(0x6dc)?)?;
    if offset % 16 != 0 || offset / 16 >= session::MAX {
        return None;
    }
    let player = players::array(m)
        .ok()??
        .get_absolute(m, (offset / 16) as i32)
        .ok()??;
    let machine = player.machine(m).ok()?;

    // The scoreboard's visibility check at `0x68aa7` uses the simulation
    // machine set's session. Use the same session to match the row's MAC.
    let machines = m.u32(MACHINES).ok().filter(|&addr| addr != 0)?;
    let machines = machines.checked_add(0xc)?;
    let s = Membership::read(m, m.u32(machines).ok()?)?;
    let peer = session::find_peer(m, s.addr, machine).ok()??;
    s.quality(m, peer).map(|q| q.bars)
}

pub(crate) struct ConnectionBars;

impl hooks::Feature for ConnectionBars {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("connection-bars", "Scoreboard connection bars from measured RTT")
            .doc("docs/game/bots.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.raw()
            .after(
                &guest_layout::points::SCOREBOARD_CONNECTION_BARS,
                "fill the visible badge from the player's peer",
                |c| {
                    if c.cpu.eax & 0xff != 0 {
                        let mut m = c.ctx.memory();
                        if let Some(bars) = scoreboard_bars(&*m, c.cpu.ebx) {
                            // Retail preserves EBX and pops the out pointer
                            // with ret 4. The popped word is still on the stack.
                            let out = m.u32(c.cpu.esp.wrapping_sub(4))?;
                            m.set_u32(out, u32::from(bars))?;
                        }
                    }
                    Ok(())
                },
            )
            .context_only();
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn rtt_bars_follow_retail_latency_boundaries() {
        for (ms, bars) in [
            (0, 6), (60, 6), (133, 6), (134, 5), (282, 5), (283, 4),
            (429, 4), (430, 3), (578, 3), (579, 2), (725, 2), (726, 1),
            (800, 1), (u32::MAX, 1),
        ] {
            assert_eq!(rtt_bars(ms), bars, "RTT {ms} ms");
        }
    }

    #[test]
    fn scoreboard_looks_up_the_peer_by_the_player_datums_machine() {
        use guest_layout::datum::layout as datum;

        const ENGINE: u32 = 0x0060_0000;
        const HEADER: u32 = 0x0061_0000;
        const DATA: u32 = 0x0062_0000;
        const SET: u32 = 0x0063_0000;
        const S: u32 = 0x0064_0000;
        const STRIDE: u32 = 0x21c;
        let machine = phantom::machine_id(phantom::PhantomHandle(3));
        let mut m = FakeGuest::new();
        m.put_u32(guest_layout::points::GAME_ENGINE.base, ENGINE)
            .put_u32(guest_layout::points::PLAYERS.base, HEADER)
            .put_u32(HEADER + datum::DATA, DATA)
            .put_u32(HEADER + datum::HIGH_WATER, 6)
            .put_u32(HEADER + datum::ELEMENT_SIZE, STRIDE)
            .put_u16(DATA + 5 * STRIDE, 1)
            .put(DATA + 5 * STRIDE + players::MACHINE, &machine)
            .put_u32(MACHINES, SET)
            .put_u32(SET + 0xc, S)
            .map(S, 0x7900)
            .put_u32(S + STATE, session::STATE_LIVE)
            .put_u32(S + PEER_COUNT, 2)
            .put(S + PEERS + PEER_SIZE + PEER_MAC, &machine);
        let stats = ENGINE + 0x6dc + 5 * 16;
        assert_eq!(scoreboard_bars(&m, stats), Some(6));
        assert_eq!(scoreboard_bars(&m, stats + 1), None);
    }

    #[test]
    fn match_roster_returns_local_phantom_remote_and_unsampled_bars() {
        const S: u32 = 0x0060_0000;
        const PARTY: u32 = 0x0061_0000;
        const OBSERVER: u32 = 0x0062_0000;
        const CHANNELS_BASE: u32 = 0x0063_0000;
        const QUEUES_BASE: u32 = 0x0064_0000;
        let mut m = FakeGuest::new();
        m.put_u8(lobby::NETWORKING, 1)
            .put_u8(lobby::LOBBY_ON, 1)
            .put_u32(lobby::SESSION_TYPE, 7)
            .put_u32(lobby::SESSION, PARTY)
            .put_u32(lobby::TARGET_SESSION, S)
            .map(S, 0x7900)
            .put_u32(S + STATE, session::STATE_LIVE)
            .put_u32(S + PEER_COUNT, 4)
            .put_u32(S + LOCAL_PEER, 1)
            .put_u32(S + PLAYER_MASK, 0b1_0111)
            .put_u32(S + 8, OBSERVER)
            .put(
                S + PEERS + 2 * PEER_SIZE + PEER_MAC,
                &phantom::machine_id(phantom::PhantomHandle(1)),
            )
            .put_u8(S + CHANNELS, 1)
            .put_u8(S + CHANNELS + 1, 1)
            .put_u32(S + CHANNELS + 4, 2);
        let record = OBSERVER + 0xa8 + 2 * 0x528;
        let channel = CHANNELS_BASE + 0xf8;
        let queue = QUEUES_BASE + 2 * 0x97c;
        m.put_u32(record, 1)
            .put_u32(record + 0xc, 1)
            .put_u32(TRANSPORT_COUNT, 3)
            .put_u32(TRANSPORT_CHANNELS, CHANNELS_BASE)
            .put_u32(RELIABLE_QUEUES, QUEUES_BASE)
            .put_u32(channel + 0x54, 5)
            .put_u8(channel + 0x48, 8)
            .put_u32(channel + 0x10, 2)
            .put_u32(queue + 0x95c, 1000)
            .put_u32(queue + 0x960, 350);
        for (slot, peer, name) in [
            (0, 1, "Local"), (1, 2, "Clanker"), (2, 0, "Remote"), (4, 3, "Waiting"),
        ] {
            let player = S + PLAYERS + slot * PLAYER_SIZE;
            let name: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
            m.put_u32(player + P_PEER, peer).put(player + P_EFFECTIVE, &name);
        }
        assert_eq!(inspect(&m).unwrap(), json!({ "players": [
            {"slot": 0, "peer": 1, "name": "Local", "bars": 6, "rtt_ms": null},
            {"slot": 1, "peer": 2, "name": "Clanker", "bars": 6, "rtt_ms": null},
            {"slot": 2, "peer": 0, "name": "Remote", "bars": 4, "rtt_ms": 350},
            {"slot": 4, "peer": 3, "name": "Waiting", "bars": 1, "rtt_ms": null}
        ] }));
        m.put_u32(MACHINES, 0x0068_0000).put_u32(0x0068_000c, S);
        assert_eq!(match_peer_rtts(&m).unwrap(), inspect(&m).unwrap());
        m.put_u32(queue + 0x960, 0);
        assert_eq!(peer_rtt(&m, S, 0), Some(0));
        m.put_u32(queue + 0x95c, 0);
        assert_eq!(peer_rtt(&m, S, 0), None);
        m.put_u32(lobby::SESSION_TYPE, 8);
        assert_eq!(inspect(&m).unwrap(), json!({ "players": [] }));
    }
}
