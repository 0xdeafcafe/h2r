//! Select the dedicated transport after normal XNADDR/IN_ADDR translation.
//! Keep XNet's bound host sockets as the receive queues so select, FIONREAD,
//! blocking reads, MSG_PEEK and truncation see the same datagrams.

use super::{Network, Result, Socket, host, lock};
use crate::{
    Hle, TunnelHandle,
    tunnel_client::{channel_destination, port_offset},
};
use std::{net::SocketAddrV4, sync::Arc};

pub(super) struct Route {
    advertised: SocketAddrV4,
    handle: TunnelHandle,
}

impl Drop for Route {
    fn drop(&mut self) {
        self.handle.stop();
    }
}

#[derive(Clone)]
pub(super) struct Connected {
    destination: SocketAddrV4,
    handle: TunnelHandle,
}

impl Hle {
    /// Installs the tunnel for the selected host's advertised XNADDR endpoint
    /// on this runtime. The join worker may probe handle.qos_base() beforehand.
    pub fn install_tunnel(
        &self,
        advertised: SocketAddrV4,
        handle: TunnelHandle,
    ) -> std::result::Result<(), String> {
        crate::NetworkConfig::new(advertised).validate()?;
        if handle.closed() {
            return Err("dedicated host tunnel is closed".into());
        }
        let state = Arc::downgrade(&self.network.state);
        let config = self.network.config.clone();
        let injector = handle.injector();
        handle.on_datagram(Box::new(move |udp, port, payload| {
            let Some(state) = state.upgrade() else {
                return;
            };
            let state = lock(&state);
            // A retired connection cannot inject into the next match.
            if state
                .tunnel
                .as_ref()
                .is_none_or(|t| t.handle.injector() != injector)
            {
                return;
            }
            for socket in state.sockets.values().filter(|s| s.host.datagram) {
                let Some(bound) = *lock(&socket.bound) else {
                    continue;
                };
                if let Some(dest) = channel_destination(&config, bound, port) {
                    let _ = udp.send_to(payload, dest);
                    break;
                }
            }
        }));
        let mut state = lock(&self.network.state);
        if let Some(current) = &mut state.tunnel
            && current.handle.same_connection(&handle)
        {
            current.advertised = advertised;
        } else {
            state.tunnel = Some(Route { advertised, handle });
        }
        Ok(())
    }

    /// Removes the dedicated route when leaving the match.
    pub fn clear_tunnel(&self) {
        lock(&self.network.state).tunnel = None;
    }

    /// Deliver complete actor packets at the next client tick boundary.
    /// Sessions without an actor-capable tunnel have an empty inbox.
    pub fn drain_actor_tunnel_packets(&self) -> Vec<actor_net::Packet> {
        lock(&self.network.state)
            .tunnel
            .as_ref()
            .map_or_else(Vec::new, |route| route.handle.drain_actor_packets())
    }

    /// Ask the current tunnel host for a fresh actor key, if negotiated.
    pub fn request_actor_tunnel_key(&self) -> bool {
        lock(&self.network.state)
            .tunnel
            .as_ref()
            .is_some_and(|route| route.handle.request_actor_key())
    }
}

impl Network {
    pub(super) fn qos_target(&self, base: SocketAddrV4) -> SocketAddrV4 {
        lock(&self.state)
            .tunnel
            .as_ref()
            .filter(|route| base == route.advertised)
            .map_or(base, |route| route.handle.qos_base())
    }

    pub(super) fn tunnel_source(&self, sender: SocketAddrV4, offset: u16) -> SocketAddrV4 {
        let state = lock(&self.state);
        match &state.tunnel {
            Some(route) if sender == route.handle.injector() => {
                SocketAddrV4::new(*route.advertised.ip(), route.advertised.port() + offset)
            }
            _ => sender,
        }
    }

    fn tunnel_destination(&self, dest: SocketAddrV4) -> Option<(TunnelHandle, u16)> {
        let state = lock(&self.state);
        let route = state.tunnel.as_ref()?;
        let offset = port_offset(route.advertised, dest)?;
        Some((route.handle.clone(), offset))
    }

    pub(super) fn connect_transport(&self, socket: &Socket, dest: SocketAddrV4) -> Result<()> {
        if !socket.host.datagram {
            return host(socket.host.connect(dest));
        }
        let tunnel = self.tunnel_destination(dest).map(|(handle, _)| Connected {
            destination: dest,
            handle,
        });
        let mut connected = lock(&socket.tunnel_connection);
        host(
            socket
                .host
                .connect(tunnel.as_ref().map_or(dest, |t| t.handle.injector())),
        )?;
        *connected = tunnel;
        Ok(())
    }

    pub(super) fn send_transport(
        &self,
        socket: &Socket,
        data: &[u8],
        dest: Option<SocketAddrV4>,
        flags: i32,
    ) -> Result<usize> {
        if socket.host.datagram {
            let connection = if dest.is_none() {
                lock(&socket.tunnel_connection).clone()
            } else {
                None
            };
            if let Some(target) = dest.or(connection.as_ref().map(|c| c.destination))
                && let Some((handle, offset)) = self.tunnel_destination(target)
            {
                if socket
                    .send_shutdown
                    .load(std::sync::atomic::Ordering::Acquire)
                {
                    return Err(10058); // WSAESHUTDOWN
                }
                if flags & 1 != 0 {
                    return Err(10045); // UDP has no out-of-band data.
                }
                if connection
                    .as_ref()
                    .is_some_and(|c| !c.handle.same_connection(&handle))
                {
                    return Err(10057); // Connected socket belongs to an earlier match.
                }
                return handle.send(offset, data);
            }
            if connection.is_some() {
                return Err(10057);
            }
        }
        host(socket.host.send(data, dest, flags))
    }
}
