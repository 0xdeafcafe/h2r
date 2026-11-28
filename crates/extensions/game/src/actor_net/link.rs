//! Tick-boundary adapter for the optional Live actor tunnel.

use std::{collections::BTreeMap, sync::Mutex};

use guest_layout::GuestMemory;
use live_hle::{
    Hle,
    actor_net::{Budget, Packet, Stats, WireError},
};

#[derive(Default)]
pub struct HostLink {
    budgets: Mutex<BTreeMap<u32, Budget>>,
}

impl HostLink {
    /// Call after capture. Each connection gets its own byte window and the
    /// scheduler is called on quiet ticks to advance keys and dialogue copies.
    pub fn post_tick(
        &self,
        hle: Option<&Hle>,
        tick: u32,
        mut schedule: impl FnMut(u32, &mut Budget, bool) -> Option<Packet>,
    ) -> Result<(), WireError> {
        let Some(hle) = hle else {
            self.reset();
            return Ok(());
        };
        let peers = hle.actor_tunnel_peers(tick);
        let mut budgets = self.budgets.lock().unwrap();
        budgets.retain(|peer, _| peers.iter().any(|(id, _)| id == peer));
        for (peer, request_key) in peers {
            if let Some(packet) = schedule(peer, budgets.entry(peer).or_default(), request_key) {
                hle.send_actor_tunnel_packet(peer, &packet)?;
            }
        }
        Ok(())
    }

    /// Each connected peer's budget counters and its bytes in the last
    /// 30 ticks, for `actors.net`.
    pub fn peers(&self) -> Vec<(u32, Stats, usize)> {
        self.budgets
            .lock()
            .unwrap()
            .iter()
            .map(|(&peer, budget)| (peer, budget.stats(), budget.window_bytes()))
            .collect()
    }

    pub fn reset(&self) {
        self.budgets.lock().unwrap().clear();
    }
}

/// Drain the inbox before the game tick. The puppet layer owns tick ordering
/// and applies accepted packets during the tick, including any waiting records.
pub fn pre_tick(hle: Option<&Hle>, mut receive: impl FnMut(Packet)) {
    if let Some(hle) = hle {
        for packet in hle.drain_actor_tunnel_packets() {
            receive(packet);
        }
    }
}

pub fn request_key(hle: Option<&Hle>) -> bool {
    hle.is_some_and(Hle::request_actor_tunnel_key)
}

/// The upstream pick messages the tunnel received since the last call,
/// each with the roster slots of the remote players whose machine id the
/// sending peer named (`crate::actor_net::pick::seats_of`), for `crate::actor_net::pick::receive`.
/// A peer that names a machine no player has gets no seats, and its picks
/// are refused there.
pub fn take_picks(hle: Option<&Hle>, m: &dyn GuestMemory) -> Vec<(Vec<u8>, Vec<u8>)> {
    let Some(hle) = hle else {
        return Vec::new();
    };
    hle.take_actor_tunnel_picks()
        .into_iter()
        .map(|(_, machine, message)| (message, crate::actor_net::pick::seats_of(m, machine)))
        .collect()
}

/// Sends one upstream pick message on the tunnel's client-to-host
/// `ActorPick` frame, behind this machine's id; whether it was queued.
/// False without a negotiated channel, before the client has read its own
/// machine id, or with the send queue full.
pub fn send_pick(hle: Option<&Hle>, pick: &[u8]) -> bool {
    let Some(machine) = crate::actor_net::pick::local_machine() else {
        return false;
    };
    let mut payload = machine.to_vec();
    payload.extend_from_slice(pick);
    hle.is_some_and(|hle| hle.send_actor_tunnel_pick(payload))
}

/// Whether the tunnel carries the upstream pick frame: it is up and
/// negotiated the actor channel. A client asks its local players only
/// while it does, so nobody is offered a pick that cannot travel.
pub fn pick_upstream(hle: Option<&Hle>) -> bool {
    hle.is_some_and(Hle::actor_tunnel_upstream)
}
