//! Signed requests against the real embedded memory-backed service. Guest
//! frames and all files are synthetic; no XBE, image build, or game execution.
use live_hle::{
    ATTACHMENT_CAP, Hle, LiveConfig, NetworkConfig, SUMMARY_SIZE, ServerOptions, entries::ENTRIES,
    status::*,
};
use live_proto::{
    CreatedAccount, Discovery, Page, Presence, WireEvent, Xuid, lxm,
    motd::Motd,
    sign::{self, AccountRequest, KeyPair, SignedEvent},
    types,
};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, exec::HARNESS_EXIT,
    guest::Permissions,
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::PathBuf,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
const DATA: u32 = 0x700000;
const OUT: u32 = DATA + 0x1000;
const USERS: u32 = DATA + 0x2000;
const BUFFER: u32 = DATA + 0x6000;
const FRAME: u32 = 0x600f00;
struct Account {
    xuid: Xuid,
    tag: String,
    key: Arc<KeyPair>,
}
impl Account {
    fn json(&self) -> Value {
        json!({"xuid":self.xuid,"gamertag":self.tag,"server":"ignored per-account URL","key":self.key.private_jwk()})
    }
}
struct Server {
    embedded: live_server::Embedded,
    dir: PathBuf,
    agent: ureq::Agent,
    discovery: Discovery,
    operator: KeyPair,
}
impl Server {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "h2-hle-http-{}-{}",
            std::process::id(),
            sign::random_id()
        ));
        let embedded = live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &dir,
            live_server::http::Site::default(),
        )
        .unwrap();
        let operator = live_server::ServerIdentity::load(&dir).unwrap().key;
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(3))
            .build();
        let discovery = serde_json::from_reader(
            agent
                .get(&format!("{}/.well-known/h2live.json", embedded.url))
                .call()
                .unwrap()
                .into_reader(),
        )
        .unwrap();
        Self {
            embedded,
            dir,
            agent,
            discovery,
            operator,
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.embedded.url)
    }
    fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> T {
        serde_json::from_reader(
            self.agent
                .get(&self.url(path))
                .call()
                .unwrap()
                .into_reader(),
        )
        .unwrap()
    }
    fn account(&self, tag: &str) -> Account {
        let key = Arc::new(KeyPair::generate());
        let body = AccountRequest::sign(
            &key,
            tag,
            None,
            &self.discovery.id,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        );
        let created: CreatedAccount = serde_json::from_reader(
            self.agent
                .post(&self.url("/v1/accounts"))
                .set("content-type", "application/jose")
                .send_string(&body)
                .unwrap()
                .into_reader(),
        )
        .unwrap();
        Account {
            xuid: created.xuid,
            tag: created.gamertag.to_string(),
            key,
        }
    }
    fn publish(&self, name: &str, bytes: &[u8]) {
        let token = sign::issue_token(
            &self.operator,
            &format!("server:{}", self.discovery.id),
            &self.discovery.id,
            lxm::TITLES_WRITE,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        );
        self.agent
            .put(&self.url(&format!("/v1/titles/4D530064/files/{name}")))
            .set("authorization", &format!("Bearer {token}"))
            .send_bytes(bytes)
            .unwrap();
    }
    fn events(&self) -> Vec<WireEvent> {
        self.get::<Page<WireEvent>>("/v1/events?from=0&limit=1000")
            .items
    }
    fn presence(&self) -> Vec<Presence> {
        self.get::<Page<Presence>>("/v1/presence").items
    }
    fn until(&self, mut condition: impl FnMut() -> bool) {
        let limit = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < limit, "server observation timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
struct World {
    rt: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
    hle: Arc<Hle>,
}
impl World {
    fn new(url: &str, accounts: &[&Account]) -> Self {
        Self::timed(url, accounts, Duration::from_millis(100))
    }
    fn timed(url: &str, accounts: &[&Account], heartbeat: Duration) -> Self {
        Self::raw(
            url,
            json!(accounts.iter().map(|a| a.json()).collect::<Vec<_>>()),
            heartbeat,
        )
    }
    fn raw(url: &str, accounts: Value, heartbeat: Duration) -> Self {
        let path = std::env::temp_dir().join(format!("h2-hle-accounts-{}.json", sign::random_id()));
        std::fs::write(&path, serde_json::to_vec(&accounts).unwrap()).unwrap();
        let hle = Hle::with_server_options(
            LiveConfig {
                enabled: true,
                accounts_path: Some(path.clone()),
                server_url: Some(url.into()),
            },
            NetworkConfig::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 31000)),
            ServerOptions {
                request_timeout: Duration::from_secs(2),
                heartbeat_interval: heartbeat,
            },
        )
        .unwrap();
        std::fs::remove_file(path).unwrap();
        let mut guest = Guest::new();
        guest
            .map_zeroed(0x360000, 0x10000, Permissions::RX)
            .unwrap();
        guest
            .map_zeroed(0x3a0000, 0x40000, Permissions::RX)
            .unwrap();
        guest.map_zeroed(0x600000, 0x1000, Permissions::RW).unwrap();
        guest.map_zeroed(DATA, 0x10000, Permissions::RW).unwrap();
        let mut dispatch = Dispatch::default();
        live_hle::register(&mut dispatch, &hle).unwrap();
        let rt = Runtime::new(guest, dispatch);
        let ctx = ExecCtx::new(rt.clone(), 1);
        Self {
            rt,
            ctx,
            cpu: CpuAbi::default(),
            hle,
        }
    }
    fn call(&mut self, name: &str, args: &[u32]) -> u32 {
        let e = ENTRIES.iter().find(|e| e.name == name).unwrap();
        assert_eq!(e.stack_words as usize, args.len(), "{name}");
        {
            let mut m = self.rt.memory();
            m.set_u32(FRAME, HARNESS_EXIT).unwrap();
            for (i, a) in args.iter().enumerate() {
                m.set_u32(FRAME + 4 + i as u32 * 4, *a).unwrap();
            }
        }
        self.cpu = CpuAbi {
            esp: FRAME,
            eip: e.va,
            ebx: 1,
            esi: 2,
            edi: 3,
            ebp: 4,
            ecx: 5,
            edx: 6,
            ..CpuAbi::default()
        };
        let t = Runner::default().run(&mut self.cpu, &mut self.ctx, e.va);
        assert!(
            matches!(self.ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{name}: {:?}",
            self.ctx.event(t.event_id)
        );
        assert_eq!(self.cpu.esp, FRAME + 4 + 4 * args.len() as u32);
        assert_eq!(
            (
                self.cpu.ebx,
                self.cpu.esi,
                self.cpu.edi,
                self.cpu.ebp,
                self.cpu.ecx,
                self.cpu.edx
            ),
            (1, 2, 3, 4, 5, 6)
        );
        self.cpu.eax
    }
    fn put(&self, p: u32, b: &[u8]) {
        self.rt.memory().write_bytes(p, b, 1).unwrap();
    }
    fn word(&self, p: u32, v: u32) {
        self.put(p, &v.to_le_bytes());
    }
    fn u32(&self, p: u32) -> u32 {
        self.rt.memory().u32(p).unwrap()
    }
    fn bytes(&self, p: u32, n: usize) -> Vec<u8> {
        let mut b = vec![0; n];
        self.rt.memory().read_bytes(p, &mut b, 1).unwrap();
        b
    }
    fn begin(&mut self, slots: &[usize]) -> u32 {
        assert_eq!(self.call("XNetStartup", &[0]), 0);
        assert_eq!(self.call("WSAStartup", &[2, DATA]), 0);
        assert_eq!(self.call("XOnlineStartup", &[0]), 0);
        assert_eq!(self.call("XOnlineGetUsers", &[DATA, OUT]), 0);
        for (slot, index) in slots.iter().enumerate() {
            self.put(
                USERS + slot as u32 * 112,
                &self.bytes(DATA + *index as u32 * 112, 112),
            );
        }
        self.word(DATA + 0x4000, 2);
        assert_eq!(
            self.call("XOnlineLogon", &[USERS, DATA + 0x4000, 1, 0, OUT]),
            0
        );
        self.u32(OUT)
    }
    fn wait(&mut self, h: u32) -> u32 {
        let limit = Instant::now() + Duration::from_secs(6);
        loop {
            let result = self.call("XOnlineTaskContinue", &[h]);
            if result != 0 {
                return result;
            }
            assert!(Instant::now() < limit, "task timed out");
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    fn logon(&mut self) -> u32 {
        let h = self.begin(&[0]);
        assert_eq!(self.wait(h), CONNECTED);
        h
    }
    fn messages(&mut self) -> Vec<u32> {
        self.word(OUT, u32::MAX);
        assert_eq!(self.call("XOnlineMessageEnumerate", &[0, DATA, OUT]), 0);
        (0..self.u32(OUT))
            .map(|i| self.u32(DATA + i * SUMMARY_SIZE as u32 + 32))
            .collect()
    }
    fn details(&mut self, id: u32) -> u32 {
        assert_eq!(
            self.call("XOnlineMessageDetails", &[0, id, 0x10, 0, 0, OUT]),
            0
        );
        let h = self.u32(OUT);
        assert_eq!(self.wait(h), TASK_SUCCESS);
        h
    }
    fn property(&mut self, h: u32, tag: u32) -> u32 {
        self.word(OUT, 0xdeadbeef);
        self.word(OUT + 4, u32::MAX);
        self.call(
            "XOnlineMessageDetailsGetResultsProperty",
            &[h, tag, 2048, DATA, OUT, OUT + 4],
        )
    }
}
fn motd() -> Vec<u8> {
    Motd {
        header: "Welcome".into(),
        messages: vec!["Synthetic client-server integration test".into()],
    }
    .build()
    .unwrap()
}
#[test]
fn logon_verifies_the_account_key_and_appends_signed_presence() {
    let server = Server::new();
    let account = server.account("Test Player");
    let mut w = World::new(&server.embedded.url, &[&account]);
    let h = w.logon();
    let p = w.call("XOnlineGetLogonUsers", &[]);
    assert_ne!(p, 0);
    assert_eq!(w.bytes(p, 8), account.xuid.0.to_le_bytes());
    assert_eq!(w.u32(p + 108), 0);
    assert_eq!(w.call("XOnlineLogonTaskGetResults", &[h]), CONNECTED);
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), CONNECTED);
    let events = server.events();
    let signed = events.iter().find(|e| e.kind == types::SIGNED_IN).unwrap();
    let event = SignedEvent::open(&signed.jws).unwrap();
    event.verify(account.key.public()).unwrap();
    assert_eq!(
        event.body.author,
        format!("acct:{}@{}", account.xuid, server.discovery.id)
    );
    assert_eq!(event.body.data["state"], "online");
    assert_eq!(server.presence()[0].xuid, account.xuid);
    assert!(
        !w.bytes(DATA, 112 * 16)
            .windows(account.key.secret_b64().len())
            .any(|b| b == account.key.secret_b64().as_bytes())
    );
}
#[test]
fn unknown_account_and_wrong_or_missing_key_fail_as_invalid_user() {
    let server = Server::new();
    let known = server.account("Known");
    let cases = [
        Account {
            xuid: Xuid(0x00090000ffffffff),
            tag: "Unknown".into(),
            key: Arc::new(KeyPair::generate()),
        },
        Account {
            xuid: known.xuid,
            tag: known.tag.clone(),
            key: Arc::new(KeyPair::generate()),
        },
    ];
    for account in &cases {
        let mut w = World::new(&server.embedded.url, &[account]);
        let h = w.begin(&[0]);
        assert_eq!(w.wait(h), INVALID_USER);
        assert_eq!(w.call("XOnlineLogonTaskGetResults", &[h]), INVALID_USER);
        assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
    }
    for key in [
        Value::Null,
        json!({"kty":"OKP", "crv":"Ed25519", "x":"invalid", "d":"private-test-marker"}),
    ] {
        let mut record = known.json();
        record["key"] = key;
        let mut w = World::raw(
            &server.embedded.url,
            json!([record]),
            Duration::from_secs(30),
        );
        let h = w.begin(&[0]);
        assert_eq!(w.wait(h), INVALID_USER);
        assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
    }
    assert!(server.presence().is_empty());
    assert!(!server.events().iter().any(|e| e.kind == types::SIGNED_IN));
}
#[test]
fn unreachable_server_reports_cannot_access_service_through_the_logon_task() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let a = Account {
        xuid: Xuid(0x0009000000000001),
        tag: "Offline".into(),
        key: Arc::new(KeyPair::generate()),
    };
    let mut w = World::new(&format!("http://{addr}"), &[&a]);
    let start = Instant::now();
    let h = w.begin(&[0]);
    assert!(start.elapsed() < Duration::from_millis(200));
    assert_eq!(w.wait(h), CANNOT_ACCESS_SERVICE);
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
}
#[test]
fn logon_and_task_pumps_do_not_wait_for_a_stalled_http_server() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (entered, seen) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut input = [0; 2048];
        assert_ne!(socket.read(&mut input).unwrap(), 0);
        entered.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        socket.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let a = Account {
        xuid: Xuid(0x0009000000000001),
        tag: "Waiting".into(),
        key: Arc::new(KeyPair::generate()),
    };
    let mut w = World::timed(&url, &[&a], Duration::from_secs(30));
    let h = w.begin(&[0]);
    seen.recv_timeout(Duration::from_secs(2)).unwrap();
    for _ in 0..10 {
        let now = Instant::now();
        assert_eq!(w.call("XOnlineTaskContinue", &[h]), 0);
        assert_eq!(w.call("XOnlineLogonTaskGetResults", &[h]), 0);
        assert!(now.elapsed() < Duration::from_millis(100));
    }
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
    release.send(()).unwrap();
    assert_eq!(w.wait(h), SERVERS_BUSY);
    thread.join().unwrap();
}
#[test]
fn notifications_presence_tasks_and_game_hook_publish_real_presence_changes() {
    let server = Server::new();
    let a = server.account("Present");
    let mut w = World::new(&server.embedded.url, &[&a]);
    w.logon();
    assert_eq!(
        w.call("XOnlineNotificationSetState", &[0, 0, 0, 0, 0, 0]),
        0
    );
    server.until(|| server.presence().first().is_some_and(|p| p.state == "away"));
    assert_eq!(w.call("XOnlinePresenceInit", &[0, 0, OUT]), 0);
    let p = w.u32(OUT);
    assert_eq!(w.wait(p), TASK_SUCCESS);
    assert_eq!(
        w.call("XOnlineNotificationSetState", &[0, 3, 0, 0, 0, 0]),
        0
    );
    server.until(|| {
        server
            .presence()
            .first()
            .is_some_and(|p| p.state == "online")
    });
    assert_eq!(w.hle.set_game_id(Some("telemetry-game")), Ok(()));
    server.until(|| {
        server
            .presence()
            .first()
            .is_some_and(|p| p.state == "playing" && p.game_id.as_deref() == Some("telemetry-game"))
    });
    assert_eq!(w.call("XOnlinePresenceSubmit", &[p]), 0);
    assert_eq!(w.wait(p), TASK_SUCCESS);
    assert_eq!(w.hle.set_game_id(None), Ok(()));
    server.until(|| {
        server
            .presence()
            .first()
            .is_some_and(|p| p.state == "online")
    });
    for e in server
        .events()
        .iter()
        .filter(|e| e.kind == types::PRESENCE_CHANGED)
    {
        SignedEvent::open(&e.jws)
            .unwrap()
            .verify(a.key.public())
            .unwrap();
    }
    assert_eq!(w.hle.set_game_id(Some("invalid/id")), Err(INVALID_ARGUMENT));
}
#[test]
fn background_pings_are_accepted_by_the_signed_in_server() {
    let server = Server::new();
    let a = server.account("Heartbeat");
    let mut w = World::new(&server.embedded.url, &[&a]);
    let h = w.logon();
    server.until(|| w.hle.server_status().acknowledged_pings >= 3);
    assert_eq!(server.presence()[0].xuid, a.xuid);
    assert_eq!(w.hle.server_status().last_error, 0);
    assert_eq!(w.call("XOnlineTaskClose", &[h]), 0);
    server.until(|| server.presence().is_empty());
    assert!(server.events().iter().any(|e| e.kind == types::SIGNED_OUT));
    assert_eq!(w.hle.server_status().last_error, 0);
}
#[test]
fn signout_cleanup_and_user_change_remove_only_the_selected_accounts() {
    let server = Server::new();
    let a = server.account("First");
    let b = server.account("Second");
    let mut w = World::new(&server.embedded.url, &[&a, &b]);
    let h = w.logon();
    assert_eq!(server.presence()[0].xuid, a.xuid);
    w.put(USERS, &[0; 448]);
    w.put(USERS, &w.bytes(DATA + 112, 112));
    assert_eq!(w.call("XOnlineChangeLogonUsers", &[USERS, 0, OUT]), 0);
    let change = w.u32(OUT);
    assert_eq!(w.wait(change), USER_CHANGE_COMMIT);
    assert_eq!(
        w.call("XOnlineTaskContinue", &[change]),
        USER_CHANGE_COMPLETE
    );
    assert_eq!(
        server.presence().iter().map(|p| p.xuid).collect::<Vec<_>>(),
        vec![b.xuid]
    );
    assert_eq!(w.call("XOnlineTaskClose", &[h]), 0);
    server.until(|| server.presence().is_empty());
    // A fresh logon is independent of the cancelled generation.
    w.put(USERS, &w.bytes(DATA, 112));
    assert_eq!(w.call("XOnlineLogon", &[USERS, 0, 0, 0, OUT]), 0);
    let h = w.u32(OUT);
    assert_eq!(w.wait(h), CONNECTED);
    assert_eq!(w.call("XOnlineCleanup", &[]), 0);
    server.until(|| server.presence().is_empty());
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
}
#[test]
fn title_files_flow_from_server_messages_to_the_guest_download_buffer() {
    let server = Server::new();
    let a = server.account("Reader");
    let file = motd();
    server.publish("message_of_the_day.dat", &file);
    server.publish("multiplayer_hopper.dat", b"synthetic hopper envelope");
    let mut w = World::new(&server.embedded.url, &[&a]);
    w.logon();
    assert_eq!(w.messages(), vec![1, 4]);
    assert_eq!(w.bytes(DATA + 12, 1), vec![1]);
    assert_eq!(w.u32(DATA + 40), 0x4d530064);
    assert_eq!(w.call("XOnlineGetNotificationEx", &[0, DATA, OUT]), 1);
    assert_eq!(w.bytes(DATA, 1), vec![1]);
    assert_eq!(w.u32(DATA + 1), 1);
    assert_eq!(w.u32(OUT), 1);
    assert_eq!(w.call("XOnlineGetNotificationEx", &[0, DATA, OUT]), 1);
    assert_eq!(w.u32(DATA + 1), 4);
    assert_eq!(w.u32(OUT), 0);
    assert_eq!(w.call("XOnlineGetNotificationEx", &[0, DATA, OUT]), 0);
    let details = w.details(1);
    for (tag, size, expected) in [
        (0x3c2, 2, Some(1)),
        (0x4c3, 4, Some(1)),
        (0x4c5, 4, Some(1)),
        (0x581, 8, None),
        (0x6c4, 46, None),
    ] {
        assert_eq!(w.property(details, tag), 0);
        assert_eq!(w.u32(OUT), size);
        assert_eq!(w.u32(OUT + 4), 0);
        if let Some(n) = expected {
            assert_eq!(
                w.bytes(DATA, size as usize),
                if size == 2 {
                    (n as u16).to_le_bytes().to_vec()
                } else {
                    (n as u32).to_le_bytes().to_vec()
                }
            );
        }
    }
    w.put(DATA, &[0xcc; 32]);
    assert_eq!(w.property(details, 0x9c1), MESSAGE_DOWNLOAD_REQUIRED);
    assert_eq!(w.u32(OUT), file.len() as u32);
    assert_eq!(w.bytes(DATA, 32), vec![0xcc; 32]);
    w.put(BUFFER, &vec![0xcc; ATTACHMENT_CAP as usize + 4]);
    assert_eq!(
        w.call(
            "XOnlineMessageDownloadAttachmentToMemory",
            &[details, 0x9c1, BUFFER, ATTACHMENT_CAP, 0, OUT]
        ),
        0
    );
    let download = w.u32(OUT);
    assert_eq!(w.wait(download), TASK_SUCCESS);
    assert_eq!(
        w.call(
            "XOnlineMessageDownloadAttachmentToMemoryGetResults",
            &[download, OUT, OUT + 4, OUT + 8]
        ),
        0
    );
    assert_eq!(w.u32(OUT), BUFFER);
    assert_eq!(w.u32(OUT + 4), file.len() as u32);
    assert_eq!(w.u32(OUT + 8), file.len() as u32);
    let received = w.bytes(BUFFER, file.len());
    assert_eq!(received, file);
    assert_eq!(w.bytes(BUFFER + file.len() as u32, 4), vec![0xcc; 4]);
    assert_eq!(Motd::parse(&received).unwrap().header, "Welcome");
    let mut inflated = Vec::new();
    flate2::read::ZlibDecoder::new(&received[20..])
        .read_to_end(&mut inflated)
        .unwrap();
    assert_eq!(inflated.len(), 0x25224);
    assert_eq!(
        u32::from_be_bytes(received[16..20].try_into().unwrap()),
        0x25224
    );
    assert_eq!(w.call("XOnlineMessageDelete", &[0, 1, 0]), 0);
    assert_eq!(w.messages(), vec![4]);
    assert_eq!(
        w.call("XOnlineMessageDelete", &[0, 1, 0]),
        MESSAGE_INVALID_ID
    );
    let hopper = w.details(4);
    assert_eq!(w.property(hopper, 0x4c5), 0);
    assert_eq!(w.u32(DATA), 4);
    assert_eq!(w.call("XOnlineTaskClose", &[download]), 0);
    assert_eq!(w.call("XOnlineTaskContinue", &[download]), INVALID_ARGUMENT);
}
#[test]
fn absent_files_are_not_messages_and_large_attachments_fail_without_buffer_writes() {
    let server = Server::new();
    let a = server.account("Empty");
    let mut w = World::new(&server.embedded.url, &[&a]);
    let logon = w.logon();
    assert!(w.messages().is_empty());
    assert_eq!(
        w.call("XOnlineMessageDetails", &[0, 1, 0, 0, 0, OUT]),
        MESSAGE_INVALID_ID
    );
    assert_eq!(w.call("XOnlineTaskClose", &[logon]), 0);
    server.until(|| server.presence().is_empty());
    server.publish(
        "message_of_the_day.dat",
        &vec![0x5a; ATTACHMENT_CAP as usize + 1],
    );
    assert_eq!(w.call("XOnlineLogon", &[USERS, 0, 0, 0, OUT]), 0);
    let logon = w.u32(OUT);
    assert_eq!(w.wait(logon), CONNECTED);
    assert_eq!(w.messages(), vec![1]);
    let details = w.details(1);
    w.word(OUT, 0xdeadbeef);
    w.put(BUFFER, &[0xcc; 32]);
    assert_eq!(
        w.call(
            "XOnlineMessageDownloadAttachmentToMemory",
            &[details, 0x9c1, BUFFER, ATTACHMENT_CAP, 0, OUT]
        ),
        BUFFER_TOO_SMALL
    );
    assert_eq!(w.u32(OUT), 0xdeadbeef);
    assert_eq!(w.bytes(BUFFER, 32), vec![0xcc; 32]);
    // A larger caller buffer must not bypass the title's attachment cap.
    assert_eq!(
        w.call(
            "XOnlineMessageDownloadAttachmentToMemory",
            &[details, 0x9c1, BUFFER, ATTACHMENT_CAP + 1, 0, OUT]
        ),
        BUFFER_TOO_SMALL
    );
}
#[test]
fn stale_and_wrong_message_tasks_and_property_buffers_return_xdk_errors() {
    let server = Server::new();
    let a = server.account("Bounds");
    server.publish("message_of_the_day.dat", &motd());
    let mut w = World::new(&server.embedded.url, &[&a]);
    w.logon();
    let d = w.details(1);
    assert_eq!(w.property(d, 0xffff), MESSAGE_PROPERTY_NOT_FOUND);
    assert_eq!(
        w.call(
            "XOnlineMessageDetailsGetResultsProperty",
            &[d, 0x4c5, 0, 0, OUT, OUT + 4]
        ),
        BUFFER_TOO_SMALL
    );
    assert_eq!(w.u32(OUT), 4);
    w.put(DATA, &[0xcc; 8]);
    assert_eq!(
        w.call(
            "XOnlineMessageDetailsGetResultsProperty",
            &[d, 0x4c5, 2, DATA, OUT, 0]
        ),
        BUFFER_TOO_SMALL
    );
    assert_eq!(w.bytes(DATA, 8), vec![0xcc; 8]);
    assert_eq!(
        w.call(
            "XOnlineMessageDownloadAttachmentToMemory",
            &[d, 0x4c5, BUFFER, ATTACHMENT_CAP, 0, OUT]
        ),
        MESSAGE_PROPERTY_NOT_FOUND
    );
    assert_eq!(w.call("XOnlineTaskClose", &[d]), 0);
    assert_eq!(w.property(d, 0x4c5), INVALID_ARGUMENT);
}

