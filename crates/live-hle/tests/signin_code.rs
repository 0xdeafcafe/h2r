//! In-game sign-in with a code (docs/live/live.md, "In-game sign-in"):
//! an identity listed with `needs_code` signs in through XOnlineLogon,
//! which stays pending while the prompt asks for the code, checks it on
//! its own thread against the real embedded server (with real TOTP codes),
//! saves the key to both accounts files and goes on to CONNECTED.
//! Synthetic guest memory only; no XBE, image build or game run.
use live_hle::{
    Hle, LiveConfig, NetworkConfig, ServerOptions,
    account_setup::{self, SetupError},
    entries::ENTRIES,
    signin::Phase,
    status::*,
};
use live_proto::{recovery::TotpSecret, sign};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, exec::HARNESS_EXIT,
    guest::Permissions,
};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

struct Server {
    embedded: live_server::Embedded,
    dir: PathBuf,
}
impl Server {
    fn new() -> Self {
        let dir = temp("server");
        let embedded = live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &dir,
            live_server::http::Site::default(),
        )
        .unwrap();
        Self { embedded, dir }
    }
    fn url(&self) -> &str {
        &self.embedded.url
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn temp(what: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "h2-hle-signin-{what}-{}-{}",
        std::process::id(),
        sign::random_id()
    ))
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn code_of(secret: &str) -> String {
    format!(
        "{:06}",
        TotpSecret::from_base32(secret).unwrap().code_at(now())
    )
}

fn wrong(right: &str) -> String {
    format!(
        "{:06}",
        (right.parse::<u32>().unwrap() + 500_000) % 1_000_000
    )
}

const DATA: u32 = 0x700000;
const OUT: u32 = DATA + 0x1000;
const USERS: u32 = DATA + 0x2000;
const SERVICES: u32 = DATA + 0x4000;
const FRAME: u32 = 0x600f00;

