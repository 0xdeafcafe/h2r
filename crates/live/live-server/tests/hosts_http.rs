//! Dedicated host HTTP contract through the in-memory router harness.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, StatusCode, header};
use common::{Resp, harness, start};
use live_proto::{Gamertag, lxm};
use live_server::{Policy, store::MemoryStore};

const DETAIL: &str = "/v1/hosts/hl-AAAAAAAAAAAAAAAAAAAAAA";
const REGISTER: &str = "/v1/hosts/hl-AAAAAAAAAAAAAAAAAAAAAA/register";
const REPORT: &str = "/v1/hosts/hl-AAAAAAAAAAAAAAAAAAAAAA/report";
#[cfg(feature = "native")]
const UNKNOWN_LEASE: &str = "hl-AAAAAAAAAAAAAAAAAAAAAA";
const INVENTORY: &str = "/v1/admin/hosts";
const STOP: &str = "/v1/admin/hosts/hl-AAAAAAAAAAAAAAAAAAAAAA/stop";

const ENDPOINT: &str = r#"{
    "transport": "udp",
    "udp": "192.0.2.12:31000",
    "xnaddr": "c000020cc000020c18790000000000000000000000000000000000000000000000000000"
}"#;
const PLAYING: &str = r#"{"state":"playing","players":2}"#;

fn assert_unauthorized(response: &Resp) {
    assert_eq!(response.status, StatusCode::UNAUTHORIZED);
    assert_eq!(response.error(), "unauthorized");
    assert_eq!(response.headers[header::WWW_AUTHENTICATE], "Bearer");
}

#[tokio::test]
async fn host_routes_require_bearers_on_a_public_server() {
    let h = harness().await;
    for (method, uri, body) in [
        (Method::POST, REGISTER, ENDPOINT),
        (Method::POST, REPORT, PLAYING),
        (Method::GET, DETAIL, ""),
        (Method::GET, INVENTORY, ""),
        (Method::POST, INVENTORY, "{"),
        (Method::POST, STOP, ""),
    ] {
        assert_unauthorized(&h.call(method, uri, &[], body).await);
    }
}

#[tokio::test]
async fn host_writes_reject_account_tokens_with_host_scopes() {
    let h = harness().await;
    let player = h.create("Player").await;
    for (uri, body, scope) in [
        (REGISTER, ENDPOINT, "hosts.register"),
        (REPORT, PLAYING, "hosts.report"),
    ] {
        let response = h
            .call(Method::POST, uri, &[h.bearer(&player, scope)], body)
            .await;
        match response.status {
            StatusCode::UNAUTHORIZED => assert_unauthorized(&response),
            StatusCode::FORBIDDEN => assert_eq!(response.error(), "forbidden"),
            status => panic!("{uri} returned {status} for an account token"),
        }
    }
}

#[tokio::test]
async fn host_admin_routes_require_the_matching_scope_and_an_admin() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            admins: vec![Gamertag::new("Boss").unwrap()],
            ..Policy::default()
        },
    )
    .await;
    let admin = h.create("Boss").await;
    let player = h.create("Player").await;

    for (method, uri, scope, wrong_scope) in [
        (Method::GET, INVENTORY, "admin.read", lxm::READ),
        (Method::POST, INVENTORY, "admin.write", "admin.read"),
        (Method::POST, STOP, "admin.write", "admin.read"),
    ] {
        assert_unauthorized(
            &h.call(
                method.clone(),
                uri,
                &[h.bearer(&admin, wrong_scope)],
                Body::empty(),
            )
            .await,
        );
        let denied = h
            .call(method, uri, &[h.bearer(&player, scope)], Body::empty())
            .await;
        assert_eq!(denied.status, StatusCode::FORBIDDEN);
        assert_eq!(denied.error(), "forbidden");
    }

    let inventory = h
        .call(
            Method::GET,
            INVENTORY,
            &[h.bearer(&admin, "admin.read")],
            Body::empty(),
        )
        .await;
    assert_eq!(inventory.status, StatusCode::OK);
    assert_eq!(inventory.headers[header::CACHE_CONTROL], "private, no-store");
}