#[test]
fn failed_user_change_preserves_the_connection_and_exposes_per_user_errors() {
    let server = Server::new();
    let a = server.account("Stays");
    let unknown = Account {
        xuid: Xuid(0x00090000ffffffff),
        tag: "Missing".into(),
        key: Arc::new(KeyPair::generate()),
    };
    let mut w = World::new(&server.embedded.url, &[&a, &unknown]);
    let logon = w.logon();
    let users = w.call("XOnlineGetLogonUsers", &[]);
    w.put(USERS, &[0; 448]);
    w.put(USERS, &w.bytes(DATA + 112, 112));
    assert_eq!(w.call("XOnlineChangeLogonUsers", &[USERS, 0, OUT]), 0);
    let change = w.u32(OUT);
    assert_eq!(w.wait(change), USER_CHANGE_FAILED);
    w.put(OUT, &[0xcc; 20]);
    assert_eq!(
        w.call("XOnlineChangeLogonUsersTaskGetResults", &[change, OUT]),
        0
    );
    assert_eq!(w.u32(OUT), INVALID_USER);
    assert_eq!(w.bytes(OUT + 4, 12), vec![0; 12]);
    assert_eq!(w.bytes(OUT + 16, 4), vec![0xcc; 4]);
    assert_eq!(w.call("XOnlineTaskContinue", &[logon]), CONNECTED);
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), users);
    assert_eq!(w.bytes(users, 8), a.xuid.0.to_le_bytes());
    assert_eq!(server.presence()[0].xuid, a.xuid);
}