struct World {
    rt: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
}
impl World {
    fn new(hle: &Arc<Hle>) -> Self {
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
        live_hle::register(&mut dispatch, hle).unwrap();
        let rt = Runtime::new(guest, dispatch);
        let ctx = ExecCtx::new(rt.clone(), 1);
        Self {
            rt,
            ctx,
            cpu: CpuAbi::default(),
        }
    }
    fn call(&mut self, name: &str, args: &[u32]) -> u32 {
        let e = ENTRIES.iter().find(|e| e.name == name).unwrap();
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
        assert!(matches!(
            self.ctx.event(t.event_id),
            Some(Event::HarnessExit)
        ));
        self.cpu.eax
    }
    fn u32(&self, p: u32) -> u32 {
        self.rt.memory().u32(p).unwrap()
    }
    fn xuid(&self, p: u32) -> u64 {
        u64::from(self.u32(p)) | u64::from(self.u32(p + 4)) << 32
    }
    fn start(&mut self) {
        assert_eq!(self.call("XNetStartup", &[0]), 0);
        assert_eq!(self.call("WSAStartup", &[2, DATA]), 0);
        assert_eq!(self.call("XOnlineStartup", &[0]), 0);
    }
    /// Lists the accounts and signs the first in on controller 0.
    fn logon_first(&mut self) -> u32 {
        assert_eq!(self.call("XOnlineGetUsers", &[DATA, OUT]), 0);
        let mut b = vec![0; live_hle::USER_SIZE];
        {
            let mut m = self.rt.memory();
            m.read_bytes(DATA, &mut b, 1).unwrap();
            m.write_bytes(USERS, &b, 1).unwrap();
            m.set_u32(SERVICES, 2).unwrap();
        }
        assert_eq!(self.call("XOnlineLogon", &[USERS, SERVICES, 1, 0, OUT]), 0);
        self.u32(OUT)
    }
    /// Pumps the logon until it is no longer pending.
    fn finish(&mut self, task: u32) -> u32 {
        let limit = Instant::now() + Duration::from_secs(8);
        loop {
            let r = self.call("XOnlineTaskContinue", &[task]);
            if r != 0 {
                return r;
            }
            assert!(Instant::now() < limit, "logon timed out");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn hle_with(path: &Path, url: &str) -> Arc<Hle> {
    Hle::with_server_options(
        LiveConfig {
            enabled: true,
            accounts_path: Some(path.into()),
            server_url: Some(url.into()),
        },
        NetworkConfig::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 31910)),
        ServerOptions {
            request_timeout: Duration::from_secs(2),
            ..Default::default()
        },
    )
    .unwrap()
}

/// `h2 play`'s session file holding one identity waiting for a code.
fn session_with(dir: &Path, xuid: live_proto::Xuid, tag: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("play-accounts.json");
    let entry = serde_json::json!([{"xuid": xuid, "gamertag": tag, "needs_code": true}]);
    std::fs::write(&path, entry.to_string()).unwrap();
    path
}

/// The prompt once the phase is no longer Working.
fn settled(hle: &Hle) -> Phase {
    let limit = Instant::now() + Duration::from_secs(8);
    loop {
        let p = hle.signin().expect("a sign-in is waiting");
        if p.phase != Phase::Working {
            return p.phase;
        }
        assert!(Instant::now() < limit, "the server call timed out");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_code_signs_the_identity_in_and_saves_it_to_both_files() {
    let server = Server::new();
    // Made on another device: this machine has no key for it.
    let (made, recovery) = account_setup::create(server.url(), "Roamer", None).unwrap();
    let secret = recovery.unwrap().secret;
    let dir = temp("code");
    let path = session_with(&dir, made.xuid, "Roamer");
    let hle = hle_with(&path, server.url());
    let mut w = World::new(&hle);
    w.start();

    let task = w.logon_first();
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), 0, "pending");
    assert_eq!(hle.take_signin_open(), Some(0), "controller 0's prompt");
    assert_eq!(hle.take_signin_open(), None, "opened once");
    let p = hle.signin().unwrap();
    assert_eq!((p.gamertag.as_str(), p.controller), ("Roamer", 0));
    assert_eq!(p.phase, Phase::Code { error: None });

    // Not six digits: refused with no request.
    assert!(hle.signin_submit("12"));
    assert_eq!(
        hle.signin().unwrap().phase,
        Phase::Code {
            error: Some(SetupError::WrongCode)
        }
    );
    // A wrong code: tries left, still pending, nothing saved.
    let right = code_of(&secret);
    assert!(hle.signin_submit(&wrong(&right)));
    assert_eq!(hle.signin().unwrap().phase, Phase::Working);
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), 0);
    assert_eq!(
        settled(&hle),
        Phase::Code {
            error: Some(SetupError::WrongCodeLeft { left: 4 })
        }
    );
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), 0);
    assert!(account_setup::load(&path).unwrap().is_empty());

    // The right code: verified, saved to both files, then signed in.
    assert!(hle.signin_submit(&code_of(&secret)));
    assert!(!hle.signin_create(), "no create while checking a code");
    assert_eq!(settled(&hle), Phase::Done);
    assert_eq!(w.finish(task), CONNECTED);
    for file in [path.clone(), dir.join("accounts.json")] {
        let saved = account_setup::load(&file).unwrap();
        assert_eq!(saved.len(), 1, "{}", file.display());
        assert_eq!(saved[0].xuid, made.xuid);
        assert_ne!(saved[0].key.kid(), made.key.kid(), "this machine's own key");
    }
    // The signed-in user is the account itself.
    let users = w.call("XOnlineGetLogonUsers", &[]);
    assert_eq!(w.xuid(users), made.xuid.0);
    assert_eq!(w.u32(users + 108), 0);
    // Listed once, now ready.
    assert_eq!(w.call("XOnlineGetUsers", &[DATA, OUT]), 0);
    assert_eq!(w.u32(OUT), 1);
    hle.signin_dismiss();
    assert_eq!(hle.signin(), None, "forgotten once closed");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn backing_out_fails_the_waiting_logon() {
    let server = Server::new();
    let (made, _) = account_setup::create(server.url(), "Shy", None).unwrap();
    let dir = temp("cancel");
    let path = session_with(&dir, made.xuid, "Shy");
    let hle = hle_with(&path, server.url());
    let mut w = World::new(&hle);
    w.start();
    let task = w.logon_first();
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), 0);
    hle.signin_cancel();
    assert_eq!(hle.signin().unwrap().phase, Phase::Cancelled);
    assert!(!hle.signin_submit("123456"), "nothing asked any more");
    assert_eq!(w.finish(task), INVALID_USER);
    assert_eq!(w.u32(USERS + 108), 0, "the guest's input is not written");
    hle.signin_dismiss();
    assert_eq!(hle.signin(), None);
    // Closing the logon lets another start.
    assert_eq!(w.call("XOnlineCleanup", &[]), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_gamertag_not_on_the_server_is_made_there() {
    let server = Server::new();
    let dir = temp("create");
    let path = session_with(&dir, live_proto::Xuid(0x0009_0000_0000_7777), "Newcomer");
    let hle = hle_with(&path, server.url());
    let mut w = World::new(&hle);
    w.start();
    let task = w.logon_first();
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), 0);
    assert!(hle.signin_submit("123456"));
    assert_eq!(settled(&hle), Phase::Create { error: None });
    assert!(hle.signin_create());
    let Phase::Created { info: Some(info) } = settled(&hle) else {
        panic!("made, with its recovery secret");
    };
    assert_eq!(info.code.len(), 7);
    assert_eq!(w.finish(task), CONNECTED);
    let saved = account_setup::load(&path).unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].gamertag, "Newcomer");
    let users = w.call("XOnlineGetLogonUsers", &[]);
    assert_eq!(w.xuid(users), saved[0].xuid.0, "the server's xuid");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn wrong_codes_count_down_then_lock_the_gamertag_out() {
    let server = Server::new();
    let (made, recovery) = account_setup::create(server.url(), "Fumbler", None).unwrap();
    let right = code_of(&recovery.unwrap().secret);
    let dir = temp("lock");
    let path = session_with(&dir, made.xuid, "Fumbler");
    let hle = hle_with(&path, server.url());
    let mut w = World::new(&hle);
    w.start();
    let task = w.logon_first();
    let mut seen = Vec::new();
    for _ in 0..6 {
        assert!(hle.signin_submit(&wrong(&right)));
        let Phase::Code { error: Some(e) } = settled(&hle) else {
            panic!("the code is asked again");
        };
        seen.push(e);
        assert_eq!(w.call("XOnlineTaskContinue", &[task]), 0, "still waiting");
    }
    assert_eq!(
        seen[..5],
        [4, 3, 2, 1, 0].map(|left| SetupError::WrongCodeLeft { left })
    );
    assert!(
        matches!(seen[5], SetupError::LockedOut { seconds } if seconds >= 1),
        "{seen:?}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_unreachable_server_is_said_and_the_code_asked_again() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let dir = temp("down");
    let path = session_with(&dir, live_proto::Xuid(0x0009_0000_0000_1234), "Lonely");
    let hle = hle_with(&path, &url);
    let mut w = World::new(&hle);
    w.start();
    let task = w.logon_first();
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), 0);
    assert!(hle.signin_submit("123456"));
    assert_eq!(
        settled(&hle),
        Phase::Code {
            error: Some(SetupError::Unreachable)
        }
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn without_a_server_an_identity_waiting_for_a_code_is_an_invalid_user() {
    let dir = temp("local");
    let path = session_with(&dir, live_proto::Xuid(0x0009_0000_0000_4321), "Offline");
    let hle = Hle::with_address(
        LiveConfig {
            enabled: true,
            accounts_path: Some(path),
            server_url: None,
        },
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, 31911),
    )
    .unwrap();
    let mut w = World::new(&hle);
    w.start();
    let _ = w.logon_first_expecting(INVALID_USER);
    assert_eq!(hle.signin(), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

impl World {
    fn logon_first_expecting(&mut self, status: u32) -> u32 {
        assert_eq!(self.call("XOnlineGetUsers", &[DATA, OUT]), 0);
        let mut b = vec![0; live_hle::USER_SIZE];
        {
            let mut m = self.rt.memory();
            m.read_bytes(DATA, &mut b, 1).unwrap();
            m.write_bytes(USERS, &b, 1).unwrap();
        }
        assert_eq!(
            self.call("XOnlineLogon", &[USERS, SERVICES, 0, 0, OUT]),
            status
        );
        status
    }
}
