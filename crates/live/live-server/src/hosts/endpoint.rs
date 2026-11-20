//! Only advertise a joinable host once the client can decode its endpoint.

use axum::http::Uri;
use live_proto::hosts::{HostEndpoint, Transport};
use std::net::SocketAddr;

use super::{ApiResult, invalid};

pub(super) fn validate(endpoint: &HostEndpoint, expected: &Transport) -> ApiResult<()> {
    if &endpoint.transport != expected {
        return Err(invalid("endpoint transport differs from the lease"));
    }
    // XNet's XNADDR is 36 bytes, encoded as two hex digits per byte.
    if endpoint.xnaddr.len() != 72 || !endpoint.xnaddr.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid("endpoint XNADDR must be 36 hex-encoded bytes"));
    }
    let reachable = match expected {
        Transport::Udp => endpoint
            .udp
            .as_deref()
            .and_then(|s| s.parse::<SocketAddr>().ok())
            .is_some_and(|addr| {
                (1..=u16::MAX - 6).contains(&addr.port())
                    && !addr.ip().is_unspecified()
                    && !addr.ip().is_multicast()
                    && !matches!(addr, SocketAddr::V4(v4) if v4.ip().is_broadcast())
            }),
        Transport::Tunnel => endpoint
            .tunnel_url
            .as_deref()
            .and_then(|s| s.parse::<Uri>().ok())
            .is_some_and(|uri| {
                uri.host().is_some_and(|host| !host.is_empty())
                    && uri.scheme_str().is_some_and(|s| {
                        s.eq_ignore_ascii_case("wss") || s.eq_ignore_ascii_case("ws")
                    })
            }),
    };
    if !reachable {
        return Err(invalid(
            "endpoint does not supply a usable address for the leased transport",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn udp(addr: &str) -> HostEndpoint {
        HostEndpoint {
            transport: Transport::Udp,
            udp: Some(addr.into()),
            tunnel_url: None,
            xnaddr: "ab".repeat(36),
            session_id: None,
        }
    }

    #[test]
    fn udp_needs_a_unicast_address_and_room_for_seven_ports() {
        assert!(validate(&udp("127.0.0.1:65529"), &Transport::Udp).is_ok());
        for addr in [
            "0.0.0.0:31000",
            "224.0.0.1:31000",
            "255.255.255.255:31000",
            "127.0.0.1:0",
            "127.0.0.1:65530",
            "127.0.0.1",
        ] {
            assert!(validate(&udp(addr), &Transport::Udp).is_err(), "{addr}");
        }
    }

    #[test]
    fn registration_checks_xnaddr_and_the_reserved_transport() {
        let mut endpoint = udp("192.0.2.10:31000");
        assert!(validate(&endpoint, &Transport::Tunnel).is_err());
        for text in ["ab".repeat(35), "ab".repeat(37), "zz".repeat(36)] {
            endpoint.xnaddr = text;
            assert!(validate(&endpoint, &Transport::Udp).is_err());
        }
    }

    #[test]
    fn tunnel_needs_an_absolute_websocket_url() {
        let mut endpoint = HostEndpoint {
            transport: Transport::Tunnel,
            udp: None,
            ..udp("127.0.0.1:31000")
        };
        for (url, valid) in [
            ("wss://live.example/v1/hosts/hl-test/tunnel", true),
            ("ws://127.0.0.1:8074/tunnel", true),
            ("https://live.example/tunnel", false),
            ("wss://", false),
            ("/tunnel", false),
        ] {
            endpoint.tunnel_url = Some(url.into());
            assert_eq!(
                validate(&endpoint, &Transport::Tunnel).is_ok(),
                valid,
                "{url}"
            );
        }
    }
}
