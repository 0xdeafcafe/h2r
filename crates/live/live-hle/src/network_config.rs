use std::net::{Ipv4Addr, SocketAddrV4};

/// The co-op channel's port, counted from the base (`coop::link::COOP_PORT_OFFSET`).
pub const COOP_PORT_OFFSET: u16 = 4;

/// Host endpoints for the title's four channels. Guest ports 1000, 1001, 1005
/// and 1006 map to base+0, +1, +5 and +6. Base+2 is the HLE QoS responder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkConfig {
    pub address: SocketAddrV4,
    /// Empty uses LAN broadcast. Otherwise broadcasts fan out to these bases
    /// on this same host IP (use its LAN IPv4 with the actual title).
    pub loopback_peers: Vec<SocketAddrV4>,
}
impl NetworkConfig {
    pub fn new(address: SocketAddrV4) -> Self {
        Self {
            address,
            loopback_peers: Vec::new(),
        }
    }
    /// Boot alone reads these variables. Direct Hle constructors are explicit.
    pub fn from_env() -> Result<Self, String> {
        Self::from_values(
            std::env::var("H2_XNET_ADDR").ok().as_deref(),
            std::env::var("H2_XNET_LOOPBACK_PEERS").ok().as_deref(),
        )
    }
    pub fn from_values(address: Option<&str>, peers: Option<&str>) -> Result<Self, String> {
        let mut c = Self::new(match address {
            Some(s) => s.parse().map_err(|_| "H2_XNET_ADDR must be IPv4:port")?,
            None => crate::config::lan_address(),
        });
        if let Some(peers) = peers {
            for p in peers.split(',').filter(|p| !p.is_empty()) {
                c.loopback_peers.push(
                    p.parse()
                        .map_err(|_| "H2_XNET_LOOPBACK_PEERS must contain IPv4:port endpoints")?,
                );
            }
        }
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.address.port() == 0
            || self.address.port() > 65529
            || self.address.ip().is_unspecified()
            || self.address.ip().is_broadcast()
            || self.address.ip().is_multicast()
        {
            return Err("XNet address must be a unicast IPv4 and base port 1..65529".into());
        }
        if !self.loopback_peers.is_empty() {
            if self.loopback_peers.len() > 16 {
                return Err("loopback discovery supports at most 16 peers".into());
            }
            let mut endpoints = self.loopback_peers.clone();
            endpoints.push(self.address);
            for (i, a) in endpoints.iter().enumerate() {
                if a.ip() != self.address.ip() || a.port() == 0 || a.port() > 65529 {
                    return Err(
                        "loopback peers must use this host IPv4 and base ports 1..65529".into(),
                    );
                }
                if endpoints[..i]
                    .iter()
                    .any(|b| a.ip() == b.ip() && a.port().abs_diff(b.port()) <= 6 && a != b)
                {
                    return Err("loopback base ports on one IP must be at least 7 apart".into());
                }
            }
        }
        Ok(())
    }
    /// The host endpoint of the co-op channel (docs/live/live-coop.md): base+4,
    /// which no title channel and not the QoS responder uses, inside the
    /// base..base+6 range a System Link host already opens. The co-op
    /// transport binds it itself (`coop::link::UdpLink`); the guest never
    /// sees it.
    pub fn coop_endpoint(&self) -> SocketAddrV4 {
        SocketAddrV4::new(*self.address.ip(), self.address.port() + COOP_PORT_OFFSET)
    }
    pub(crate) fn local(&self, guest: SocketAddrV4) -> SocketAddrV4 {
        if (1000..=1006).contains(&guest.port()) {
            SocketAddrV4::new(
                if guest.ip().is_unspecified() && !self.loopback_peers.is_empty() {
                    *self.address.ip()
                } else {
                    *guest.ip()
                },
                self.address.port() + (guest.port() - 1000),
            )
        } else {
            guest
        }
    }
    /// The IN_ADDR a peer on this console's own IP is known by, made from
    /// its XNet base port: 169.254.(port >> 8).(port & 255), a link-local
    /// address no peer of a game has. Each game on one Mac has its own base
    /// (`h2 coop test`: 31000, 31010, 31020), so each gets its own.
    pub fn shared_ip_in_addr(base: u16) -> Ipv4Addr {
        let [hi, lo] = base.to_be_bytes();
        Ipv4Addr::new(169, 254, hi, lo)
    }
    pub(crate) fn channel_port(base: u16, guest: u16) -> u16 {
        if (1000..=1006).contains(&guest) {
            base + (guest - 1000)
        } else {
            guest
        }
    }
    pub(crate) fn broadcasts(&self, port: u16) -> Vec<SocketAddrV4> {
        if self.loopback_peers.is_empty() {
            vec![SocketAddrV4::new(
                Ipv4Addr::BROADCAST,
                Self::channel_port(self.address.port(), port),
            )]
        } else {
            self.loopback_peers
                .iter()
                .filter(|p| **p != self.address)
                .map(|p| SocketAddrV4::new(*p.ip(), Self::channel_port(p.port(), port)))
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_coop_channel_is_a_free_port_inside_the_xnet_range() {
        let c = NetworkConfig::new("192.168.1.50:31000".parse().unwrap());
        assert_eq!(c.coop_endpoint(), "192.168.1.50:31004".parse().unwrap());
        // Guest ports 1000, 1001, 1005 and 1006 are the title's; +2 is QoS.
        let used: Vec<u16> = [1000, 1001, 1005, 1006]
            .into_iter()
            .map(|g| NetworkConfig::channel_port(31000, g))
            .chain([31002])
            .collect();
        assert!(!used.contains(&c.coop_endpoint().port()));
        assert!(c.coop_endpoint().port() <= 31006);
    }
}
