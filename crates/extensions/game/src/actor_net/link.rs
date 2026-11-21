//! Tick-boundary adapter for the optional Live actor tunnel.

use std::{collections::BTreeMap, sync::Mutex};

use live_hle::{
    Hle,
    actor_net::{Budget, Packet, WireError},
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