#[test]
fn all_selected_accounts_are_verified_before_any_presence_is_published() {
    let server = Server::new();
    let a = server.account("Exists");
    let unknown = Account {
        xuid: Xuid(0x00090000ffffffff),
        tag: "Absent".into(),
        key: Arc::new(KeyPair::generate()),
    };
    let mut w = World::new(&server.embedded.url, &[&a, &unknown]);
    let h = w.begin(&[0, 1]);
    assert_eq!(w.wait(h), INVALID_USER);
    assert!(server.presence().is_empty());
    assert!(!server.events().iter().any(|e| e.kind == types::SIGNED_IN));
}

#[test]
fn every_selected_account_is_signed_in_pinged_and_signed_out() {
    let server = Server::new();
    let a = server.account("ControllerOne");
    let b = server.account("ControllerTwo");
    let mut w = World::new(&server.embedded.url, &[&a, &b]);
    let h = w.begin(&[0, 1]);
    assert_eq!(w.wait(h), CONNECTED);
    assert_eq!(server.presence().len(), 2);
    server.until(|| w.hle.server_status().acknowledged_pings >= 2);
    assert_eq!(w.call("XOnlineTaskClose", &[h]), 0);
    server.until(|| server.presence().is_empty());
    assert_eq!(
        server
            .events()
            .iter()
            .filter(|e| e.kind == types::SIGNED_IN)
            .count(),
        2
    );
    assert_eq!(
        server
            .events()
            .iter()
            .filter(|e| e.kind == types::SIGNED_OUT)
            .count(),
        2
    );
}

