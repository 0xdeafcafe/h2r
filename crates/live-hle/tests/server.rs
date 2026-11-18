//! Signed requests against the real embedded memory-backed service. Guest
//! frames and all files are synthetic; no XBE, image build, or game execution.
use live_hle::{
    Hle, LiveConfig, NetworkConfig, SUMMARY_SIZE, ServerOptions, entries::ENTRIES, status::*,
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
    /// GET as the operator (an admin): friends-only and invite-only
    /// sessions' events and presence are hidden from anonymous readers.
    fn get_as_operator<T: serde::de::DeserializeOwned>(&self, path: &str) -> T {
        let token = sign::issue_token(
            &self.operator,
            &format!("server:{}", self.discovery.id),
            &self.discovery.id,
            lxm::READ,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        );
        serde_json::from_reader(
            self.agent
                .get(&self.url(path))
                .set("authorization", &format!("Bearer {token}"))
                .call()
                .unwrap()
                .into_reader(),
        )
        .unwrap()
    }
    fn events(&self) -> Vec<WireEvent> {
        self.get_as_operator::<Page<WireEvent>>("/v1/events?from=0&limit=1000")
            .items
    }
    fn presence(&self) -> Vec<Presence> {
        self.get_as_operator::<Page<Presence>>("/v1/presence").items
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
        Self::raw_network(url, accounts, heartbeat, 31000)
    }
    fn raw_network(url: &str, accounts: Value, heartbeat: Duration, port: u16) -> Self {
        let path = std::env::temp_dir().join(format!("h2-hle-accounts-{}.json", sign::random_id()));
        std::fs::write(&path, serde_json::to_vec(&accounts).unwrap()).unwrap();
        let hle = Hle::with_server_options(
            LiveConfig {
                enabled: true,
                accounts_path: Some(path.clone()),
                server_url: Some(url.into()),
                offer_new_account: false,
            },
            NetworkConfig::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)),
            ServerOptions {
                request_timeout: Duration::from_secs(2),
                heartbeat_interval: heartbeat,
                ..Default::default()
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
        guest.map_zeroed(DATA, 0x40000, Permissions::RW).unwrap();
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
    fn raw_call(&mut self, name: &str, args: &[u32]) -> runtime::TransferAbi {
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
        Runner::default().run(&mut self.cpu, &mut self.ctx, e.va)
    }
    fn call(&mut self, name: &str, args: &[u32]) -> u32 {
        let t = self.raw_call(name, args);
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
    fn storage_path(&mut self, prefix: &str, file: &str) -> u32 {
        let relative = format!("{prefix}/{file}");
        self.put(
            DATA + 0x4000,
            &relative
                .encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        self.word(OUT, 256);
        assert_eq!(
            self.call(
                "XOnlineStorageCreateServerPath",
                &[4, 0, 0, 0, 0, DATA + 0x4000, DATA + 0x4200, OUT]
            ),
            0
        );
        let expected = format!("//title.4d530064/t:4d530064/{relative}");
        assert_eq!(
            self.bytes(DATA + 0x4200, self.u32(OUT) as usize * 2),
            expected
                .encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        );
        DATA + 0x4200
    }
    fn storage_start(&mut self, path: u32, capacity: u32) -> u32 {
        assert_eq!(
            self.call(
                "XOnlineStorageDownloadToMemory",
                &[4, 0, path, BUFFER, capacity, 0, 0, OUT]
            ),
            0
        );
        self.u32(OUT)
    }
    fn storage_results(&mut self, task: u32) -> u32 {
        self.call(
            "XOnlineStorageDownloadToMemoryGetResults",
            &[task, OUT, OUT + 4, OUT + 8, OUT + 12, OUT + 20],
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
fn title_storage_files_reach_both_completion_handlers_with_their_real_caps() {
    let server = Server::new();
    let a = server.account("Reader");
    let motd_file = motd();
    let hopper_file = live_proto::hopper::HopperFile::blank().build();
    server.publish("message_of_the_day.dat", &motd_file);
    server.publish("multiplayer_hopper.dat", &hopper_file);
    let mut w = World::new(&server.embedded.url, &[&a]);
    w.logon();
    assert!(
        w.messages().is_empty(),
        "title resources are not player messages"
    );
    for (name, file, cap, inflated_size) in [
        ("message_of_the_day.dat", &motd_file, 0x8000u32, 0x25224),
        ("multiplayer_hopper.dat", &hopper_file, 0x20000u32, 0x162cc0),
    ] {
        for prefix in ["9809", "test"] {
            let path = w.storage_path(prefix, name);
            w.put(BUFFER, &vec![0xcc; cap as usize + 4]);
            let task = w.storage_start(path, cap);
            assert_eq!(w.wait(task), TASK_SUCCESS);
            w.put(OUT, &[0xdd; 32]);
            assert_eq!(w.storage_results(task), 0);
            let ptr = w.u32(OUT);
            let received = w.u32(OUT + 4);
            assert_eq!(ptr, BUFFER);
            assert_eq!(received, file.len() as u32);
            assert_eq!(received, w.u32(OUT + 8));
            assert_eq!(
                w.bytes(OUT + 12, 16),
                vec![0; 16],
                "shared title owner and unknown creation date"
            );
            assert_eq!(w.bytes(OUT + 28, 4), vec![0xdd; 4]);
            let bytes = w.bytes(ptr, received as usize);
            assert_eq!(&bytes, file);
            assert_eq!(w.bytes(BUFFER + received, 4), vec![0xcc; 4]);
            assert_eq!(
                u32::from_be_bytes(bytes[16..20].try_into().unwrap()),
                inflated_size
            );
            let mut inflated = Vec::new();
            flate2::read::ZlibDecoder::new(&bytes[20..])
                .read_to_end(&mut inflated)
                .unwrap();
            assert_eq!(inflated.len(), inflated_size as usize);
            if name == "message_of_the_day.dat" {
                assert_eq!(Motd::parse(&bytes).unwrap().header, "Welcome");
            } else {
                live_proto::hopper::HopperFile::parse(&bytes).unwrap();
            }
            assert_eq!(
                w.call("XOnlineStorageGetProgress", &[task, OUT, OUT + 4, OUT + 12]),
                TASK_SUCCESS
            );
            assert_eq!(w.u32(OUT), 100);
            assert_eq!(w.bytes(OUT + 4, 8), u64::from(received).to_le_bytes());
            assert_eq!(w.bytes(OUT + 12, 8), u64::from(received).to_le_bytes());
            assert_eq!(
                w.call(
                    "XOnlineStorageDownloadToMemoryGetResults",
                    &[task, 0, 0, 0, 0, 0]
                ),
                0
            );
            // A repeated pump/result read does not copy again into reused memory.
            w.put(BUFFER, &[0x12; 4]);
            assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
            assert_eq!(w.bytes(BUFFER, 4), vec![0x12; 4]);
            assert_eq!(w.call("XOnlineTaskClose", &[task]), 0);
            assert_eq!(
                w.call(
                    "XOnlineStorageDownloadToMemoryGetResults",
                    &[task, 0, 0, 0, 0, 0]
                ),
                INVALID_ARGUMENT
            );
        }
    }
    assert!(w.messages().is_empty());
    w.put(DATA, &[0xcc; 9]);
    w.word(OUT, u32::MAX);
    assert_eq!(w.call("XOnlineGetNotificationEx", &[0, DATA, OUT]), 0);
    assert_eq!(w.bytes(DATA, 9), vec![0; 9]);
    assert_eq!(w.u32(OUT), 0);
}
#[test]
fn storage_404_and_unrecognized_paths_preserve_buffers_and_optional_results() {
    let server = Server::new();
    let account = server.account("MissingFile");
    let mut w = World::new(&server.embedded.url, &[&account]);
    w.logon();
    let path = w.storage_path("9809", "message_of_the_day.dat");
    let task = w.storage_start(path, 32);
    assert_eq!(w.wait(task), STORAGE_FILE_NOT_FOUND);
    w.put(OUT, &[0xdd; 32]);
    assert_eq!(w.storage_results(task), STORAGE_FILE_NOT_FOUND);
    assert_eq!(w.bytes(OUT, 32), vec![0xdd; 32]);

    server.publish("message_of_the_day.dat", &motd());
    for (facility, path) in [
        (4, "//title.4d530064/t:4d530064/message_of_the_day.dat"),
        (4, "//title.4d530064/t:4d530064/9810/message_of_the_day.dat"),
        (
            4,
            "//title.4d530064/t:4d530064/9809/../message_of_the_day.dat",
        ),
        (
            4,
            "//title.4d530064/t:4d530064/9809/message_of_the_day.dat/extra",
        ),
        (4, "//title.ffffffff/t:4d530064/9809/message_of_the_day.dat"),
        (4, "//title.4d530064/t:4d530064/test/other.dat"),
        (5, "//title.4d530064/t:4d530064/9809/message_of_the_day.dat"),
    ] {
        w.put(
            DATA + 0x4200,
            &path
                .encode_utf16()
                .chain([0])
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
        w.put(BUFFER, &[0xcc; 32]);
        assert_eq!(
            w.call(
                "XOnlineStorageDownloadToMemory",
                &[facility, 0, DATA + 0x4200, BUFFER, 32, 0, 0, OUT]
            ),
            0
        );
        let h = w.u32(OUT);
        assert_eq!(w.wait(h), STORAGE_FILE_NOT_FOUND);
        assert_eq!(w.bytes(BUFFER, 32), vec![0xcc; 32]);
        w.put(OUT, &[0xdd; 32]);
        assert_eq!(w.storage_results(h), STORAGE_FILE_NOT_FOUND);
        assert_eq!(w.bytes(OUT, 32), vec![0xdd; 32]);
    }
}
#[test]
fn title_files_are_never_inbox_messages_and_stale_message_handles_are_invalid() {
    let server = Server::new();
    let account = server.account("NotMail");
    server.publish("message_of_the_day.dat", &motd());
    let mut w = World::new(&server.embedded.url, &[&account]);
    w.logon();
    w.put(DATA, &[0xcc; 64]);
    assert!(w.messages().is_empty());
    assert_eq!(w.bytes(DATA, 64), vec![0xcc; 64]);
    for id in [1, 4] {
        w.word(OUT, 0xdeadbeef);
        assert_eq!(
            w.call("XOnlineMessageDetails", &[0, id, 0x10, 0, 0, OUT]),
            MESSAGE_INVALID_ID
        );
        assert_eq!(w.u32(OUT), 0xdeadbeef);
        assert_eq!(
            w.call("XOnlineMessageDelete", &[0, id, 0]),
            MESSAGE_INVALID_ID
        );
    }
    for name in [
        "XOnlineMessageDetailsGetResultsProperty",
        "XOnlineMessageDownloadAttachmentToMemory",
    ] {
        assert_eq!(
            w.call(name, &[0x7fff0000, 0x9c1, 2048, DATA, OUT, 0]),
            INVALID_ARGUMENT
        );
    }
    assert_eq!(
        w.call(
            "XOnlineMessageDownloadAttachmentToMemoryGetResults",
            &[0x7fff0000, OUT, OUT + 4, OUT + 8]
        ),
        INVALID_ARGUMENT
    );
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
fn storage_reads_current_publications_without_logon_and_obeys_each_callers_capacity() {
    let server = Server::new();
    let account = server.account("UpdatedFile");
    let mut w = World::new(&server.embedded.url, &[&account]);
    w.logon();
    // Publishing after sign-in is visible to a new explicit storage request:
    // logon no longer caches a message catalog.
    let file = vec![0x5a; 0x8000];
    server.publish("message_of_the_day.dat", &file);
    let path = w.storage_path("9809", "message_of_the_day.dat");
    w.put(BUFFER, &vec![0xcc; 0x8004]);
    let h = w.storage_start(path, 0x8000);
    assert_eq!(w.wait(h), TASK_SUCCESS);
    assert_eq!(w.storage_results(h), 0);
    assert_eq!(w.bytes(BUFFER, file.len()), file);
    assert_eq!(w.bytes(BUFFER + 0x8000, 4), vec![0xcc; 4]);
    server.publish("message_of_the_day.dat", &vec![0x12; 0x8001]);
    for cap in [0, 32, 0x8000] {
        w.put(BUFFER, &vec![0xcc; 0x8004]);
        let h = w.storage_start(path, cap);
        assert_eq!(w.wait(h), BUFFER_TOO_SMALL);
        assert_eq!(w.bytes(BUFFER, 0x8004), vec![0xcc; 0x8004]);
        w.put(OUT, &[0xdd; 32]);
        assert_eq!(w.storage_results(h), BUFFER_TOO_SMALL);
        assert_eq!(w.bytes(OUT, 32), vec![0xdd; 32]);
    }
    server.publish("multiplayer_hopper.dat", &vec![0x34; 0x20000]);
    let path = w.storage_path("test", "multiplayer_hopper.dat");
    let h = w.storage_start(path, 0x20000);
    assert_eq!(w.wait(h), TASK_SUCCESS);
    assert_eq!(w.storage_results(h), 0);
    assert_eq!(w.u32(OUT + 4), 0x20000);
    assert_eq!(w.bytes(BUFFER, 0x20000), vec![0x34; 0x20000]);
    server.publish("multiplayer_hopper.dat", &vec![0x56; 0x20001]);
    w.put(BUFFER, &vec![0xcc; 0x20004]);
    let h = w.storage_start(path, 0x20000);
    assert_eq!(w.wait(h), BUFFER_TOO_SMALL);
    assert_eq!(w.bytes(BUFFER, 0x20004), vec![0xcc; 0x20004]);
}
#[test]
fn storage_transport_failures_complete_as_service_errors_and_cleanup_invalidates_handles() {
    let server = Server::new();
    let account = server.account("OfflineStorage");
    let mut w = World::timed(&server.embedded.url, &[&account], Duration::from_secs(3600));
    w.logon();
    let path = w.storage_path("9809", "message_of_the_day.dat");
    drop(server);
    w.put(BUFFER, &[0xcc; 32]);
    let h = w.storage_start(path, 32);
    assert_eq!(w.wait(h), CANNOT_ACCESS_SERVICE);
    assert_eq!(w.bytes(BUFFER, 32), vec![0xcc; 32]);
    assert_eq!(w.call("XOnlineCleanup", &[]), 0);
    assert_eq!(w.call("XOnlineStartup", &[0]), 0);
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), INVALID_ARGUMENT);
}

#[test]
fn storage_poll_rechecks_guest_memory_and_result_outputs_are_atomic() {
    let server = Server::new();
    let account = server.account("CheckedBuffers");
    let file = motd();
    server.publish("message_of_the_day.dat", &file);
    let mut w = World::new(&server.embedded.url, &[&account]);
    w.logon();
    let path = w.storage_path("9809", "message_of_the_day.dat");
    let h = w.storage_start(path, 0x8000);
    w.rt.memory().unmap(BUFFER, 0x1000).unwrap();
    let t = w.raw_call("XOnlineTaskContinue", &[h]);
    assert!(matches!(
        w.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    w.rt.memory()
        .map_zeroed(BUFFER, 0x1000, Permissions::RW)
        .unwrap();
    assert_eq!(w.wait(h), TASK_SUCCESS);
    assert_eq!(w.bytes(BUFFER, file.len()), file);
    w.put(OUT, &[0xdd; 32]);
    let t = w.raw_call(
        "XOnlineStorageDownloadToMemoryGetResults",
        &[h, OUT, OUT + 4, 0xdead0000, OUT + 12, OUT + 20],
    );
    assert!(matches!(
        w.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(w.bytes(OUT, 32), vec![0xdd; 32]);
    assert_eq!(w.storage_results(h), 0);
    assert_eq!(w.u32(OUT), BUFFER);
    assert_eq!(w.u32(OUT + 4), file.len() as u32);
}
#[test]
fn storage_pending_pumps_results_and_close_never_wait_for_http() {
    let server = Server::new();
    let account = server.account("SlowStorage");
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let upstream = server.embedded.url.clone();
    let mut discovery = serde_json::to_value(&server.discovery).unwrap();
    discovery["api"] = json!(format!("{url}/v1"));
    let (entered, seen) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    // Forward the three logon requests, then deliberately hold the file GET.
    let proxy = std::thread::spawn(move || {
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(3))
            .build();
        for _ in 0..4 {
            let (mut socket, _) = listener.accept().unwrap();
            let header = String::from_utf8(receive_headers(&mut socket)).unwrap();
            let mut lines = header.lines();
            let first = lines.next().unwrap().split_whitespace().collect::<Vec<_>>();
            let method = first[0];
            let path = first[1];
            let fields = lines
                .filter_map(|s| s.split_once(':'))
                .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string()))
                .collect::<std::collections::BTreeMap<_, _>>();
            let mut body = vec![
                0;
                fields
                    .get("content-length")
                    .map_or(0, |s| s.parse::<usize>().unwrap())
            ];
            socket.read_exact(&mut body).unwrap();
            if path == "/.well-known/h2live.json" {
                http_response(
                    &mut socket,
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    &serde_json::to_vec(&discovery).unwrap(),
                );
            } else if path.contains("/titles/4D530064/files/") {
                entered.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                http_response(&mut socket, "503 Service Unavailable", "", b"");
            } else {
                let mut req = agent.request(method, &format!("{upstream}{path}"));
                for key in ["authorization", "content-type"] {
                    if let Some(v) = fields.get(key) {
                        req = req.set(key, v);
                    }
                }
                let response = if body.is_empty() {
                    req.call()
                } else {
                    req.send_bytes(&body)
                }
                .unwrap();
                let mut result = Vec::new();
                response.into_reader().read_to_end(&mut result).unwrap();
                http_response(
                    &mut socket,
                    "200 OK",
                    "Content-Type: application/json\r\n",
                    &result,
                );
            }
        }
    });
    let mut w = World::timed(&url, &[&account], Duration::from_secs(3600));
    w.logon();
    let path = w.storage_path("9809", "message_of_the_day.dat");
    w.put(BUFFER, &[0xcc; 32]);
    let start = Instant::now();
    let h = w.storage_start(path, 32);
    assert!(start.elapsed() < Duration::from_millis(200));
    seen.recv_timeout(Duration::from_secs(2)).unwrap();
    for _ in 0..3 {
        w.put(OUT, &[0xdd; 32]);
        let start = Instant::now();
        assert_eq!(w.call("XOnlineTaskContinue", &[h]), 0);
        assert_eq!(w.storage_results(h), TASK_BUSY);
        assert!(start.elapsed() < Duration::from_millis(100));
        assert_eq!(w.bytes(OUT, 32), vec![0xdd; 32]);
        assert_eq!(w.bytes(BUFFER, 32), vec![0xcc; 32]);
    }
    let start = Instant::now();
    assert_eq!(w.call("XOnlineTaskClose", &[h]), 0);
    assert!(start.elapsed() < Duration::from_millis(100));
    release.send(()).unwrap();
    proxy.join().unwrap();
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), INVALID_ARGUMENT);
    assert_eq!(
        w.bytes(BUFFER, 32),
        vec![0xcc; 32],
        "a closed request cannot copy late"
    );
}

fn clan_properties(w: &World, name: &str) {
    let mut b = vec![0; 1670];
    for (p, u) in b[..32]
        .as_chunks_mut::<2>()
        .0
        .iter_mut()
        .zip(name.encode_utf16())
    {
        p.copy_from_slice(&u.to_le_bytes());
    }
    w.put(DATA + 0x4000, &b);
    w.put(DATA + 0x4800, &[0; 106]);
    w.word(DATA + 0x4800, u32::MAX);
}
#[test]
fn teams_two_guests_create_recruit_accept_list_edit_member_leave_and_delete_on_the_server() {
    let server = Server::new();
    let alpha = server.account("Alpha");
    let bravo = server.account("Bravo");
    let mut a = World::new(&server.embedded.url, &[&alpha]);
    let mut b = World::new(&server.embedded.url, &[&bravo]);
    a.logon();
    b.logon();
    clan_properties(&a, "HLE Clan");
    assert_eq!(
        a.call(
            "XOnlineTeamCreate",
            &[0, DATA + 0x4000, DATA + 0x4800, 100, 0, OUT]
        ),
        0
    );
    let task = a.u32(OUT);
    assert_eq!(a.wait(task), TASK_SUCCESS);
    let id = a.hle.local_teams().of(alpha.xuid)[0].id;
    let lo = id.0 as u32;
    let hi = (id.0 >> 32) as u32;
    a.word(DATA + 0x4800, 0x10);
    a.put(DATA + 0x4900, b"Bravo\0");
    // Bravo is not in Alpha's host accounts file: worker resolves the name.
    assert_eq!(
        a.call(
            "XOnlineTeamMemberRecruitByName",
            &[0, lo, hi, 0, DATA + 0x4900, DATA + 0x4800, 0, 0, OUT]
        ),
        0
    );
    assert_eq!(a.wait(a.u32(OUT)), TASK_SUCCESS);
    assert_eq!(
        b.call(
            "XOnlineTeamEnumerateByUserXUID",
            &[
                0,
                bravo.xuid.0 as u32,
                (bravo.xuid.0 >> 32) as u32,
                0,
                0,
                OUT
            ]
        ),
        0
    );
    let list = b.u32(OUT);
    assert_eq!(b.wait(list), TASK_SUCCESS);
    assert_eq!(
        b.call(
            "XOnlineTeamEnumerateGetResults",
            &[list, OUT, DATA + 0x5000]
        ),
        0
    );
    assert_eq!(b.u32(OUT), 1);
    assert_eq!(
        b.call("XOnlineTeamGetDetails", &[list, lo, hi, 0, BUFFER]),
        0
    );
    assert_eq!(b.u32(BUFFER + 1682), 1);
    assert_eq!(
        b.call("XOnlineTeamMemberAnswerRecruit", &[0, lo, hi, 0, 0, 0, OUT]),
        0
    );
    assert_eq!(b.wait(b.u32(OUT)), TASK_SUCCESS);
    // The same remote recruit path also resolves a XUID absent from our file.
    assert_eq!(
        a.call(
            "XOnlineTeamMemberRecruit",
            &[
                0,
                lo,
                hi,
                0,
                bravo.xuid.0 as u32,
                (bravo.xuid.0 >> 32) as u32,
                0,
                DATA + 0x4800,
                0,
                0,
                OUT
            ]
        ),
        0
    );
    assert_eq!(a.wait(a.u32(OUT)), TASK_SUCCESS);
    assert_eq!(
        a.call(
            "XOnlineTeamMemberRecruit",
            &[0, lo, hi, 0, 0x777, 0x0009, 0, DATA + 0x4800, 0, 0, OUT]
        ),
        0
    );
    assert_eq!(
        a.wait(a.u32(OUT)),
        live_proto::team::TeamError::UserNotFound.hresult()
    );
    assert_eq!(
        b.call("XOnlineTeamMemberAnswerRecruit", &[0, lo, hi, 0, 1, 0, OUT]),
        0
    );
    assert_eq!(b.wait(b.u32(OUT)), TASK_SUCCESS);
    assert_eq!(b.call("XOnlineTeamDelete", &[0, lo, hi, 0, 0, OUT]), 0);
    assert_eq!(
        b.wait(b.u32(OUT)),
        live_proto::team::TeamError::Privileges.hresult()
    );
    a.word(DATA + 0x4800, 0x1c);
    assert_eq!(
        a.call(
            "XOnlineTeamMemberSetProperties",
            &[
                0,
                lo,
                hi,
                0,
                bravo.xuid.0 as u32,
                (bravo.xuid.0 >> 32) as u32,
                0,
                DATA + 0x4800,
                0,
                OUT
            ]
        ),
        0
    );
    assert_eq!(a.wait(a.u32(OUT)), TASK_SUCCESS);
    assert_eq!(
        b.call("XOnlineTeamMembersEnumerate", &[0, lo, hi, 0, 1, 0, OUT]),
        0
    );
    let members = b.u32(OUT);
    assert_eq!(b.wait(members), TASK_SUCCESS);
    assert_eq!(
        b.call(
            "XOnlineTeamMembersEnumerateGetResults",
            &[members, OUT, DATA + 0x5000]
        ),
        0
    );
    assert_eq!(b.u32(OUT), 2);
    assert_eq!(
        b.call(
            "XOnlineTeamMemberGetDetails",
            &[
                members,
                bravo.xuid.0 as u32,
                (bravo.xuid.0 >> 32) as u32,
                0,
                BUFFER
            ]
        ),
        0
    );
    assert_eq!(b.u32(BUFFER + 28), 0x1c);
    assert_eq!(
        b.call(
            "XOnlineTeamMemberRemove",
            &[
                0,
                lo,
                hi,
                0,
                bravo.xuid.0 as u32,
                (bravo.xuid.0 >> 32) as u32,
                0,
                0,
                OUT
            ]
        ),
        0
    );
    assert_eq!(b.wait(b.u32(OUT)), TASK_SUCCESS);
    assert_eq!(a.call("XOnlineTeamDelete", &[0, lo, hi, 0, 0, OUT]), 0);
    assert_eq!(a.wait(a.u32(OUT)), TASK_SUCCESS);
    let events = server.events();
    assert!(events.iter().any(|e| e.kind == "TeamCreated"));
    assert!(events.iter().any(|e| e.kind == "TeamRecruitReceived"));
    assert!(events.iter().any(|e| e.kind == "TeamDeleted"));
}
#[test]
fn teams_server_failure_finishes_a_task_without_mutating_the_local_cache() {
    let server = Server::new();
    let account = server.account("Alpha");
    let mut w = World::timed(&server.embedded.url, &[&account], Duration::from_secs(3600));
    w.logon();
    drop(server);
    clan_properties(&w, "Unavailable");
    assert_eq!(
        w.call(
            "XOnlineTeamCreate",
            &[0, DATA + 0x4000, DATA + 0x4800, 100, 0, OUT]
        ),
        0
    );
    assert_eq!(w.wait(w.u32(OUT)), CANNOT_ACCESS_SERVICE);
    assert!(w.hle.local_teams().teams.is_empty());
}

/// The host's advertisement says exactly which map it has loaded when the
/// boot's map probe names a package's `MapId` (docs/game/maps.md §5).
#[test]
fn advertisements_carry_the_hosts_map_id() {
    let s = Server::new();
    let a = s.account("Host");
    let mut host = World::new(&s.embedded.url, &[&a]);
    host.logon();
    let id = live_proto::maps::MapId([0x6d; 32]);
    host.hle
        .set_map_probe(Some(std::sync::Arc::new(move |_: &runtime::Guest| Some(id))));
    attr(&host, MATCH_ATTR, 1, 101);
    assert_eq!(
        host.call(
            "XOnlineMatchSessionCreate",
            &[1, 7, 0, 0, 1, MATCH_ATTR, 0, OUT]
        ),
        0
    );
    let create = host.u32(OUT);
    assert_eq!(host.wait(create), TASK_SUCCESS);
    assert_eq!(host.hle.local_sessions()[0].map_id, Some(id));
    let created = s
        .events()
        .into_iter()
        .find(|e| e.kind == "SessionCreated")
        .unwrap();
    assert_eq!(created.data["advertisement"]["map_id"], id.to_string());
}

// The same nine match entry points the retail wrappers drive, over two separate
// guest memories and the actual HTTP worker. Values and frames are synthetic.
const MATCH_ATTR: u32 = DATA + 0x5000;
const MATCH_ID: u32 = DATA + 0x5800;
const MATCH_KEY: u32 = DATA + 0x5820;
const MATCH_SPEC: u32 = DATA + 0x5900;
fn attr(w: &World, at: u32, id: u32, value: u64) {
    w.word(at, id);
    w.word(at + 4, 1);
    w.put(at + 8, &value.to_le_bytes());
}
fn match_search(w: &mut World, procedure: u32, params: u32, cap: u32) -> (u32, u32, u32) {
    assert_eq!(
        w.call(
            "XOnlineMatchSearch",
            &[procedure, 50, params, MATCH_ATTR, cap, 0, OUT]
        ),
        0
    );
    let task = w.u32(OUT);
    assert_eq!(w.wait(task), TASK_SUCCESS);
    assert_eq!(
        w.call("XOnlineMatchSearchGetResults", &[task, OUT, OUT + 4]),
        0
    );
    (task, w.u32(OUT), w.u32(OUT + 4))
}
#[test]
fn sessions_two_guests_publish_find_parse_update_and_delete() {
    let s = Server::new();
    let a = s.account("Host");
    let b = s.account("Joiner");
    let mut host = World::new(&s.embedded.url, &[&a]);
    let mut peer = World::raw_network(
        &s.embedded.url,
        json!([b.json()]),
        Duration::from_millis(100),
        32000,
    );
    host.logon();
    peer.logon();
    for (i, id) in [1, 2, 3, 5, 6, 7, 8].into_iter().enumerate() {
        attr(&host, MATCH_ATTR + i as u32 * 16, id, 100 + u64::from(id));
    }
    assert_eq!(
        host.call(
            "XOnlineMatchSessionCreate",
            &[1, 7, 0, 0, 7, MATCH_ATTR, 0, OUT]
        ),
        0
    );
    let create = host.u32(OUT);
    assert!(
        host.hle.local_sessions().is_empty(),
        "not cached until the guest polls completion"
    );
    assert_eq!(host.wait(create), TASK_SUCCESS);
    assert_eq!(
        host.call("XOnlineMatchSessionGetInfo", &[create, MATCH_ID, MATCH_KEY]),
        0
    );
    let id = host.bytes(MATCH_ID, 8);
    let key = host.bytes(MATCH_KEY, 16);
    assert_eq!(id[0] & 0xf0, 0x80);
    assert_ne!(key, vec![0; 16]);
    let lo = host.u32(MATCH_ID);
    let hi = host.u32(MATCH_ID + 4);
    assert_eq!(host.hle.local_sessions()[0].id.to_vec(), id);
    let rows: Page<live_proto::session::Session> = s.get_as_operator("/v1/sessions");
    assert_eq!(rows.items.len(), 1);
    assert_eq!(rows.items[0].xnkid, sign::hex(&id));
    assert_eq!(rows.items[0].endpoint.port, 31000);
    let wire_id = rows.items[0].session_id.clone();
    let cap = peer.call("XOnlineMatchSearchResultsLen", &[50, 7, MATCH_SPEC]);
    let (search, table, count) = match_search(&mut peer, 1, 0, cap);
    assert_eq!(count, 1);
    let row = peer.u32(table);
    assert_eq!(peer.bytes(row + 4, 8), id);
    assert_eq!(peer.bytes(row + 12, 4), Ipv4Addr::LOCALHOST.octets());
    assert_eq!(peer.bytes(row + 20, 2), 31000u16.to_be_bytes());
    assert_eq!(
        peer.bytes(row + 0x30, 16),
        vec![0; 16],
        "exchange key never published"
    );
    assert_eq!(peer.u32(row + 0x40), 7);
    assert_eq!(peer.u32(row + 0x48), 1);
    assert_eq!(peer.u32(row + 0x50), 7);
    peer.put(BUFFER, &[0xcc; 64]);
    assert_eq!(
        peer.call("XOnlineMatchSearchParse", &[row, 7, MATCH_SPEC, BUFFER]),
        0
    );
    let values: Vec<u8> = [1, 2, 3, 5, 6, 7, 8]
        .into_iter()
        .flat_map(|id| (100u64 + id).to_le_bytes())
        .collect();
    assert_eq!(peer.bytes(BUFFER, 56), values);
    assert_eq!(peer.bytes(BUFFER + 56, 8), vec![0xcc; 8]);
    assert_eq!(
        peer.call("XOnlineMatchSessionFindFromID", &[lo, hi, 0, OUT]),
        0
    );
    let find = peer.u32(OUT);
    assert_eq!(peer.wait(find), TASK_SUCCESS);
    assert_eq!(
        peer.call("XOnlineMatchSearchGetResults", &[find, OUT, OUT + 4]),
        0
    );
    assert_eq!(peer.u32(OUT + 4), 1);
    attr(&host, MATCH_ATTR, 3, 333);
    assert_eq!(
        host.call(
            "XOnlineMatchSessionUpdate",
            &[lo, hi, 2, 6, 0, 0, 1, MATCH_ATTR, 0, OUT]
        ),
        0
    );
    let update = host.u32(OUT);
    assert_eq!(host.wait(update), TASK_SUCCESS);
    assert_eq!(
        host.call("XOnlineMatchSessionGetInfo", &[update, MATCH_ID, MATCH_KEY]),
        0
    );
    assert_eq!(host.bytes(MATCH_ID, 8), id);
    assert_eq!(host.bytes(MATCH_KEY, 16), key);
    assert_eq!(host.u32(MATCH_ATTR + 4), 0);
    let (_, table2, count) = match_search(&mut peer, 1, 0, cap);
    assert_eq!(count, 1);
    let row2 = peer.u32(table2);
    assert_eq!(peer.u32(row2 + 0x40), 6);
    assert_eq!(
        peer.call("XOnlineMatchSearchParse", &[row2, 7, MATCH_SPEC, BUFFER]),
        0
    );
    assert_eq!(peer.rt.memory().u64(BUFFER + 16).unwrap(), 333);
    // Completed searches retain their original task-owned snapshot.
    assert_eq!(peer.u32(row + 0x40), 7);
    attr(&peer, MATCH_ATTR, 3, 333);
    assert_eq!(match_search(&mut peer, 42, 1, cap).2, 1);
    attr(&peer, MATCH_ATTR, 3, 334);
    assert_eq!(match_search(&mut peer, 42, 1, cap).2, 0);
    assert_eq!(
        peer.call("XOnlineMatchSearch", &[1, 50, 0, 0, 0x5c, 0, OUT]),
        0
    );
    let small = peer.u32(OUT);
    assert_eq!(peer.wait(small), BUFFER_TOO_SMALL);
    peer.word(OUT, 0xdeadbeef);
    assert_eq!(
        peer.call("XOnlineMatchSearchGetResults", &[small, OUT, OUT + 4]),
        BUFFER_TOO_SMALL
    );
    assert_eq!(peer.u32(OUT), 0xdeadbeef);
    assert_eq!(host.call("XOnlineMatchSessionDelete", &[lo, hi, 0, OUT]), 0);
    let delete = host.u32(OUT);
    assert_eq!(host.wait(delete), TASK_SUCCESS);
    assert!(host.hle.local_sessions().is_empty());
    assert_eq!(match_search(&mut peer, 1, 0, cap).2, 0);
    assert_eq!(
        peer.call("XOnlineMatchSearchGetResults", &[search, OUT, OUT + 4]),
        0
    );
    assert_eq!(peer.u32(OUT + 4), 1);
    assert_eq!(peer.call("XOnlineTaskClose", &[search]), 0);
    assert_eq!(
        peer.call("XOnlineMatchSearchGetResults", &[search, OUT, OUT + 4]),
        INVALID_ARGUMENT
    );
    let kinds: Vec<_> = s
        .events()
        .into_iter()
        .filter(|e| e.stream == format!("session-{wire_id}"))
        .map(|e| e.kind)
        .collect();
    assert_eq!(
        kinds,
        vec!["SessionCreated", "SessionUpdated", "SessionDeleted"]
    );
}
/// A host's multiplayer skulls ride on its advertisement (docs/game/skulls.md
/// "In multiplayer"): a guest that found and joined the session reads them
/// at game start, asking the server again, so a change the host made after
/// the join is what the guest applies.
#[test]
fn guests_read_the_skulls_their_host_advertises() {
    use live_hle::HostSkulls;
    let s = Server::new();
    let a = s.account("Host");
    let b = s.account("Guest");
    let mut host = World::new(&s.embedded.url, &[&a]);
    let mut guest = World::raw_network(
        &s.embedded.url,
        json!([b.json()]),
        Duration::from_millis(100),
        32000,
    );
    host.logon();
    guest.logon();
    let wait = Duration::from_secs(5);
    assert_eq!(guest.hle.host_skulls(wait), HostSkulls::NotJoined);
    // What the boot installs: the signed-in profiles' choice, read when the
    // game advertises.
    host.hle
        .set_skulls_probe(Some(std::sync::Arc::new(|_: &runtime::Guest| {
            Some(vec![6])
        })));
    for (i, id) in [1, 2, 3, 5, 6, 7, 8].into_iter().enumerate() {
        attr(&host, MATCH_ATTR + i as u32 * 16, id, 100 + u64::from(id));
    }
    assert_eq!(
        host.call(
            "XOnlineMatchSessionCreate",
            &[1, 7, 0, 0, 7, MATCH_ATTR, 0, OUT]
        ),
        0
    );
    let create = host.u32(OUT);
    assert_eq!(host.wait(create), TASK_SUCCESS);
    assert_eq!(host.hle.local_sessions()[0].skulls, Some(vec![6]));
    let created = s
        .events()
        .into_iter()
        .find(|e| e.kind == "SessionCreated")
        .unwrap();
    assert_eq!(created.data["advertisement"]["skulls"], json!([6]));
    // The host's own key is not a joined session.
    assert_eq!(
        host.call("XOnlineMatchSessionGetInfo", &[create, MATCH_ID, MATCH_KEY]),
        0
    );
    assert_eq!(host.call("XNetRegisterKey", &[MATCH_ID, MATCH_KEY]), 0);
    assert_eq!(host.hle.host_skulls(wait), HostSkulls::NotJoined);
    // The guest finds it in the browser and joins (registers its key).
    let cap = guest.call("XOnlineMatchSearchResultsLen", &[50, 7, MATCH_SPEC]);
    let (_, table, count) = match_search(&mut guest, 1, 0, cap);
    assert_eq!(count, 1);
    let row = guest.u32(table);
    assert_eq!(guest.call("XNetRegisterKey", &[row + 4, row + 0x30]), 0);
    let session = live_proto::sign::hex(&host.hle.local_sessions()[0].id);
    assert_eq!(
        guest.hle.host_skulls(wait),
        HostSkulls::Known {
            session: session.clone(),
            skulls: vec![6],
            refreshed: true
        }
    );
    // The host turns Famine on too, then all off: open lobbies follow at
    // once, without waiting for the game's next update.
    host.hle.set_skulls_probe(None);
    host.hle.set_session_skulls(Some(vec![4, 6]));
    s.until(|| {
        s.get_as_operator::<Page<live_proto::session::Session>>("/v1/sessions")
            .items[0]
            .advertisement
            .skulls
            == Some(vec![4, 6])
    });
    assert_eq!(
        guest.hle.host_skulls(wait),
        HostSkulls::Known {
            session: session.clone(),
            skulls: vec![4, 6],
            refreshed: true
        }
    );
    host.hle.set_session_skulls(None);
    s.until(|| {
        s.get_as_operator::<Page<live_proto::session::Session>>("/v1/sessions")
            .items[0]
            .advertisement
            .skulls
            .is_none()
    });
    assert_eq!(
        guest.hle.host_skulls(wait),
        HostSkulls::Known {
            session,
            skulls: vec![],
            refreshed: true
        }
    );
}
/// Co-op over Live (docs/live/live-coop.md, "Co-op over Live built"): the
/// co-op screen arms a campaign on the host, so its lobby advertises as a
/// friends-only `coop` session naming the level and difficulty and presence
/// says "Co-op: <level>"; the screen's friends list and Invite send a game
/// invitation to that lobby; the invited guest finds the lobby and, once
/// joined, reads the campaign for its join screen. Leaving the co-op screen
/// turns the lobby back into a custom one.
#[test]
fn a_coop_lobby_advertises_its_campaign_and_the_invited_guest_reads_it() {
    use live_hle::Joined;
    use live_proto::session::{Campaign, Visibility};
    let s = Server::new();
    let a = s.account("Host");
    let b = s.account("Guest");
    let c = s.account("Stranger");
    friend_pair(&s, &a, &b);
    let port = game_port();
    let mut peer_port = game_port();
    while peer_port.abs_diff(port) <= 6 {
        peer_port = game_port();
    }
    let mut host = World::raw_network(
        &s.embedded.url,
        json!([a.json()]),
        Duration::from_secs(30),
        port,
    );
    let mut guest = World::raw_network(
        &s.embedded.url,
        json!([b.json()]),
        Duration::from_secs(30),
        peer_port,
    );
    let mut stranger = World::new(&s.embedded.url, &[&c]);
    host.logon();
    guest.logon();
    stranger.logon();
    let campaign = Campaign {
        level: "03a_oldmombasa".into(),
        name: Some("Outskirts".into()),
        difficulty: 2,
    };
    host.hle.set_coop(Some(campaign.clone()), Some(vec![0, 4]));
    assert_eq!(host.hle.coop(), Some(campaign.clone()));
    assert_eq!(host.hle.activity().as_deref(), Some("Co-op: Outskirts"));
    // The lobby the game advertises is a coop session, friends-only.
    for (i, id) in [1, 2, 3, 5, 6, 7, 8].into_iter().enumerate() {
        attr(&host, MATCH_ATTR + i as u32 * 16, id, 100 + u64::from(id));
    }
    assert_eq!(
        host.call(
            "XOnlineMatchSessionCreate",
            &[1, 7, 0, 0, 7, MATCH_ATTR, 0, OUT]
        ),
        0
    );
    let create = host.u32(OUT);
    assert_eq!(host.wait(create), TASK_SUCCESS);
    let local = &host.hle.local_sessions()[0];
    assert_eq!(local.campaign, Some(campaign.clone()));
    assert_eq!(local.visibility, Visibility::Friends);
    assert_eq!(local.skulls, Some(vec![0, 4]), "the campaign skulls, not multiplayer ones");
    let created = s
        .events()
        .into_iter()
        .find(|e| e.kind == "SessionCreated")
        .unwrap();
    assert_eq!(created.data["advertisement"]["flags"], json!("coop"));
    assert_eq!(
        created.data["advertisement"]["campaign"],
        json!({"level": "03a_oldmombasa", "name": "Outskirts", "difficulty": 2})
    );
    assert_eq!(created.data["advertisement"]["visibility"], json!("friends"));
    let local_id = local.id;
    let session = live_proto::sign::hex(&local_id);
    // A stranger's browser does not list it; the friend's does.
    let cap = stranger.call("XOnlineMatchSearchResultsLen", &[50, 7, MATCH_SPEC]);
    assert_eq!(match_search(&mut stranger, 1, 0, cap).2, 0);
    // The co-op screen's friends list, then Invite.
    let deadline = Instant::now() + Duration::from_secs(6);
    let friends = loop {
        let f = host.hle.friends(0);
        if f.iter().any(|f| f.xuid == b.xuid && f.online) {
            break f;
        }
        assert!(Instant::now() < deadline, "the friends list never filled");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(friends[0].gamertag, "Guest");
    assert_eq!(host.hle.friends(4), vec![], "no such controller");
    assert_eq!(host.hle.invite_to_lobby(0, b.xuid), Ok(session.clone()));
    assert_eq!(host.hle.invite_to_lobby(1, b.xuid).unwrap_err(), INVALID_USER);
    assert_eq!(guest.call("XOnlineFriendsStartup", &[0, OUT]), 0);
    let background = guest.u32(OUT);
    guest_until(&mut guest, |w| {
        w.call("XOnlineTaskContinue", &[background]) == TASK_RESULTS_AVAILABLE
    });
    let f = invited_friend(&mut guest, a.xuid);
    assert_eq!(f[40..48], local_id, "the invite names the co-op lobby");
    // The guest joins (registers the lobby's key) and reads the campaign.
    let wait = Duration::from_secs(5);
    assert_eq!(guest.hle.joined(wait), Joined::NotJoined);
    let cap = guest.call("XOnlineMatchSearchResultsLen", &[50, 7, MATCH_SPEC]);
    let (_, table, count) = match_search(&mut guest, 1, 0, cap);
    assert_eq!(count, 1);
    let row = guest.u32(table);
    assert_eq!(guest.call("XNetRegisterKey", &[row + 4, row + 0x30]), 0);
    let joined = guest.hle.joined(wait);
    assert_eq!(joined.campaign(), Some(&campaign));
    let Joined::Known { session: read, .. } = &joined else {
        panic!("{joined:?}")
    };
    assert_eq!(read.host_gamertag, "Host");
    assert_eq!(read.advertisement.skulls, Some(vec![0, 4]));
    // Ping: the host's QoS responder echoes the guest's pings at the
    // session's endpoint; the guest's next ping carries its last round
    // trip, which the host shows and advertises.
    let fresh = Duration::from_secs(6);
    assert_eq!(host.hle.guest_ping(fresh), None, "no guest has reported yet");
    let to = std::net::SocketAddrV4::new(read.endpoint.addr, read.endpoint.port);
    let timeout = Duration::from_secs(2);
    let first = guest.hle.ping_host(to, None, timeout).expect("the host echoes");
    assert!((1..=live_proto::session::PING_MAX_MS).contains(&first));
    assert_eq!(host.hle.guest_ping(fresh), None, "the first ping reports nothing");
    guest.hle.ping_host(to, Some(first), timeout).expect("the host echoes");
    let reported = host.hle.guest_ping(fresh);
    assert_eq!(reported, Some(first));
    host.hle.set_coop_ping(reported);
    assert_eq!(host.hle.local_sessions()[0].ping_ms, Some(first));
    s.until(|| {
        s.events().into_iter().any(|e| {
            e.kind == "SessionUpdated"
                && e.stream == format!("session-{session}")
                && e.data["ping_ms"] == json!(first)
        })
    });
    // Nothing answers on a port with no responder.
    let nobody = std::net::SocketAddrV4::new(read.endpoint.addr, game_port());
    assert_eq!(guest.hle.ping_host(nobody, None, Duration::from_millis(200)), None);
    // Presence says what the host is doing.
    s.until(|| {
        s.presence()
            .iter()
            .any(|p| p.xuid == a.xuid && p.activity.as_deref() == Some("Co-op: Outskirts"))
    });
    // Leaving co-op turns the lobby back into a custom game.
    host.hle.set_coop(None, None);
    s.until(|| {
        s.events().into_iter().any(|e| {
            e.kind == "SessionUpdated"
                && e.stream == format!("session-{session}")
                && e.data["flags"] == json!("custom")
        })
    });
    assert_eq!(host.hle.local_sessions()[0].campaign, None);
    assert_eq!(host.hle.local_sessions()[0].ping_ms, None, "a custom game has no co-op ping");
    assert_eq!(host.hle.activity(), None);
    assert!(guest.hle.joined(wait).campaign().is_none());
}
/// The custom games browser (docs/live/live-custom-games.md) is the retail
/// System Link browser in its Live mode: its search is `0x0008fa80`'s
/// procedure 1 with 50 results, and `0x0008fb30` keeps rows with exactly
/// seven columns. The host's visibility setting decides who gets the row.
#[test]
fn browser_search_rows_follow_the_hosts_visibility_setting() {
    use live_hle::Visibility;
    let s = Server::new();
    let a = s.account("Host");
    let b = s.account("Friend");
    let c = s.account("Stranger");
    friend_pair(&s, &a, &b);
    let mut host = World::new(&s.embedded.url, &[&a]);
    let mut friend = World::raw_network(
        &s.embedded.url,
        json!([b.json()]),
        Duration::from_millis(100),
        32000,
    );
    let mut stranger = World::raw_network(
        &s.embedded.url,
        json!([c.json()]),
        Duration::from_millis(100),
        33000,
    );
    host.logon();
    friend.logon();
    stranger.logon();
    assert_eq!(host.hle.session_visibility(), Visibility::Public);
    host.hle.set_session_visibility(Visibility::Friends);
    for (i, id) in [1, 2, 3, 5, 6, 7, 8].into_iter().enumerate() {
        attr(&host, MATCH_ATTR + i as u32 * 16, id, 100 + u64::from(id));
    }
    assert_eq!(
        host.call(
            "XOnlineMatchSessionCreate",
            &[1, 7, 0, 0, 7, MATCH_ATTR, 0, OUT]
        ),
        0
    );
    let create = host.u32(OUT);
    assert_eq!(host.wait(create), TASK_SUCCESS);
    assert_eq!(host.hle.local_sessions()[0].visibility, Visibility::Friends);
    // The advertisement says so on the server.
    let doc: Value = s.get_as_operator("/v1/sessions?state=open&visibility=friends");
    assert_eq!(
        doc["items"],
        json!([]),
        "anonymous reads never see friends-only"
    );
    let created = s
        .events()
        .into_iter()
        .find(|e| e.kind == "SessionCreated")
        .unwrap();
    assert_eq!(created.data["advertisement"]["visibility"], "friends");
    let cap = friend.call("XOnlineMatchSearchResultsLen", &[50, 7, MATCH_SPEC]);
    let (_, table, count) = match_search(&mut friend, 1, 0, cap);
    assert_eq!(count, 1, "the friend's browser lists it");
    let row = friend.u32(table);
    assert_eq!(
        friend.u32(row + 0x50),
        7,
        "seven columns, as 0x0008fb30 needs"
    );
    assert_eq!(friend.bytes(row + 20, 2), 31000u16.to_be_bytes());
    assert_eq!(
        match_search(&mut stranger, 1, 0, cap).2,
        0,
        "a stranger's does not"
    );
    // Changing the setting updates the open lobby at once, with a signed event.
    host.hle.set_session_visibility(Visibility::Public);
    s.until(|| {
        s.get_as_operator::<Page<live_proto::session::Session>>("/v1/sessions")
            .items
            .len()
            == 1
    });
    assert_eq!(match_search(&mut stranger, 1, 0, cap).2, 1);
    host.hle.set_session_visibility(Visibility::Invite);
    s.until(|| {
        s.get_as_operator::<Page<live_proto::session::Session>>("/v1/sessions")
            .items
            .is_empty()
    });
    assert_eq!(
        match_search(&mut friend, 1, 0, cap).2,
        0,
        "invite only: unlisted"
    );
    // The game's own updates carry the setting only when it changed.
    attr(&host, MATCH_ATTR, 3, 333);
    let id = host.hle.local_sessions()[0].id;
    let (lo, hi) = (
        u32::from_le_bytes(id[..4].try_into().unwrap()),
        u32::from_le_bytes(id[4..].try_into().unwrap()),
    );
    assert_eq!(
        host.call(
            "XOnlineMatchSessionUpdate",
            &[lo, hi, 2, 6, 0, 0, 1, MATCH_ATTR, 0, OUT]
        ),
        0
    );
    let update = host.u32(OUT);
    assert_eq!(host.wait(update), TASK_SUCCESS);
    let updates: Vec<_> = s
        .events()
        .into_iter()
        .filter(|e| e.kind == "SessionUpdated")
        .map(|e| e.data.get("visibility").cloned())
        .collect();
    assert_eq!(
        updates,
        vec![Some(json!("public")), Some(json!("invite")), None]
    );
    // An invitation still finds it by id.
    assert_eq!(
        friend.call("XOnlineMatchSessionFindFromID", &[lo, hi, 0, OUT]),
        0
    );
    let find = friend.u32(OUT);
    assert_eq!(friend.wait(find), TASK_SUCCESS);
    assert_eq!(
        friend.call("XOnlineMatchSearchGetResults", &[find, OUT, OUT + 4]),
        0
    );
    assert_eq!(friend.u32(OUT + 4), 1);
}
/// The title's own party privacy, read through the embedder's probe when a
/// lobby is advertised, wins over the setting.
#[test]
fn the_titles_privacy_probe_decides_a_new_advertisement() {
    use live_hle::Visibility;
    const PRIVACY: u32 = DATA + 0x14000;
    let s = Server::new();
    let a = s.account("Host");
    let mut host = World::new(&s.embedded.url, &[&a]);
    host.logon();
    host.hle.set_session_visibility(Visibility::Public);
    host.hle
        .set_visibility_probe(Some(std::sync::Arc::new(|g: &runtime::Guest| {
            match g.u32(PRIVACY).ok()? {
                0 => Some(Visibility::Public),
                1 => Some(Visibility::Friends),
                2 => Some(Visibility::Invite),
                _ => None,
            }
        })));
    host.word(PRIVACY, 2);
    for (i, id) in [1, 2, 3, 5, 6, 7, 8].into_iter().enumerate() {
        attr(&host, MATCH_ATTR + i as u32 * 16, id, 1);
    }
    assert_eq!(
        host.call(
            "XOnlineMatchSessionCreate",
            &[1, 7, 0, 0, 7, MATCH_ATTR, 0, OUT]
        ),
        0
    );
    let create = host.u32(OUT);
    assert_eq!(host.wait(create), TASK_SUCCESS);
    assert_eq!(host.hle.session_visibility(), Visibility::Invite);
    let created = s
        .events()
        .into_iter()
        .find(|e| e.kind == "SessionCreated")
        .unwrap();
    assert_eq!(created.data["advertisement"]["visibility"], "invite");
    assert!(
        s.get_as_operator::<Page<live_proto::session::Session>>("/v1/sessions")
            .items
            .is_empty()
    );
}
#[test]
fn sessions_typed_results_are_owned_bounded_and_signout_closes_advertisement() {
    let s = Server::new();
    let a = s.account("Host");
    let b = s.account("Peer");
    let mut host = World::new(&s.embedded.url, &[&a]);
    let mut peer = World::new(&s.embedded.url, &[&b]);
    let logon = host.logon();
    peer.logon();
    host.put(
        BUFFER,
        &"Héllo"
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    attr(&host, MATCH_ATTR, 0x100001, u64::from(BUFFER));
    host.put(BUFFER + 100, &[10, 20, 30]);
    attr(
        &host,
        MATCH_ATTR + 16,
        0x200002,
        (3u64 << 32) | u64::from(BUFFER + 100),
    );
    assert_eq!(
        host.call(
            "XOnlineMatchSessionCreate",
            &[1, 7, 0, 0, 2, MATCH_ATTR, 0, OUT]
        ),
        0
    );
    let task = host.u32(OUT);
    assert_eq!(host.wait(task), TASK_SUCCESS);
    host.put(BUFFER, &[0xff; 120]); // No guest pointers escape into the worker.
    let (_, table, n) = match_search(&mut peer, 42, 0, 0x2000);
    assert_eq!(n, 1);
    let row = peer.u32(table);
    peer.word(MATCH_SPEC, 0x100000);
    peer.word(MATCH_SPEC + 4, 20);
    peer.word(MATCH_SPEC + 8, 0x200000);
    peer.word(MATCH_SPEC + 12, 8);
    peer.put(BUFFER, &[0xcc; 40]);
    assert_eq!(
        peer.call("XOnlineMatchSearchParse", &[row, 2, MATCH_SPEC, BUFFER]),
        0
    );
    let expected: Vec<_> = "Héllo"
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(peer.bytes(BUFFER, expected.len()), expected);
    assert_eq!(
        peer.bytes(BUFFER + 20, 10),
        vec![3, 0, 10, 20, 30, 0, 0, 0, 0, 0]
    );
    assert_eq!(peer.bytes(BUFFER + 30, 10), vec![0xcc; 10]);
    peer.word(MATCH_SPEC + 4, 2);
    peer.put(BUFFER, &[0xcc; 40]);
    assert_eq!(
        peer.call("XOnlineMatchSearchParse", &[row, 2, MATCH_SPEC, BUFFER]),
        BUFFER_TOO_SMALL
    );
    assert_eq!(peer.bytes(BUFFER, 40), vec![0xcc; 40]);
    assert_eq!(host.call("XOnlineTaskClose", &[logon]), 0);
    s.until(|| {
        s.get_as_operator::<Page<live_proto::session::Session>>("/v1/sessions")
            .items
            .is_empty()
    });
    assert!(s.events().iter().any(|e| e.kind == "SessionDeleted"));
}

// Invitations are discovered over HTTP only; the transport proof uses no
// broadcast peers or synthetic networking bypass.
const INV_FRIEND: u32 = DATA + 0x11000;
const INV_RECORD: u32 = DATA + 0x12000;
const INV_ANSWER: u32 = DATA + 0x13000;
fn friend_pair(server: &Server, a: &Account, b: &Account) {
    for (signer, kind) in [(a, types::FRIEND_REQUESTED), (b, types::FRIEND_ACCEPTED)] {
        let author = format!("acct:{}@{}", signer.xuid, server.discovery.id);
        let event = live_proto::sign::EventBody::at_unix(
            &signer.key,
            &author,
            &live_proto::streams::account(signer.xuid),
            kind,
            json!({"from":a.xuid,"to":b.xuid}),
            time::OffsetDateTime::now_utc().unix_timestamp(),
        )
        .sign(&signer.key);
        let token = sign::issue_token(
            &signer.key,
            &author,
            &server.discovery.id,
            lxm::EVENTS_WRITE,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        );
        server
            .agent
            .post(&server.url("/v1/events"))
            .set("authorization", &format!("Bearer {token}"))
            .set("content-type", "application/jose")
            .send_string(&event)
            .unwrap();
    }
}
fn guest_until(w: &mut World, mut condition: impl FnMut(&mut World) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(6);
    while !condition(w) {
        assert!(Instant::now() < deadline, "guest invite refresh timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn invite_host(w: &mut World) -> (u32, u32) {
    assert_eq!(
        w.call("XOnlineMatchSessionCreate", &[1, 7, 0, 0, 0, 0, 0, OUT]),
        0
    );
    let task = w.u32(OUT);
    assert_eq!(w.wait(task), TASK_SUCCESS);
    assert_eq!(
        w.call("XOnlineMatchSessionGetInfo", &[task, MATCH_ID, MATCH_KEY]),
        0
    );
    (w.u32(MATCH_ID), w.u32(MATCH_ID + 4))
}
fn invited_friend(w: &mut World, from: Xuid) -> Vec<u8> {
    guest_until(w, |w| {
        let count = w.call("XOnlineFriendsGetLatest", &[0, 100, INV_FRIEND]);
        (0..count).any(|i| {
            w.rt.memory().u64(INV_FRIEND + i * 86).unwrap() == from.0
                && w.u32(INV_FRIEND + i * 86 + 28) & 0x08000000 != 0
        })
    });
    let count = w.call("XOnlineFriendsGetLatest", &[0, 100, INV_FRIEND]);
    let at = (0..count)
        .find(|i| w.rt.memory().u64(INV_FRIEND + i * 86).unwrap() == from.0)
        .unwrap();
    w.bytes(INV_FRIEND + at * 86, 86)
}
fn game_answer_info(w: &World, f: &[u8]) {
    w.put(INV_ANSWER, &f[..28]);
    w.put(INV_ANSWER + 28, &f[48..52]);
    w.put(INV_ANSWER + 32, &f[40..48]);
    w.put(INV_ANSWER + 40, &f[32..40]);
}
fn game_port() -> u16 {
    loop {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let p = socket.local_addr().unwrap().port();
        if p <= 65529 {
            return p;
        }
    }
}
fn address(ip: Ipv4Addr, port: u16) -> [u8; 16] {
    let mut b = [0; 16];
    b[..2].copy_from_slice(&2u16.to_le_bytes());
    b[2..4].copy_from_slice(&port.to_be_bytes());
    b[4..8].copy_from_slice(&ip.octets());
    b
}
fn udp_socket(w: &mut World) -> u32 {
    let socket = w.call("socket", &[2, 2, 17]);
    assert_ne!(socket, u32::MAX);
    w.put(BUFFER, &address(Ipv4Addr::LOCALHOST, 1000));
    assert_eq!(w.call("bind", &[socket, BUFFER, 16]), 0);
    w.word(OUT, 1000);
    assert_eq!(w.call("setsockopt", &[socket, 0xffff, 0x1006, OUT, 4]), 0);
    socket
}
#[test]
fn invites_message_accept_find_and_join_exchange_datagrams_without_broadcast() {
    let s = Server::new();
    let a = s.account("Host");
    let b = s.account("Joiner");
    friend_pair(&s, &a, &b);
    let port = game_port();
    let mut peer_port = game_port();
    while peer_port.abs_diff(port) <= 6 {
        peer_port = game_port();
    }
    let mut host = World::raw_network(
        &s.embedded.url,
        json!([a.json()]),
        Duration::from_secs(30),
        port,
    );
    let mut peer = World::raw_network(
        &s.embedded.url,
        json!([b.json()]),
        Duration::from_secs(30),
        peer_port,
    );
    host.logon();
    peer.logon();
    let (lo, hi) = invite_host(&mut host);
    host.put(MATCH_ATTR, &b.xuid.0.to_le_bytes());
    host.word(MATCH_ATTR + 8, 0);
    assert_eq!(
        host.call(
            "XOnlineGameInviteSend",
            &[0, 1, MATCH_ATTR, lo, hi, 0, 0, 0, OUT]
        ),
        0
    );
    let send = host.u32(OUT);
    assert_eq!(host.wait(send), TASK_SUCCESS);
    assert_eq!(peer.call("XOnlineFriendsStartup", &[0, OUT]), 0);
    let background = peer.u32(OUT);
    guest_until(&mut peer, |w| {
        w.call("XOnlineTaskContinue", &[background]) == TASK_RESULTS_AVAILABLE
    });
    let f = invited_friend(&mut peer, a.xuid);
    assert_eq!(
        u64::from_le_bytes(f[40..48].try_into().unwrap()),
        u64::from(lo) | (u64::from(hi) << 32)
    );
    assert_eq!(
        peer.call("XOnlineGetNotificationEx", &[0, OUT, OUT + 16]),
        1
    );
    assert_eq!(peer.bytes(OUT, 1), vec![3]);
    let message = peer.u32(OUT + 1);
    assert_eq!(
        peer.call("XOnlineGetNotificationEx", &[0, OUT, OUT + 16]),
        0,
        "one toast per invite"
    );
    assert_eq!(
        peer.call("XOnlineMessageEnumerate", &[0, INV_RECORD, OUT]),
        0
    );
    assert_eq!(peer.u32(OUT), 1);
    assert_eq!(peer.u32(INV_RECORD + 32), message);
    assert_eq!(peer.bytes(INV_RECORD, 8), a.xuid.0.to_le_bytes());
    assert_eq!(peer.bytes(INV_RECORD + 12, 1), vec![3]);
    assert_eq!(
        peer.call("XOnlineMessageDetails", &[0, message, 0, 0, 0, OUT]),
        0
    );
    let details = peer.u32(OUT);
    assert_eq!(peer.wait(details), TASK_SUCCESS);
    assert_eq!(
        peer.call(
            "XOnlineMessageDetailsGetResultsProperty",
            &[details, 0x581, 8, BUFFER, OUT, OUT + 4]
        ),
        0
    );
    assert_eq!(peer.bytes(BUFFER, 8), f[40..48]);
    assert_eq!(peer.u32(OUT), 8);
    assert_eq!(peer.call("XOnlinePresenceInit", &[0, 0, OUT]), 0);
    let presence = peer.u32(OUT);
    assert_eq!(peer.wait(presence), TASK_SUCCESS);
    peer.put(INV_ANSWER, &a.xuid.0.to_le_bytes());
    peer.word(INV_ANSWER + 8, 0);
    assert_eq!(
        peer.call("XOnlinePresenceAdd", &[presence, 9, 1, INV_ANSWER]),
        0
    );
    assert_eq!(
        peer.call("XOnlinePresenceGetLatest", &[presence, 9, 1, BUFFER]),
        0
    );
    assert_eq!(peer.bytes(BUFFER, 8), a.xuid.0.to_le_bytes());
    assert_ne!(peer.u32(BUFFER + 12) & 0x08000000, 0);
    assert_eq!(peer.bytes(BUFFER + 16, 8), f[40..48]);
    game_answer_info(&peer, &f);
    assert_eq!(
        peer.call("XOnlineGameInviteAnswer", &[0, INV_ANSWER, 1, 0, OUT]),
        0
    );
    let accept = peer.u32(OUT);
    assert_eq!(peer.wait(accept), TASK_SUCCESS);
    peer.put(INV_RECORD, &[0xcc; 160]);
    assert_eq!(
        peer.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]),
        0
    );
    assert_eq!(peer.bytes(INV_RECORD, 86), f);
    assert_eq!(peer.bytes(INV_RECORD + 86, 2), vec![0; 2]);
    assert_eq!(peer.bytes(INV_RECORD + 88, 8), b.xuid.0.to_le_bytes());
    assert!(peer.rt.memory().u64(INV_RECORD + 100).unwrap() > 0);
    assert_eq!(peer.bytes(INV_RECORD + 108, 8), b.xuid.0.to_le_bytes());
    assert_eq!(peer.bytes(INV_RECORD + 156, 4), vec![0xcc; 4]);
    assert_eq!(
        peer.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]),
        NO_ACCEPTED_INVITE,
        "accepted records are consumed"
    );
    assert_eq!(peer.bytes(INV_RECORD, 156), vec![0; 156]);
    assert_eq!(
        peer.call("XOnlineMessageDelete", &[0, message, 0]),
        0,
        "delete after acceptance acknowledges an already handled ID"
    );
    assert_eq!(
        peer.call("XOnlineMatchSessionFindFromID", &[lo, hi, 0, OUT]),
        0
    );
    let find = peer.u32(OUT);
    assert_eq!(peer.wait(find), TASK_SUCCESS);
    assert_eq!(
        peer.call("XOnlineMatchSearchGetResults", &[find, OUT, OUT + 4]),
        0
    );
    assert_eq!(peer.u32(OUT + 4), 1);
    let row = peer.u32(peer.u32(OUT));
    assert_eq!(peer.call("XNetRegisterKey", &[row + 4, row + 0x30]), 0);
    assert_eq!(
        peer.call("XNetXnAddrToInAddr", &[row + 12, row + 4, OUT]),
        0
    );
    let ip = peer.u32(OUT);
    assert_eq!(peer.call("XNetConnect", &[ip]), 0);
    assert_eq!(peer.call("XNetGetConnectStatus", &[ip]), 2);
    // The session's actual host key and zero remote HLE key still identify
    // the same QoS listener; no exchange key is published by the server.
    assert_eq!(host.call("XNetRegisterKey", &[MATCH_ID, MATCH_KEY]), 0);
    host.put(BUFFER, b"synthetic-lobby");
    assert_eq!(
        host.call(
            "XNetQosListen",
            &[MATCH_ID, BUFFER, 15, 2_000_000, 1 | 4 | 8]
        ),
        0
    );
    peer.word(MATCH_ATTR, row + 12);
    peer.word(MATCH_ATTR + 4, row + 4);
    peer.word(MATCH_ATTR + 8, row + 0x30);
    assert_eq!(
        peer.call(
            "XNetQosLookup",
            &[
                1,
                MATCH_ATTR,
                MATCH_ATTR + 4,
                MATCH_ATTR + 8,
                0,
                0,
                0,
                3,
                0,
                0,
                0,
                OUT
            ]
        ),
        0
    );
    let qos = peer.u32(OUT);
    assert_eq!(peer.u32(qos + 4), 0);
    assert_eq!(peer.bytes(qos + 8, 1), vec![0xb]);
    assert_eq!(peer.bytes(peer.u32(qos + 16), 15), b"synthetic-lobby");
    assert_eq!(peer.call("XNetQosRelease", &[qos]), 0);
    let hs = udp_socket(&mut host);
    let ps = udp_socket(&mut peer);
    peer.put(BUFFER, b"join");
    peer.put(
        BUFFER + 100,
        &address(Ipv4Addr::from(ip.to_le_bytes()), 1000),
    );
    assert_eq!(
        peer.call("sendto", &[ps, BUFFER, 4, 0, BUFFER + 100, 16]),
        4
    );
    host.word(OUT, 16);
    assert_eq!(
        host.call("recvfrom", &[hs, BUFFER, 64, 0, BUFFER + 100, OUT]),
        4
    );
    assert_eq!(host.bytes(BUFFER, 4), b"join");
    host.put(BUFFER, b"welcome");
    assert_eq!(
        host.call("sendto", &[hs, BUFFER, 7, 0, BUFFER + 100, 16]),
        7
    );
    assert_eq!(peer.call("recv", &[ps, BUFFER, 64, 0]), 7);
    assert_eq!(peer.bytes(BUFFER, 7), b"welcome");
    assert_eq!(host.call("closesocket", &[hs]), 0);
    assert_eq!(peer.call("closesocket", &[ps]), 0);
    for task in [details, accept, find, background, presence] {
        assert_eq!(peer.call("XOnlineTaskClose", &[task]), 0);
    }
    assert_eq!(host.call("XOnlineTaskClose", &[send]), 0);
    assert_eq!(
        peer.call("XOnlineMessageEnumerate", &[0, INV_RECORD, OUT]),
        0
    );
    assert_eq!(peer.u32(OUT), 0);
    let events = s.events();
    assert!(events.iter().any(|e| e.kind == live_proto::mailbox::SENT));
    assert!(
        events
            .iter()
            .any(|e| e.kind == live_proto::mailbox::ANSWERED && e.data["answer"] == "accept")
    );
}
#[test]
fn invites_friend_send_decline_remove_accept_and_join_are_asynchronous() {
    let s = Server::new();
    let a = s.account("Host");
    let b = s.account("Joiner");
    friend_pair(&s, &a, &b);
    let mut host = World::new(&s.embedded.url, &[&a]);
    let mut peer = World::new(&s.embedded.url, &[&b]);
    host.logon();
    peer.logon();
    let (lo, hi) = invite_host(&mut host);
    // Two friends prove the retail 86-byte array stride, including the second recipient.
    let c = s.account("Third");
    friend_pair(&s, &a, &c);
    let mut third = World::new(&s.embedded.url, &[&c]);
    third.logon();
    let mut f = [0; 86];
    f[..8].copy_from_slice(&b.xuid.0.to_le_bytes());
    host.put(INV_FRIEND, &f);
    f[..8].copy_from_slice(&c.xuid.0.to_le_bytes());
    host.put(INV_FRIEND + 86, &f);
    assert_eq!(
        host.call("XOnlineFriendsGameInvite", &[0, lo, hi, 2, INV_FRIEND]),
        0
    );
    let _ = invited_friend(&mut third, a.xuid);
    assert_eq!(
        third.call("XOnlineMessageEnumerate", &[0, INV_RECORD, OUT]),
        0
    );
    assert_eq!(third.u32(OUT), 1);
    let third_message = third.u32(INV_RECORD + 32);
    assert_eq!(
        third.call("XOnlineMessageDelete", &[0, third_message, 0]),
        0
    );
    guest_until(&mut third, |w| {
        assert_eq!(
            w.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]),
            NO_ACCEPTED_INVITE
        );
        s.events().iter().any(|e| {
            e.kind == live_proto::mailbox::ANSWERED
                && e.data["by"] == c.xuid.to_string()
                && e.data["answer"] == "remove"
        })
    });

    let f = invited_friend(&mut peer, a.xuid);
    peer.put(INV_FRIEND, &f);
    assert_eq!(
        peer.call("XOnlineFriendsAnswerGameInvite", &[0, INV_FRIEND, 0]),
        0
    );
    guest_until(&mut peer, |w| {
        w.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]);
        s.events().iter().any(|e| {
            e.kind == live_proto::mailbox::ANSWERED
                && e.data["by"] == b.xuid.to_string()
                && e.data["answer"] == "decline"
        })
    });
    assert_eq!(
        peer.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]),
        NO_ACCEPTED_INVITE
    );
    for answer in [2, 1] {
        assert_eq!(
            host.call("XOnlineFriendsGameInvite", &[0, lo, hi, 1, INV_FRIEND]),
            0
        );
        let f = invited_friend(&mut peer, a.xuid);
        peer.put(INV_FRIEND, &f);
        assert_eq!(
            peer.call("XOnlineFriendsAnswerGameInvite", &[0, INV_FRIEND, answer]),
            0
        );
        if answer == 1 {
            guest_until(&mut peer, |w| {
                w.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]) == 0
            });
        } else {
            guest_until(&mut peer, |w| {
                w.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]);
                s.events().iter().any(|e| {
                    e.kind == live_proto::mailbox::ANSWERED
                        && e.data["by"] == b.xuid.to_string()
                        && e.data["answer"] == "remove"
                })
            });
        }
    }
    // Joining an already playing friend needs no invitation: it resolves the
    // signed-in user's actual friendship and the friend's open session first.
    assert_eq!(peer.call("XOnlineFriendsJoinGame", &[0, INV_FRIEND]), 0);
    guest_until(&mut peer, |w| {
        w.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]) == 0
    });
    assert_eq!(peer.bytes(INV_RECORD + 40, 8), host.bytes(MATCH_ID, 8));
}

