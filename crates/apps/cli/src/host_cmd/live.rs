use std::io::Read;
use std::net::{SocketAddr, SocketAddrV4};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use live_proto::hosts::{EndReason, HostEndpoint, HostState, PackUsage, Transport};
use serde::Serialize;

pub(super) struct Live<'a> {
    base: String,
    path: String,
    lease: String,
    token: &'a str,
    http: ureq::Agent,
}

#[derive(Serialize)]
struct Report<'a> {
    state: HostState,
    players: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a EndReason>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pack_usage: Option<PackUsage>,
}

impl<'a> Live<'a> {
    pub fn new(base: &str, lease: &str, token: &'a str) -> Result<Self> {
        let http = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(5))
            .redirects(0)
            .build();
        let parsed = http
            .post(base)
            .request_url()
            .ok()
            .context("invalid Live URL")?;
        let url = parsed.as_url();
        ensure!(
            matches!(url.scheme(), "http" | "https")
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "Live URL must be an HTTP(S) base URL without credentials, query or fragment"
        );
        ensure!(
            live_proto::bearer_allowed(url.as_str()),
            "the host credential requires HTTPS or a loopback Live URL"
        );
        Ok(Self {
            base: url.as_str().trim_end_matches('/').to_owned(),
            path: format!("/v1/hosts/{lease}"),
            lease: lease.to_owned(),
            token,
            http,
        })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn register(&self, endpoint: &HostEndpoint) -> Result<()> {
        self.post("/register", "registering endpoint", endpoint)
    }

    pub fn playing(&self, players: u8) -> Result<()> {
        self.post(
            "/report",
            "reporting playing",
            &Report {
                state: HostState::Playing,
                players,
                reason: None,
                pack_usage: None,
            },
        )
    }

    pub fn ended(&self, reason: &EndReason, players_peak: u8, pack: Option<SocketAddr>) -> Result<()> {
        self.post(
            "/report",
            "reporting ended",
            &Report {
                state: HostState::Ended,
                players: players_peak,
                reason: Some(reason),
                pack_usage: pack.and_then(|control| self.pack_usage(control)),
            },
        )
    }

    fn pack_usage(&self, control: SocketAddr) -> Option<PackUsage> {
        let read = || -> Result<PackUsage> {
            let path = live_proto::host_pack::MATCHES_PATH;
            let url = format!("http://{control}{path}/{}/usage", self.lease);
            let response = self.http.get(&url).call()?;
            Ok(serde_json::from_reader(response.into_reader().take(4096))?)
        };
        match read() {
            Ok(usage) => Some(usage),
            Err(error) => {
                eprintln!("h2 host {}: pack usage unavailable: {error}", self.lease);
                None
            }
        }
    }

    pub fn endpoint(
        &self,
        transport: Transport,
        bind: SocketAddrV4,
        public: Option<SocketAddrV4>,
        session_id: Option<String>,
    ) -> HostEndpoint {
        let udp = (transport == Transport::Udp).then(|| public.unwrap_or(bind).to_string());
        let tunnel_url = (transport == Transport::Tunnel).then(|| {
            let (scheme, rest) = self.base.split_once("://").expect("validated Live URL");
            let ws = if scheme == "https" { "wss" } else { "ws" };
            format!("{ws}://{rest}{}/tunnel", self.path)
        });
        HostEndpoint {
            transport,
            udp,
            tunnel_url,
            xnaddr: xnaddr(bind),
            session_id,
        }
    }

    fn post(&self, suffix: &str, operation: &str, body: &impl Serialize) -> Result<()> {
        let bytes = serde_json::to_vec(body)?;
        let response = self
            .http
            .post(&format!("{}{}{suffix}", self.base, self.path))
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_bytes(&bytes);
        checked(operation, response)?;
        Ok(())
    }
}

fn checked(
    operation: &str,
    response: Result<ureq::Response, ureq::Error>,
) -> Result<ureq::Response> {
    match response {
        Ok(response) if (200..300).contains(&response.status()) => Ok(response),
        Ok(response) => bail!("{operation}: Live returned HTTP {}", response.status()),
        Err(ureq::Error::Status(code, _)) => bail!("{operation}: Live returned HTTP {code}"),
        Err(ureq::Error::Transport(error)) => bail!("{operation}: {error}"),
    }
}

/// Match the HLE's accountless XNADDR for the configured H2_XNET_ADDR
/// (live-hle/src/network.rs). The public UDP route can differ from it.
fn xnaddr(bind: SocketAddrV4) -> String {
    let ip = bind.ip().octets();
    let port = bind.port().to_be_bytes();
    let mut bytes = [0; 36];
    bytes[..4].copy_from_slice(&ip);
    bytes[4..8].copy_from_slice(&ip);
    bytes[8..10].copy_from_slice(&port);
    bytes[10] = 2;
    bytes[11] = ip[0] ^ ip[1];
    bytes[12..14].copy_from_slice(&ip[2..4]);
    bytes[14..16].copy_from_slice(&port);
    live_proto::sign::hex(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn endpoint_keeps_advertised_xnaddr_and_public_udp_route_separate() {
        let live = Live::new("http://127.0.0.1:8074", "hl-a", "test-token").unwrap();
        let bind = "192.168.1.2:31000".parse().unwrap();
        let public = "203.0.113.8:31020".parse().unwrap();
        let endpoint = live.endpoint(Transport::Udp, bind, Some(public), Some("abcd".into()));
        assert_eq!(endpoint.udp.as_deref(), Some("203.0.113.8:31020"));
        assert_eq!(endpoint.tunnel_url, None);
        assert_eq!(endpoint.session_id.as_deref(), Some("abcd"));
        assert_eq!(
            endpoint.xnaddr,
            "c0a80102c0a8010279180268010279180000000000000000000000000000000000000000"
        );
        let local = live.endpoint(Transport::Udp, bind, None, None);
        assert_eq!(local.udp.as_deref(), Some("192.168.1.2:31000"));
    }

    #[test]
    fn tunnel_endpoint_preserves_live_base_and_uses_websocket_scheme() {
        let bind = "192.168.1.2:31000".parse().unwrap();
        for (live, expected) in [
            (
                "http://localhost:8074/",
                "ws://localhost:8074/v1/hosts/hl-a/tunnel",
            ),
            (
                "https://live.example/service/",
                "wss://live.example/service/v1/hosts/hl-a/tunnel",
            ),
        ] {
            let live = Live::new(live, "hl-a", "test-token").unwrap();
            let endpoint = live.endpoint(Transport::Tunnel, bind, None, None);
            assert_eq!(endpoint.udp, None);
            assert_eq!(endpoint.tunnel_url.as_deref(), Some(expected));
        }
    }

    #[test]
    fn reports_match_live_wire_format() {
        let playing = Report {
            state: HostState::Playing,
            players: 4,
            reason: None,
            pack_usage: None,
        };
        assert_eq!(
            serde_json::to_value(playing).unwrap(),
            json!({"state":"playing","players":4})
        );
        let ended = Report {
            state: HostState::Ended,
            players: 5,
            reason: Some(&EndReason::MatchOver),
            pack_usage: None,
        };
        assert_eq!(
            serde_json::to_value(ended).unwrap(),
            json!({"state":"ended","players":5,"reason":"match_over"})
        );
    }
}
