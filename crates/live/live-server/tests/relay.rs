//! The co-op relay over real sockets: only a co-op session's two members
//! get in, each one's binary messages reach the other, oversized and
//! too-frequent messages close the socket, and two co-op peers whose direct
//! path is dead play through it (docs/live/live-coop.md, docs/live/live-api.md).
mod common;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use common::*;
use futures_util::{SinkExt, StreamExt};
use live_proto::session::{NewSession, Slots};
use live_proto::{lxm, mailbox};
use live_server::Policy;
use live_server::relay::{CLOSE_CONFLICT, CLOSE_RATE, CLOSE_TOO_LARGE, MAX_RELAY_MESSAGE};
use live_server::store::MemoryStore;
use serde_json::json;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::{Error, Message};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(policy: Policy) -> (Harness, SocketAddr) {
    let h = start(std::sync::Arc::new(MemoryStore::new()), policy).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, h.app.clone()).into_future());
    (h, addr)
}

async fn session(h: &Harness, host: &User, flags: &str) -> String {
    assert_eq!(h.sign_in(host).await.status, StatusCode::CREATED);
    let new = NewSession {
        addr: Some("127.0.0.1".parse().unwrap()),
        port: 31000,
        host_address: vec![0; 36],
        slots: Slots {
            public_filled: 1,
            public_open: 1,
            ..Default::default()
        },
        attributes: BTreeMap::new(),
        flags: flags.into(),
        hopper_id: None,
        map: None,
        name: None,
        language: None,
        visibility: Default::default(),
        map_id: None,
        skulls: None,
        campaign: None,
        ping_ms: None,
        variant_ext: None,
    };
    let r = h
        .call(
            Method::POST,
            "/v1/sessions",
            &[
                h.bearer(host, lxm::SESSIONS_WRITE),
                ("content-type", "application/json".into()),
            ],
            serde_json::to_vec(&new).unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    r.json()["session_id"].as_str().unwrap().into()
}

/// `host` invites `guest` to `id`; the invitation's id.
async fn invite(h: &Harness, host: &User, guest: &User, id: &str) -> u64 {
    let r = h
        .post_event(
            host,
            h.event(
                host,
                &mailbox::stream(guest.xuid),
                mailbox::SENT,
                json!({"from": host.xuid, "session_id": id}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    r.json()["position"].as_u64().unwrap()
}

async fn accept(h: &Harness, guest: &User, id: &str, invite: u64) {
    let r = h
        .post_event(
            guest,
            h.event(
                guest,
                &mailbox::stream(guest.xuid),
                mailbox::ANSWERED,
                json!({"by": guest.xuid, "session_id": id, "invite_id": invite, "answer": "accept"}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
}

/// A co-op session with its host and a guest who accepted.
async fn coop(h: &Harness) -> (User, User, String) {
    let host = h.create("Host").await;
    let guest = h.create("Guest").await;
    assert_eq!(h.sign_in(&guest).await.status, StatusCode::CREATED);
    let id = session(h, &host, "coop").await;
    let n = invite(h, &host, &guest, &id).await;
    accept(h, &guest, &id, n).await;
    (host, guest, id)
}

fn url(addr: SocketAddr, id: &str) -> String {
    format!("ws://{addr}/v1/sessions/{id}/relay")
}

// tungstenite's `Error` is large and this helper only feeds `unwrap` and
// `matches!` in tests.
#[allow(clippy::result_large_err)]
async fn upgrade(addr: SocketAddr, id: &str, token: Option<&str>) -> Result<Socket, Error> {
    let mut req = url(addr, id).into_client_request()?;
    if let Some(token) = token {
        req.headers_mut()
            .insert("authorization", format!("Bearer {token}").parse().unwrap());
    }
    tokio_tungstenite::connect_async(req)
        .await
        .map(|(ws, _)| ws)
}

async fn open(h: &Harness, addr: SocketAddr, id: &str, user: &User) -> Socket {
    let token = h.token(&user.key, &user.author, lxm::SESSIONS_RELAY);
    upgrade(addr, id, Some(&token)).await.unwrap()
}

async fn expect_upgrade_refused(
    addr: SocketAddr,
    id: &str,
    token: Option<&str>,
    expected: StatusCode,
) {
    match upgrade(addr, id, token).await {
        Err(Error::Http(resp)) => assert_eq!(resp.status(), expected),
        Err(e) => panic!("expected HTTP {expected}, got {e}"),
        Ok(_) => panic!("expected HTTP {expected}, but got a socket"),
    }
}

async fn next(ws: &mut Socket) -> Message {
    tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("a message within 5 s")
        .expect("the socket is open")
        .expect("no socket error")
}

async fn expect_peer(ws: &mut Socket, present: bool) {
    match next(ws).await {
        Message::Text(t) => assert_eq!(
            serde_json::from_str::<serde_json::Value>(&t).unwrap(),
            json!({"kind": "peer", "present": present})
        ),
        other => panic!("expected a peer notice, got {other:?}"),
    }
}

async fn expect_close(ws: &mut Socket, code: u16) -> String {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), ws.next()).await {
            Ok(Some(Ok(Message::Close(Some(frame))))) => {
                assert_eq!(u16::from(frame.code), code, "{}", frame.reason);
                return frame.reason.to_string();
            }
            Ok(Some(Ok(_))) => continue,
            other => panic!("expected close {code}, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn members_relay_to_each_other_and_hear_of_each_other() {
    let (h, addr) = serve(Policy::default()).await;
    let (host, guest, id) = coop(&h).await;
    let mut a = open(&h, addr, &id, &host).await;
    let mut b = open(&h, addr, &id, &guest).await;
    expect_peer(&mut a, true).await;
    expect_peer(&mut b, true).await;

    a.send(Message::Binary(vec![1, 2, 3].into())).await.unwrap();
    b.send(Message::Binary(vec![9; MAX_RELAY_MESSAGE].into()))
        .await
        .unwrap();
    assert_eq!(next(&mut b).await, Message::Binary(vec![1, 2, 3].into()));
    assert_eq!(
        next(&mut a).await,
        Message::Binary(vec![9; MAX_RELAY_MESSAGE].into())
    );
    assert_eq!(h.live.relays().forwarded(), 2);

    // The guest connects again (a dropped connection): the old socket is
    // closed, the new one carries on.
    let mut b2 = open(&h, addr, &id, &guest).await;
    expect_close(&mut b, CLOSE_CONFLICT).await;
    expect_peer(&mut b2, true).await;
    expect_peer(&mut a, true).await;
    a.send(Message::Binary(vec![4].into())).await.unwrap();
    assert_eq!(next(&mut b2).await, Message::Binary(vec![4].into()));

    b2.close(None).await.unwrap();
    expect_peer(&mut a, false).await;
    // Alone, a message goes nowhere and nothing breaks.
    a.send(Message::Binary(vec![5].into())).await.unwrap();
    a.close(None).await.unwrap();
    for _ in 0..100 {
        if h.live.relays().rooms() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(h.live.relays().rooms(), 0);
    assert_eq!(h.live.relays().forwarded(), 3);
}

#[tokio::test]
async fn only_the_two_members_of_an_open_coop_session_get_in() {
    let (h, addr) = serve(Policy::default()).await;
    let (host, guest, id) = coop(&h).await;
    let stranger = h.create("Stranger").await;
    let pending = h.create("Pending").await;
    assert_eq!(h.sign_in(&pending).await.status, StatusCode::CREATED);
    invite(&h, &host, &pending, &id).await;

    // A refused admission never upgrades the request.
    for user in [&stranger, &pending] {
        let token = h.token(&user.key, &user.author, lxm::SESSIONS_RELAY);
        expect_upgrade_refused(addr, &id, Some(&token), StatusCode::FORBIDDEN).await;
    }
    expect_upgrade_refused(addr, &id, None, StatusCode::UNAUTHORIZED).await;
    let wrong = h.token(&host.key, &host.author, lxm::SESSIONS_WRITE);
    expect_upgrade_refused(addr, &id, Some(&wrong), StatusCode::UNAUTHORIZED).await;

    let token = h.token(&host.key, &host.author, lxm::SESSIONS_RELAY);
    expect_upgrade_refused(
        addr,
        "0123456789abcdef",
        Some(&token),
        StatusCode::NOT_FOUND,
    )
    .await;

    let other = h.create("Other").await;
    let custom = session(&h, &other, "custom").await;
    let token = h.token(&other.key, &other.author, lxm::SESSIONS_RELAY);
    expect_upgrade_refused(addr, &custom, Some(&token), StatusCode::FORBIDDEN).await;
    assert_eq!(h.live.relays().rooms(), 0);

    // With both members in, a second accepted guest finds the room full.
    let late = h.create("Late").await;
    assert_eq!(h.sign_in(&late).await.status, StatusCode::CREATED);
    let n = invite(&h, &host, &late, &id).await;
    accept(&h, &late, &id, n).await;
    let mut a = open(&h, addr, &id, &host).await;
    let mut b = open(&h, addr, &id, &guest).await;
    expect_peer(&mut a, true).await;
    expect_peer(&mut b, true).await;
    let mut c = open(&h, addr, &id, &late).await;
    expect_close(&mut c, CLOSE_CONFLICT).await;
    assert_eq!(h.live.relays().forwarded(), 0, "nothing was ever relayed");
}

#[tokio::test]
async fn an_oversized_message_closes_the_socket_and_is_not_relayed() {
    let (h, addr) = serve(Policy::default()).await;
    let (host, guest, id) = coop(&h).await;
    let mut a = open(&h, addr, &id, &host).await;
    let mut b = open(&h, addr, &id, &guest).await;
    expect_peer(&mut a, true).await;
    expect_peer(&mut b, true).await;
    a.send(Message::Binary(vec![0; MAX_RELAY_MESSAGE + 1].into()))
        .await
        .unwrap();
    expect_close(&mut a, CLOSE_TOO_LARGE).await;
    expect_peer(&mut b, false).await;
    assert_eq!(h.live.relays().forwarded(), 0);
}

#[tokio::test]
async fn a_socket_that_sends_too_fast_is_closed() {
    let (h, addr) = serve(Policy {
        relay_rate: (1.0, 5),
        ..Policy::default()
    })
    .await;
    let (host, guest, id) = coop(&h).await;
    let mut a = open(&h, addr, &id, &host).await;
    let mut b = open(&h, addr, &id, &guest).await;
    expect_peer(&mut a, true).await;
    expect_peer(&mut b, true).await;
    for n in 0..20u8 {
        if a.send(Message::Binary(vec![n].into())).await.is_err() {
            break;
        }
    }
    expect_close(&mut a, CLOSE_RATE).await;
    // The burst got through, in order; the rest did not.
    for n in 0..5u8 {
        assert_eq!(next(&mut b).await, Message::Binary(vec![n].into()));
    }
    expect_peer(&mut b, false).await;
    assert_eq!(h.live.relays().forwarded(), 5);
}

/// One co-op machine on a thread: its direct path never delivers, its relay
/// is the real socket. Returns the pads its guest read, frame by frame.
fn play(
    relay_url: String,
    token: String,
    port: usize,
    frames: usize,
) -> std::thread::JoinHandle<(Vec<[coop::Pad; 2]>, coop::link::Path, u64)> {
    use coop::link::{Conditions, FakeClock, Fallback, sim_pair, wall_clock};
    std::thread::spawn(move || {
        let relay = coop::relay::RelayLink::connect(&relay_url, &token).unwrap();
        // A direct path into the void: the other end is never read.
        let (dead, _other_end, control) = sim_pair(&FakeClock::default(), Conditions::default(), 1);
        control.set_down(true);
        let link = Fallback::new(dead, relay, wall_clock(), 500_000);
        let mut peer = coop::Peer::new(link, 0x5e55_1017, port, 2);
        let mut seen = Vec::new();
        let started = std::time::Instant::now();
        while seen.len() < frames {
            assert!(
                started.elapsed().as_secs() < 30,
                "port {port} stuck at frame {}",
                seen.len()
            );
            peer.pump();
            let mut pad = [0u8; coop::PAD];
            pad[0] = port as u8 + 1;
            pad[1] = seen.len() as u8;
            if let Ok(frame) = peer.begin_frame(pad) {
                let pads = peer.pads().unwrap();
                peer.report_hash(frame, u64::from(pads[0][1]) << 8 | u64::from(pads[1][1]));
                seen.push(pads);
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        // Let the last hashes and acknowledgements through.
        for _ in 0..50 {
            peer.pump();
            std::thread::sleep(Duration::from_millis(4));
        }
        assert_eq!(peer.desync(), None);
        (seen, peer.link().path(), peer.stats().ticks_agreed)
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_coop_peers_play_through_the_relay_when_direct_is_dead() {
    let (h, addr) = serve(Policy::default()).await;
    let (host, guest, id) = coop(&h).await;
    const FRAMES: usize = 90;
    let a = play(
        url(addr, &id),
        h.token(&host.key, &host.author, lxm::SESSIONS_RELAY),
        0,
        FRAMES,
    );
    let b = play(
        url(addr, &id),
        h.token(&guest.key, &guest.author, lxm::SESSIONS_RELAY),
        1,
        FRAMES,
    );
    let (a, b) = tokio::task::spawn_blocking(move || (a.join().unwrap(), b.join().unwrap()))
        .await
        .unwrap();
    assert_eq!(a.0, b.0, "both guests read the same pads");
    assert_eq!(a.0.len(), FRAMES);
    // Two neutral frames, then each player's samples in order on their port.
    for (f, pads) in a.0.iter().enumerate().skip(2) {
        assert_eq!((pads[0][0], pads[0][1]), (1, f as u8 - 2));
        assert_eq!((pads[1][0], pads[1][1]), (2, f as u8 - 2));
    }
    assert_eq!(
        (a.1, b.1),
        (coop::link::Path::Relay, coop::link::Path::Relay)
    );
    assert!(
        a.2 >= FRAMES as u64 - 2 && b.2 >= FRAMES as u64 - 2,
        "{} {}",
        a.2,
        b.2
    );
    assert!(h.live.relays().forwarded() > FRAMES as u64);
}

/// The hub's tickets for a relay that is not the hub (the Worker's
/// `CoopRelay`): signed for a member, checked with the server's key,
/// renewed while it stays a member, refused to anyone else.
#[tokio::test]
async fn members_get_signed_tickets_that_renew_while_they_stay_members() {
    use live_server::relay::{Ticket, TicketReply};
    let (h, _) = serve(Policy::default()).await;
    let (host, guest, id) = coop(&h).await;
    let stranger = h.create("Stranger").await;
    let uri = format!("/v1/sessions/{id}/relay/ticket");
    let post = |user: &User, method: &str| {
        let bearer = h.bearer(user, method);
        let uri = uri.clone();
        let h = &h;
        async move { h.call(Method::POST, &uri, &[bearer], Vec::new()).await }
    };

    let r = post(&guest, lxm::SESSIONS_RELAY).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let reply: TicketReply = serde_json::from_slice(&r.body).unwrap();
    let key = h.operator.public();
    let t = Ticket::open(key, &reply.ticket, &id, now()).unwrap();
    assert_eq!(
        (t.xuid, t.author.as_str()),
        (guest.xuid, guest.author.as_str())
    );
    assert!(t.exp > now() && t.exp <= now() + live_server::relay::TICKET_S);
    assert!(Ticket::open(key, &reply.ticket, "another", now()).is_err());
    assert!(
        Ticket::open(
            live_proto::sign::KeyPair::generate().public(),
            &reply.ticket,
            &id,
            now()
        )
        .is_err()
    );

    // Renewed with the ticket alone.
    let renew = |ticket: String| {
        let uri = uri.clone();
        let h = &h;
        async move {
            h.call(
                Method::POST,
                &uri,
                &[("content-type", "application/json".into())],
                json!({ "ticket": ticket }).to_string(),
            )
            .await
        }
    };
    let r = renew(reply.ticket.clone()).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let again: TicketReply = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(
        Ticket::open(key, &again.ticket, &id, now()).unwrap().xuid,
        guest.xuid
    );
    let r = renew("not.a.ticket".into()).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Not to a stranger, nor for another method; the host gets one too.
    assert_eq!(
        post(&stranger, lxm::SESSIONS_RELAY).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&host, lxm::SESSIONS_WRITE).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(&host, lxm::SESSIONS_RELAY).await.status,
        StatusCode::OK
    );
}
