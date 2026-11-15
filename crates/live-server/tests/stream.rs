//! `/v1/stream` over a real socket: replay, `caught_up`, live events,
//! filters, and `--private` auth as the first message.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use live_proto::sign::{AccountRequest, KeyPair, issue_token};
use live_proto::{ClientMessage, CreatedAccount, StreamMessage, lxm};
use live_server::store::MemoryStore;
use live_server::{Live, Policy, ServerIdentity, http};
use tokio_tungstenite::tungstenite::Message;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(policy: Policy) -> (Arc<Live>, SocketAddr) {
    let dir = std::env::temp_dir().join(format!(
        "h2live-stream-{}-{}",
        std::process::id(),
        live_proto::sign::random_id()
    ));
    let identity = ServerIdentity::open(&dir, Some("live.test")).unwrap();
    let live = Arc::new(
        Live::start(Arc::new(MemoryStore::new()), identity, policy)
            .await
            .unwrap(),
    );
    let _ = std::fs::remove_dir_all(&dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, http::router(live.clone())).into_future());
    (live, addr)
}

async fn create(live: &Live, tag: &str) -> (KeyPair, CreatedAccount) {
    let key = KeyPair::generate();
    let created = live
        .create_account(&AccountRequest::sign(&key, tag, None))
        .await
        .unwrap();
    (key, created)
}

async fn connect(addr: SocketAddr, query: &str) -> Socket {
    tokio_tungstenite::connect_async(format!("ws://{addr}/v1/stream{query}"))
        .await
        .unwrap()
        .0
}

async fn next(ws: &mut Socket) -> StreamMessage {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("a message in time")
            .unwrap()
            .unwrap();
        match msg {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected {other:?}"),
        }
    }
}

fn event_type(m: &StreamMessage) -> (u64, String) {
    match m {
        StreamMessage::Event { event } => (event.position, event.kind.clone()),
        other => panic!("expected an event, got {other:?}"),
    }
}

#[tokio::test]
async fn replays_then_follows() {
    let (live, addr) = serve(Policy::default()).await;
    create(&live, "First").await;

    let mut ws = connect(addr, "?from=0").await;
    let replayed: Vec<_> = [
        next(&mut ws).await,
        next(&mut ws).await,
        next(&mut ws).await,
    ]
    .iter()
    .map(event_type)
    .collect();
    assert_eq!(
        replayed,
        [
            (1, "GamertagReserved".into()),
            (2, "AccountCreated".into()),
            (3, "KeyAdded".into())
        ]
    );
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 3 });

    create(&live, "Second").await;
    let live_events: Vec<_> = [
        next(&mut ws).await,
        next(&mut ws).await,
        next(&mut ws).await,
    ]
    .iter()
    .map(event_type)
    .collect();
    assert_eq!(
        live_events.iter().map(|e| e.0).collect::<Vec<_>>(),
        [4, 5, 6]
    );

    // `from` is exclusive; no `from` starts at the head.
    let mut ws = connect(addr, "?from=5").await;
    assert_eq!(event_type(&next(&mut ws).await).0, 6);
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 6 });
    let mut ws = connect(addr, "").await;
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 6 });
    create(&live, "Third").await;
    assert_eq!(event_type(&next(&mut ws).await).0, 7);
}

#[tokio::test]
async fn filters_types() {
    let (live, addr) = serve(Policy::default()).await;
    create(&live, "First").await;
    let mut ws = connect(addr, "?from=0&exclude=KeyAdded,GamertagReserved").await;
    assert_eq!(
        event_type(&next(&mut ws).await),
        (2, "AccountCreated".into())
    );
    // caught_up still reports the head, past the filtered-out events.
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 3 });
    let mut ws = connect(addr, "?from=0&types=KeyAdded").await;
    assert_eq!(event_type(&next(&mut ws).await), (3, "KeyAdded".into()));
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 3 });
    create(&live, "Second").await;
    assert_eq!(event_type(&next(&mut ws).await), (6, "KeyAdded".into()));
}

async fn expect_close(ws: &mut Socket, code: u16) {
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match msg {
        Message::Close(Some(frame)) => assert_eq!(u16::from(frame.code), code),
        other => panic!("expected a close, got {other:?}"),
    }
}

#[tokio::test]
async fn private_streams_authenticate_first() {
    let (live, addr) = serve(Policy {
        private: true,
        ..Policy::default()
    })
    .await;
    let (key, account) = create(&live, "First").await;
    let auth = |method: &str| {
        let token = issue_token(
            &key,
            &account.id,
            "live.test",
            method,
            live_server::app::now(),
        );
        Message::Text(
            serde_json::to_string(&ClientMessage::Auth { token })
                .unwrap()
                .into(),
        )
    };

    let mut ws = connect(addr, "?from=0").await;
    ws.send(Message::Text("{\"kind\":\"hello\"}".into()))
        .await
        .unwrap();
    expect_close(&mut ws, 4401).await;

    let mut ws = connect(addr, "?from=0").await;
    ws.send(auth(lxm::EVENTS_WRITE)).await.unwrap();
    expect_close(&mut ws, 4401).await;

    let mut ws = connect(addr, "?from=0").await;
    ws.send(auth(lxm::READ)).await.unwrap();
    assert_eq!(event_type(&next(&mut ws).await).0, 1);
    for _ in 0..2 {
        next(&mut ws).await;
    }
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 3 });
    // A fresh token is accepted mid-stream; a bad one closes it.
    ws.send(auth(lxm::READ)).await.unwrap();
    create(&live, "Second").await;
    assert_eq!(event_type(&next(&mut ws).await).0, 4);
    ws.send(Message::Text(
        "{\"kind\":\"auth\",\"token\":\"a.b.c\"}".into(),
    ))
    .await
    .unwrap();
    for _ in 0..2 {
        // The rest of Second's events may arrive before the close.
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 4401);
                return;
            }
            _ => continue,
        }
    }
    expect_close(&mut ws, 4401).await;
}