#[tokio::test]
async fn admin_host_start_checks_auth_before_reading_or_parsing_the_body() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            admins: vec![Gamertag::new("Boss").unwrap()],
            ..Policy::default()
        },
    )
    .await;
    let admin = h.create("Boss").await;
    for (body, status) in [
        (vec![b'{'], StatusCode::BAD_REQUEST),
        (vec![b' '; 1025], StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        assert_unauthorized(&h.call(Method::POST, INVENTORY, &[], body.clone()).await);
        let response = h
            .call(Method::POST, INVENTORY, &[h.bearer(&admin, "admin.write")], body)
            .await;
        assert_eq!(response.status, status);
    }
}

#[tokio::test]
async fn host_request_bodies_reject_invalid_shapes() {
    let h = harness().await;
    for (uri, body) in [
        (REGISTER, "{"),
        (REGISTER, r#"{"transport":"udp"}"#),
        (REPORT, r#"{"state":"joinable","players":2}"#),
        (REPORT, r#"{"state":"playing","players":-1}"#),
        (REPORT, r#"{"state":"playing"}"#),
        (REPORT, r#"{"state":"ended","players":0,"reason":"unknown"}"#),
    ] {
        let response = h.call(Method::POST, uri, &[], body).await;
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{uri} {body}");
        assert_eq!(response.error(), "invalid");
    }
}

#[tokio::test]
async fn host_request_body_caps_are_route_specific() {
    let h = harness().await;
    for (uri, body, max) in [(REGISTER, ENDPOINT, 8 * 1024), (REPORT, PLAYING, 1024)] {
        let mut bytes = body.as_bytes().to_vec();
        bytes.resize(max, b' ');
        assert_unauthorized(&h.call(Method::POST, uri, &[], bytes.clone()).await);

        bytes.push(b' ');
        let response = h.call(Method::POST, uri, &[], bytes).await;
        assert_eq!(response.status, StatusCode::PAYLOAD_TOO_LARGE, "{uri}");
        assert_eq!(response.error(), "too_large");
    }
}

#[tokio::test]
async fn host_reads_share_the_api_read_limit() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            read_rate: (0.0, 1),
            ..Policy::default()
        },
    )
    .await;
    assert_unauthorized(&h.get(DETAIL).await);
    let response = h.get(INVENTORY).await;
    assert_eq!(response.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(response.error(), "rate_limited");
}

#[cfg(feature = "native")]
mod tunnel {
    use super::*;
    use std::net::SocketAddr;
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use live_proto::hosts::{HostClass, HostEndpoint, HostLease, ProviderKind, Transport};
    use live_proto::tunnel::{Frame, Kind, decode, encode};
    use live_server::hosts::{HostBoot, HostConfig, HostError, HostProvider};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::time::timeout;
    use tokio_tungstenite::tungstenite::{
        Error, Message,
        client::IntoClientRequest,
        handshake::server::{Request, Response},
    };
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

    type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

    struct Provider(SocketAddr);

    #[async_trait::async_trait]
    impl HostProvider for Provider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::Local
        }

        async fn start(&self, lease: &HostLease, _: &HostBoot) -> Result<String, HostError> {
            Ok(lease.id.clone())
        }

        async fn stop(&self, _: &str, _: &str) -> Result<(), HostError> {
            Ok(())
        }

        async fn alive(&self, _: &str) -> Result<bool, HostError> {
            Ok(true)
        }

        fn tunnel_addr(&self, lease_id: &str, provider_ref: &str) -> Option<SocketAddr> {
            assert_eq!(lease_id, provider_ref);
            Some(self.0)
        }
    }

    async fn serve() -> (common::Harness, SocketAddr) {
        let h = harness().await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(axum::serve(listener, h.app.clone()).into_future());
        (h, addr)
    }

    async fn upgrade(addr: SocketAddr, lease: &str, token: Option<&str>) -> Result<Socket, Error> {
        let mut request = format!("ws://{addr}/v1/hosts/{lease}/tunnel").into_client_request()?;
        request
            .headers_mut()
            .insert("x-h2-tunnel-peer", "spoofed".parse().unwrap());
        if let Some(token) = token {
            request.headers_mut().insert(
                header::AUTHORIZATION,
                format!("Bearer {token}").parse().unwrap(),
            );
        }
        timeout(
            Duration::from_secs(5),
            tokio_tungstenite::connect_async(request),
        )
        .await
        .expect("tunnel upgrade timed out")
        .map(|(socket, _)| socket)
    }

    fn refused(result: Result<Socket, Error>, status: StatusCode) {
        match result {
            Err(Error::Http(response)) => assert_eq!(response.status(), status),
            Err(err) => panic!("expected HTTP {status}, got {err}"),
            Ok(_) => panic!("expected HTTP {status}, got a tunnel"),
        }
    }

    #[tokio::test]
    async fn unknown_tunnel_lease_is_only_disclosed_to_an_authenticated_account() {
        let (h, addr) = serve().await;
        let member = h.create("TunnelPlayer").await;
        let wrong_scope = h.token(&member.key, &member.author, lxm::READ);
        let relay = h.token(&member.key, &member.author, lxm::SESSIONS_RELAY);
        for token in [None, Some(wrong_scope.as_str())] {
            refused(
                upgrade(addr, UNKNOWN_LEASE, token).await,
                StatusCode::UNAUTHORIZED,
            );
        }
        refused(
            upgrade(addr, UNKNOWN_LEASE, Some(&relay)).await,
            StatusCode::NOT_FOUND,
        );
    }

    #[tokio::test]
    async fn only_the_roster_can_relay_datagrams_to_and_from_the_local_host() {
        let (h, addr) = serve().await;
        let member = h.create("TunnelMember").await;
        let stranger = h.create("TunnelStranger").await;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        h.live
            .set_host_provider(Arc::new(Provider(listener.local_addr().unwrap())));
        h.live.set_host_config(HostConfig {
            live_url: format!("http://{addr}"),
            data_url: "https://private.test/host-data.tar.zst".into(),
            data_sha256: "ab".repeat(32),
            archive: "dual".into(),
            max_hosts: 1,
        });
        let lease = h
            .live
            .request_host(HostLease {
                id: String::new(),
                assignment_id: "custom-tunnel".into(),
                provider: ProviderKind::Local,
                region: None,
                class: HostClass::Small,
                map: "lockout".into(),
                map_id: None,
                variant: None,
                hopper: None,
                max_players: 2,
                roster: vec![member.xuid],
                bots: vec![],
                transport: Transport::Tunnel,
                idle_timeout_s: 120,
                max_match_s: 1800,
                created_ms: 0,
            })
            .await
            .unwrap();
        let boot_token = h.live.issue_host_token(&lease);
        h.live
            .host_register(
                Some(&boot_token),
                &lease.id,
                HostEndpoint {
                    transport: Transport::Tunnel,
                    udp: None,
                    tunnel_url: Some(format!("ws://{addr}/v1/hosts/{}/tunnel", lease.id)),
                    xnaddr: "00".repeat(36),
                    session_id: None,
                },
            )
            .await
            .unwrap();
        let token = h.token(&stranger.key, &stranger.author, lxm::SESSIONS_RELAY);
        refused(
            upgrade(addr, &lease.id, Some(&token)).await,
            StatusCode::FORBIDDEN,
        );

        let hello = Frame {
            kind: Kind::Hello,
            port: 0,
            peer: 0,
            payload: vec![],
        };
        let reply = Frame {
            kind: Kind::Hello,
            port: 0,
            peer: 42,
            payload: vec![],
        };
        let expect_hello = hello.clone();
        let send_reply = reply.clone();
        let peer = member.xuid.to_string();
        let listener_task = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut socket =
                tokio_tungstenite::accept_hdr_async(tcp, |req: &Request, res: Response| {
                    assert!(req.headers().get(header::AUTHORIZATION).is_none());
                    assert_eq!(req.headers()["x-h2-tunnel-peer"].to_str().unwrap(), peer);
                    Ok(res)
                })
                .await
                .unwrap();
            let Message::Binary(bytes) = socket.next().await.unwrap().unwrap() else {
                panic!("expected a tunnel hello");
            };
            assert_eq!(decode(&bytes).unwrap(), expect_hello);
            socket
                .send(Message::binary(encode(&send_reply)))
                .await
                .unwrap();
        });
        let token = h.token(&member.key, &member.author, lxm::SESSIONS_RELAY);
        let mut client = upgrade(addr, &lease.id, Some(&token)).await.unwrap();
        client.send(Message::binary(encode(&hello))).await.unwrap();
        let Message::Binary(bytes) = timeout(Duration::from_secs(5), client.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        else {
            panic!("expected a tunnel hello reply");
        };
        assert_eq!(decode(&bytes).unwrap(), reply);
        listener_task.await.unwrap();
    }
}