#[test]
fn invites_invalid_arguments_faults_and_unreachable_answers_never_launch_a_join() {
    let s = Server::new();
    let a = s.account("Host");
    let b = s.account("Joiner");
    let mut host = World::timed(&s.embedded.url, &[&a], Duration::from_secs(3600));
    let mut peer = World::timed(&s.embedded.url, &[&b], Duration::from_secs(3600));
    host.logon();
    peer.logon();
    let (lo, hi) = invite_host(&mut host);
    host.put(INV_FRIEND, &b.xuid.0.to_le_bytes());
    assert_eq!(
        host.call("XOnlineFriendsGameInvite", &[0, lo, hi, 101, INV_FRIEND]),
        INVALID_ARGUMENT
    );
    let event = host.raw_call("XOnlineFriendsGameInvite", &[0, lo, hi, 1, 0xfffffff0]);
    assert!(matches!(
        host.ctx.event(event.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(
        host.call("XOnlineFriendsGameInvite", &[0, lo, hi, 1, INV_FRIEND]),
        0
    );
    let f = invited_friend(&mut peer, a.xuid);
    peer.put(INV_FRIEND, &f);
    game_answer_info(&peer, &f);
    peer.word(OUT, 0xdeadbeef);
    for args in [
        [0, INV_ANSWER, 3, 0, OUT],
        [0, INV_ANSWER, 1, 1, OUT],
        [4, INV_ANSWER, 1, 0, OUT],
    ] {
        let code = peer.call("XOnlineGameInviteAnswer", &args);
        assert!(matches!(code, INVALID_ARGUMENT | INVALID_USER));
        assert_eq!(peer.u32(OUT), 0xdeadbeef);
    }
    let event = peer.raw_call(
        "XOnlineGameInviteAnswer",
        &[0, INV_ANSWER, 1, 0, 0xfffffff0],
    );
    assert!(matches!(
        peer.ctx.event(event.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(
        peer.call("XOnlineMessageEnumerate", &[0, INV_RECORD, OUT]),
        0
    );
    assert_eq!(
        peer.u32(OUT),
        1,
        "bad caller data did not consume the invite"
    );
    assert_eq!(
        peer.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]),
        NO_ACCEPTED_INVITE
    );
    drop(s);
    assert_eq!(
        peer.call("XOnlineGameInviteAnswer", &[0, INV_ANSWER, 1, 0, OUT]),
        0
    );
    let task = peer.u32(OUT);
    assert_eq!(peer.wait(task), CANNOT_ACCESS_SERVICE);
    assert_eq!(
        peer.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]),
        NO_ACCEPTED_INVITE
    );
    assert_eq!(peer.call("XOnlineTaskClose", &[task]), 0);
    assert_eq!(peer.call("XOnlineTaskContinue", &[task]), INVALID_ARGUMENT);
    assert_eq!(
        peer.call("XOnlineMessageEnumerate", &[0, INV_RECORD, OUT]),
        0
    );
    assert_eq!(
        peer.u32(OUT),
        1,
        "failed answer restores the cached invitation"
    );
}
#[test]
fn invites_nonfriend_join_is_refused_and_peer_never_answer_blocks_future_invitations() {
    let s = Server::new();
    let a = s.account("Host");
    let b = s.account("Joiner");
    let mut host = World::new(&s.embedded.url, &[&a]);
    let mut peer = World::new(&s.embedded.url, &[&b]);
    host.logon();
    peer.logon();
    let (lo, hi) = invite_host(&mut host);
    let mut friend = [0; 86];
    friend[..8].copy_from_slice(&a.xuid.0.to_le_bytes());
    friend[40..48].copy_from_slice(&host.bytes(MATCH_ID, 8));
    peer.put(INV_FRIEND, &friend);
    assert_eq!(peer.call("XOnlineFriendsJoinGame", &[0, INV_FRIEND]), 0);
    guest_until(&mut peer, |w| {
        assert_eq!(
            w.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]),
            NO_ACCEPTED_INVITE
        );
        w.hle.server_status().last_error != 0
    });
    host.put(INV_FRIEND, &b.xuid.0.to_le_bytes());
    assert_eq!(
        host.call("XOnlineFriendsGameInvite", &[0, lo, hi, 1, INV_FRIEND]),
        0
    );
    let f = invited_friend(&mut peer, a.xuid);
    game_answer_info(&peer, &f);
    assert_eq!(
        peer.call("XOnlineGameInviteAnswer", &[0, INV_ANSWER, 2, 0, OUT]),
        0
    );
    let task = peer.u32(OUT);
    assert_eq!(peer.wait(task), TASK_SUCCESS);
    assert_eq!(
        peer.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]),
        NO_ACCEPTED_INVITE
    );
    host.put(MATCH_ATTR, &b.xuid.0.to_le_bytes());
    assert_eq!(
        host.call(
            "XOnlineGameInviteSend",
            &[0, 1, MATCH_ATTR, lo, hi, 0, 0, 0, OUT]
        ),
        0
    );
    let task = host.u32(OUT);
    assert_eq!(host.wait(task), INVALID_ARGUMENT);
    assert!(
        s.events()
            .iter()
            .any(|e| e.kind == live_proto::mailbox::ANSWERED && e.data["answer"] == "block")
    );
}