#[test]
fn heartbeat_failure_reaches_the_persistent_connection_task() {
    let server = Server::new();
    let a = server.account("Disconnected");
    let mut w = World::new(&server.embedded.url, &[&a]);
    let h = w.logon();
    drop(server);
    let deadline = Instant::now() + Duration::from_secs(4);
    while w.hle.server_status().last_error == 0 {
        assert!(
            Instant::now() < deadline,
            "heartbeat never detected disconnect"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), CONNECTION_LOST);
    assert_eq!(w.call("XOnlineLogonTaskGetResults", &[h]), CONNECTION_LOST);
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
}

fn synthetic_account() -> Account {
    Account {
        xuid: Xuid(0x0009000000000001),
        tag: "Synthetic".into(),
        key: Arc::new(KeyPair::generate()),
    }
}
fn receive_headers(socket: &mut std::net::TcpStream) -> Vec<u8> {
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        bytes.push(byte[0]);
        assert!(bytes.len() < 16384);
    }
    bytes
}
fn http_response(socket: &mut std::net::TcpStream, status: &str, extra: &str, body: &[u8]) {
    write!(
        socket,
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{extra}\r\n",
        body.len()
    )
    .unwrap();
    socket.write_all(body).unwrap();
}

#[test]
fn bearer_redirects_and_cross_origin_discovery_are_rejected() {
    for redirect in [false, true] {
        let destination = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        destination.set_nonblocking(true).unwrap();
        let stolen = format!("http://{}", destination.local_addr().unwrap());
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let api = if redirect {
            format!("{url}/v1")
        } else {
            format!("{stolen}/v1")
        };
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let header = receive_headers(&mut socket);
            assert!(header.starts_with(b"GET /.well-known/h2live.json "));
            let discovery = json!({"id":"test-server", "api":api, "keys":[], "software":"test", "federation":false});
            http_response(
                &mut socket,
                "200 OK",
                "Content-Type: application/json\r\n",
                &serde_json::to_vec(&discovery).unwrap(),
            );
            drop(socket);
            if redirect {
                let (mut socket, _) = listener.accept().unwrap();
                let header = receive_headers(&mut socket);
                assert!(header.starts_with(b"GET /v1/accounts/by-xuid/"));
                assert!(
                    String::from_utf8(header)
                        .unwrap()
                        .to_ascii_lowercase()
                        .contains("authorization: bearer ")
                );
                http_response(
                    &mut socket,
                    "302 Found",
                    &format!("Location: {stolen}/stolen\r\n"),
                    b"",
                );
            }
        });
        let account = synthetic_account();
        let mut w = World::new(&url, &[&account]);
        let h = w.begin(&[0]);
        assert_eq!(w.wait(h), CANNOT_ACCESS_SERVICE);
        thread.join().unwrap();
        assert!(
            matches!(destination.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock)
        );
    }
}

