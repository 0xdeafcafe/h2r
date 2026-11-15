//! The message of the day end to end, as the owner uses it: the embedded
//! server on its file store, the web admin's `PUT /v1/titles/{title}/motd`,
//! and the guest calls the game's message manager makes to fetch it
//! (docs/live-motd-fetch.md §2). Guest frames are synthetic; no XBE, image
//! build or game execution.
use live_hle::{
    ATTACHMENT_CAP, Hle, LiveConfig, NetworkConfig, SUMMARY_SIZE, ServerOptions, entries::ENTRIES,
    status::*,
};
use live_proto::{
    CreatedAccount, Discovery, Xuid, lxm,
    sign::{self, AccountRequest, KeyPair},
};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Runner, Runtime, exec::HARNESS_EXIT, guest::Permissions,
};
use serde_json::{Value, json};
use std::{
    io::Read,
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const DATA: u32 = 0x700000;
const OUT: u32 = DATA + 0x1000;
const USERS: u32 = DATA + 0x2000;
const BUFFER: u32 = DATA + 0x6000;
const FRAME: u32 = 0x600f00;
const MOTD_ID: u32 = 1;

struct Server {
    embedded: live_server::Embedded,
    agent: ureq::Agent,
    discovery: Discovery,
}

impl Server {
    /// `h2 play --live` without Postgres: the file store in `dir`.
    fn start(dir: &Path) -> Self {
        let embedded = live_server::start_embedded(
            live_server::FILE_DB,
            "127.0.0.1:0".parse().unwrap(),
            dir,
            live_server::http::Site::default(),
        )
        .unwrap();
        assert_eq!(embedded.store, "file");
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
            agent,
            discovery,
        }
    }
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.embedded.url)
    }
    fn account(&self, tag: &str) -> (Xuid, Arc<KeyPair>) {
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
        (created.xuid, key)
    }
    /// The public read the dashboard and stats site use: no token.
    fn motd(&self) -> Value {
        serde_json::from_reader(
            self.agent
                .get(&self.url("/v1/titles/4D530064/motd"))
                .call()
                .unwrap()
                .into_reader(),
        )
        .unwrap()
    }
    /// What the web admin's MOTD editor sends: a token the account signs.
    fn put_motd(&self, xuid: Xuid, key: &KeyPair, doc: &Value) -> Result<Value, u16> {
        let token = sign::issue_token(
            key,
            &format!("acct:{xuid}@{}", self.discovery.id),
            &self.discovery.id,
            lxm::TITLES_WRITE,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        );
        match self
            .agent
            .put(&self.url("/v1/titles/4D530064/motd"))
            .set("authorization", &format!("Bearer {token}"))
            .set("content-type", "application/json")
            .send_string(&doc.to_string())
        {
            Ok(r) => Ok(serde_json::from_reader(r.into_reader()).unwrap()),
            Err(ureq::Error::Status(code, _)) => Err(code),
            Err(e) => panic!("{e}"),
        }
    }
}

struct World {
    rt: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
    _hle: Arc<Hle>,
}