/// A lobby that re-advertises gets a new session id; the invitation and
/// the guest's acceptance follow the newest, whatever the ids' order (the
/// smallest id was once taken for the lobby's).
#[test]
fn coop_auto_invites_to_the_lobbys_current_session_not_the_smallest_id() {
    let s = Server::new();
    let a = s.account("Host");
    let b = s.account("Guest");
    friend_pair(&s, &a, &b);
    let port = game_port();
    let mut peer_port = game_port();
    while peer_port.abs_diff(port) <= 6 {
        peer_port = game_port();
    }
    let mut host = World::raw_network(
        &s.embedded.url,
        json!([a.json()]),
        Duration::from_secs(30),
        port,
    );
    let mut guest = World::raw_network(
        &s.embedded.url,
        json!([b.json()]),
        Duration::from_secs(30),
        peer_port,
    );
    host.logon();
    guest.logon();
    // Advertise until the newest session isn't the smallest id.
    let mut ids = Vec::new();
    loop {
        invite_host(&mut host);
        let id = u64::from_be_bytes(host.bytes(MATCH_ID, 8).try_into().unwrap());
        ids.push(id);
        if ids.len() > 1 && ids.iter().any(|&old| old < id) {
            break;
        }
        assert!(ids.len() < 16, "ids {ids:x?}");
    }
    let current = host.bytes(MATCH_ID, 8);
    host.hle.set_coop_auto(Some("guest"), None);
    guest.hle.set_coop_auto(None, Some("HOST"));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        host.call("XOnlineFriendsGetLatest", &[0, 100, INV_FRIEND]);
        guest.call("XOnlineFriendsGetLatest", &[0, 100, INV_FRIEND]);
        if guest.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]) == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "no accepted invitation");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(guest.bytes(INV_RECORD + 40, 8), current);
    let sent: Vec<_> = s
        .events()
        .into_iter()
        .filter(|e| e.kind == live_proto::mailbox::SENT)
        .collect();
    assert_eq!(sent.len(), 1, "one invitation, to the current session");
}
#[test]
fn coop_auto_invites_the_named_friend_and_the_guest_accepts_by_itself() {
    let s = Server::new();
    let a = s.account("Host");
    let b = s.account("Guest");
    friend_pair(&s, &a, &b);
    let port = game_port();
    let mut peer_port = game_port();
    while peer_port.abs_diff(port) <= 6 {
        peer_port = game_port();
    }
    let mut host = World::raw_network(
        &s.embedded.url,
        json!([a.json()]),
        Duration::from_secs(30),
        port,
    );
    let mut guest = World::raw_network(
        &s.embedded.url,
        json!([b.json()]),
        Duration::from_secs(30),
        peer_port,
    );
    host.hle.set_coop_auto(Some("guest"), None);
    guest.hle.set_coop_auto(None, Some("HOST"));
    host.logon();
    guest.logon();
    invite_host(&mut host);
    // Nobody presses anything: the host's and the guest's ordinary friends
    // polling is all that runs.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        host.call("XOnlineFriendsGetLatest", &[0, 100, INV_FRIEND]);
        guest.call("XOnlineFriendsGetLatest", &[0, 100, INV_FRIEND]);
        if guest.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]) == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "no accepted invitation");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(guest.rt.memory().u64(INV_RECORD).unwrap(), a.xuid.0);
    assert_eq!(guest.bytes(INV_RECORD + 40, 8), host.bytes(MATCH_ID, 8));
    // Once: no second invitation, no second acceptance.
    for _ in 0..50 {
        host.call("XOnlineFriendsGetLatest", &[0, 100, INV_FRIEND]);
        guest.call("XOnlineFriendsGetLatest", &[0, 100, INV_FRIEND]);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_ne!(
        guest.call("XOnlineFriendsGetAcceptedGameInvite", &[INV_RECORD]),
        0
    );
    let sent = s
        .events()
        .into_iter()
        .filter(|e| e.kind == live_proto::mailbox::SENT)
        .count();
    assert_eq!(sent, 1);
}