#[test]
fn unsafe_base_urls_fail_without_a_live_connection() {
    let account = synthetic_account();
    for url in [
        "http://example.invalid",
        "file:///accounts",
        "https://user:password@example.invalid",
        "https://example.invalid/?key=secret",
    ] {
        let mut w = World::new(url, &[&account]);
        let h = w.begin(&[0]);
        assert_eq!(w.wait(h), CANNOT_ACCESS_SERVICE);
    }
}

#[test]
fn closing_pending_logon_is_nonblocking_and_publishes_no_users() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (entered, seen) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        receive_headers(&mut socket);
        entered.send(()).unwrap();
        wait.recv_timeout(Duration::from_secs(3)).unwrap();
        http_response(&mut socket, "503 Service Unavailable", "", b"");
    });
    let account = synthetic_account();
    let mut w = World::new(&url, &[&account]);
    let h = w.begin(&[0]);
    seen.recv_timeout(Duration::from_secs(2)).unwrap();
    let start = Instant::now();
    assert_eq!(w.call("XOnlineTaskClose", &[h]), 0);
    assert!(start.elapsed() < Duration::from_millis(100));
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), INVALID_ARGUMENT);
    release.send(()).unwrap();
    thread.join().unwrap();
}

#[test]
fn exhausted_guest_storage_does_not_start_a_server_user_change() {
    let server = Server::new();
    let a = server.account("Allocated");
    let b = server.account("Unallocated");
    let mut w = World::timed(&server.embedded.url, &[&a, &b], Duration::from_secs(30));
    w.logon();
    assert_eq!(w.call("XOnlinePresenceInit", &[0, 0, OUT]), 0);
    let presence = w.u32(OUT);
    assert_eq!(w.wait(presence), TASK_SUCCESS);
    let mut allocations = 0;
    loop {
        let result = w.call("XOnlineFriendsEnumerate", &[0, 0, OUT]);
        if result != 0 {
            assert_eq!(result, OUT_OF_MEMORY);
            break;
        }
        allocations += 1;
        assert!(allocations < 10000);
    }
    w.put(USERS, &[0; 448]);
    w.put(USERS, &w.bytes(DATA + 112, 112));
    w.word(OUT, 0xdeadbeef);
    assert_eq!(
        w.call("XOnlineChangeLogonUsers", &[USERS, 0, OUT]),
        OUT_OF_MEMORY
    );
    assert_eq!(w.u32(OUT), 0xdeadbeef);
    // This acknowledged request is ordered after the rejected change. An
    // orphaned change would remove account A and make this presence task fail.
    assert_eq!(
        w.call("XOnlineNotificationSetState", &[0, 0, 0, 0, 0, 0]),
        0
    );
    assert_eq!(w.call("XOnlinePresenceSubmit", &[presence]), 0);
    assert_eq!(w.wait(presence), TASK_SUCCESS);
    let current = server.presence();
    assert_eq!(current.len(), 1);
    assert_eq!(current[0].xuid, a.xuid);
    assert_eq!(current[0].state, "away");
}