impl World {
    fn new(url: &str, xuid: Xuid, key: &KeyPair) -> Self {
        let accounts =
            json!([{"xuid": xuid, "gamertag": "Owner", "server": url, "key": key.private_jwk()}]);
        let path =
            std::env::temp_dir().join(format!("h2-motd-accounts-{}.json", sign::random_id()));
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
                heartbeat_interval: Duration::from_secs(30),
            },
        )
        .unwrap();
        std::fs::remove_file(path).unwrap();
        let mut guest = runtime::Guest::new();
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
            _hle: hle,
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
            ..CpuAbi::default()
        };
        let t = Runner::default().run(&mut self.cpu, &mut self.ctx, e.va);
        assert!(
            matches!(self.ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{name}: {:?}",
            self.ctx.event(t.event_id)
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
        assert_eq!(self.call("XNetStartup", &[0]), 0);
        assert_eq!(self.call("WSAStartup", &[2, DATA]), 0);
        assert_eq!(self.call("XOnlineStartup", &[0]), 0);
        assert_eq!(self.call("XOnlineGetUsers", &[DATA, OUT]), 0);
        self.put(USERS, &self.bytes(DATA, 112));
        self.word(DATA + 0x4000, 2);
        assert_eq!(
            self.call("XOnlineLogon", &[USERS, DATA + 0x4000, 1, 0, OUT]),
            0
        );
        let h = self.u32(OUT);
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
    /// The summary `messages` just enumerated, and the notification poll,
    /// as the game's player-facing UI sees them: XONLINE_MSG_SUMMARY
    /// (`xonline.h`, pack 8: XUID +0..12, type +12, context +16, sent time
    /// +24, id +32, flags +36, sender title +40, expiry +44, details size
    /// +46, sender name +48). Every per-player screen matches on a nonzero
    /// sender XUID, so XUID 0 is never listed or opened; GetNotificationEx
    /// reports nothing, so no "new message" toast or indicator
    /// (`0x002359ce`, `0x0023591a`).
    fn assert_hidden_title_message(&mut self) {
        let summary = self.bytes(DATA, SUMMARY_SIZE);
        assert_eq!(&summary[0..12], &[0; 12], "sender XUID 0");
        assert_eq!(summary[12], 1, "TITLE_CUSTOM");
        assert_eq!(&summary[32..36], &MOTD_ID.to_le_bytes());
        assert_eq!(&summary[40..44], &0x4d530064u32.to_le_bytes());
        self.put(DATA, &[0xcc; 9]);
        self.word(OUT, 0xdeadbeef);
        assert_eq!(self.call("XOnlineGetNotificationEx", &[0, DATA, OUT]), 0);
        assert_eq!(self.bytes(DATA, 9), vec![0; 9]);
        assert_eq!(self.u32(OUT), 0);
    }
    /// The game's message manager for one file (docs/live-motd-fetch.md
    /// §2-§3): details, the file-class property, the attachment probe,
    /// then the download into its 0x499a-byte buffer.
    fn download(&mut self, id: u32) -> Vec<u8> {
        assert_eq!(
            self.call("XOnlineMessageDetails", &[0, id, 0x10, 0, 0, OUT]),
            0
        );
        let details = self.u32(OUT);
        assert_eq!(self.wait(details), TASK_SUCCESS);
        assert_eq!(
            self.call(
                "XOnlineMessageDetailsGetResultsProperty",
                &[details, 0x4c5, 4, DATA, OUT, OUT + 4]
            ),
            0
        );
        assert_eq!(self.u32(DATA), id, "the file class routes it to the MOTD");
        assert_eq!(
            self.call(
                "XOnlineMessageDetailsGetResultsProperty",
                &[details, 0x9c1, 2048, DATA, OUT, OUT + 4]
            ),
            MESSAGE_DOWNLOAD_REQUIRED
        );
        let size = self.u32(OUT);
        assert!(
            size <= ATTACHMENT_CAP,
            "{size} bytes overflow the game's buffer"
        );
        assert_eq!(
            self.call(
                "XOnlineMessageDownloadAttachmentToMemory",
                &[details, 0x9c1, BUFFER, ATTACHMENT_CAP, 0, OUT]
            ),
            0
        );
        let download = self.u32(OUT);
        assert_eq!(self.wait(download), TASK_SUCCESS);
        assert_eq!(
            self.call(
                "XOnlineMessageDownloadAttachmentToMemoryGetResults",
                &[download, OUT, OUT + 4, OUT + 8]
            ),
            0
        );
        let received = self.u32(OUT + 4);
        assert_eq!(received, self.u32(OUT + 8));
        assert_eq!(self.call("XOnlineTaskClose", &[download]), 0);
        assert_eq!(self.call("XOnlineTaskClose", &[details]), 0);
        self.bytes(BUFFER, received as usize)
    }
}

/// What the game's completion handler `0x0008c790` checks and writes to
/// `n:\message_of_the_day.dat` (docs/live-motd.md §3): BE size 0x25224 at
/// +0x10, a zlib stream from +0x14 inflating to exactly that, then the
/// console language's header and its message block.
fn as_the_game_reads(file: &[u8]) -> (String, Vec<String>) {
    assert!(file.len() > 0x10);
    assert_eq!(
        u32::from_be_bytes(file[0x10..0x14].try_into().unwrap()),
        0x25224
    );
    let mut raw = Vec::new();
    flate2::read::ZlibDecoder::new(&file[0x14..])
        .read_to_end(&mut raw)
        .unwrap();
    assert_eq!(raw.len(), 0x25224);
    let wide = |b: &[u8]| {
        let units: Vec<u16> = b
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|u| *u != 0)
            .collect();
        String::from_utf16(&units).unwrap()
    };
    let slot = 0; // English
    let header = wide(&raw[slot * 0x200..][..0x200]);
    let block = &raw[0x1200 + slot * 0x4004..][..0x4004];
    let count = u32::from_le_bytes(block[..4].try_into().unwrap()) as usize;
    assert!(count <= 32);
    let messages = (0..count)
        .map(|i| wide(&block[4 + i * 0x200..][..0x200]))
        .collect();
    (header, messages)
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "h2-hle-motd-{}-{}",
        std::process::id(),
        sign::random_id()
    ))
}