#[test]
fn final_transport_cleanup_and_hle_drop_send_signed_out() {
    let server = Server::new();
    let account = server.account("Cleanup");
    for cleanup in [Some("WSACleanup"), Some("XNetCleanup"), None] {
        let mut w = World::timed(&server.embedded.url, &[&account], Duration::from_secs(30));
        w.logon();
        if let Some(api) = cleanup {
            assert_eq!(w.call(api, &[]), 0);
            assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
        }
        drop(w);
        server.until(|| server.presence().is_empty());
    }
    assert_eq!(
        server
            .events()
            .iter()
            .filter(|e| e.kind == types::SIGNED_OUT)
            .count(),
        3
    );
}

#[test]
fn attachment_replacement_is_fetched_and_overflow_never_copies_a_prefix() {
    let server = Server::new();
    let account = server.account("ChangedFile");
    server.publish("message_of_the_day.dat", &motd());
    let mut w = World::new(&server.embedded.url, &[&account]);
    w.logon();
    let details = w.details(1);
    server.publish(
        "message_of_the_day.dat",
        &vec![0x5a; ATTACHMENT_CAP as usize + 1],
    );
    w.put(BUFFER, &vec![0xcc; ATTACHMENT_CAP as usize + 4]);
    assert_eq!(
        w.call(
            "XOnlineMessageDownloadAttachmentToMemory",
            &[details, 0x9c1, BUFFER, ATTACHMENT_CAP, 0, OUT]
        ),
        0
    );
    let download = w.u32(OUT);
    assert_eq!(w.wait(download), BUFFER_TOO_SMALL);
    assert_eq!(
        w.bytes(BUFFER, ATTACHMENT_CAP as usize + 4),
        vec![0xcc; ATTACHMENT_CAP as usize + 4]
    );
    w.put(OUT, &[0xdd; 12]);
    assert_eq!(
        w.call(
            "XOnlineMessageDownloadAttachmentToMemoryGetResults",
            &[download, OUT, OUT + 4, OUT + 8]
        ),
        BUFFER_TOO_SMALL
    );
    assert_eq!(w.bytes(OUT, 12), vec![0xdd; 12]);
}