#[test]
fn a_motd_published_from_the_web_editor_reaches_the_game_and_survives_a_restart() {
    let dir = temp_dir();
    let published = json!({
        "header": "Owner's header",
        "messages": ["Did you know? This came from the web editor.", "Second tip"],
    });
    let (xuid, key) = {
        let server = Server::start(&dir);
        // Nothing published yet: the server's default is there, publicly
        // readable, and the game finds it at sign-in.
        let default = live_server::default_motd::motd();
        assert_eq!(server.motd()["header"], default.header);
        let (xuid, key) = server.account("Owner");
        let mut w = World::new(&server.embedded.url, xuid, &key);
        w.logon();
        assert_eq!(w.messages(), vec![MOTD_ID]);
        w.assert_hidden_title_message();
        assert_eq!(
            as_the_game_reads(&w.download(MOTD_ID)),
            (default.header.clone(), default.messages.clone())
        );

        // Only an admin may publish; `h2 play --live` makes its own
        // account one.
        assert_eq!(server.put_motd(xuid, &key, &published), Err(403));
        server.embedded.live.grant_admin_runtime(xuid, "Owner");
        server.put_motd(xuid, &key, &published).unwrap();
        assert_eq!(server.motd(), published);

        // In the same session the catalog still lists the MOTD (it was
        // there at sign-in) and a fresh download is the new file: the game
        // gets it the next time its manager fetches, i.e. its next sign-in.
        assert_eq!(w.messages(), vec![MOTD_ID]);
        assert_eq!(
            as_the_game_reads(&w.download(MOTD_ID)),
            (
                "Owner's header".to_string(),
                vec![
                    "Did you know? This came from the web editor.".to_string(),
                    "Second tip".to_string()
                ]
            )
        );
        (xuid, key)
        // The server stops here, as when the game quits.
    };

    // Restarted on the same directory: the account and the published MOTD
    // are still there, and the default does not replace it.
    let server = Server::start(&dir);
    assert_eq!(server.motd(), published);
    let mut w = World::new(&server.embedded.url, xuid, &key);
    w.logon();
    assert_eq!(w.messages(), vec![MOTD_ID]);
    let (header, messages) = as_the_game_reads(&w.download(MOTD_ID));
    assert_eq!(header, "Owner's header");
    assert_eq!(messages.len(), 2);
    drop(w);
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_motd_the_game_cannot_hold_is_refused_and_the_old_one_stays() {
    let dir = temp_dir();
    let server = Server::start(&dir);
    let (xuid, key) = server.account("Owner");
    server.embedded.live.grant_admin_runtime(xuid, "Owner");
    // 33 messages: more than the game keeps.
    let doc = json!({"header": "h", "messages": vec!["m"; 33]});
    assert_eq!(server.put_motd(xuid, &key, &doc), Err(400));
    assert_eq!(
        server.motd()["header"],
        live_server::default_motd::HEADER,
        "the previous MOTD stays"
    );
    drop(server);
    let _ = std::fs::remove_dir_all(&dir);
}
