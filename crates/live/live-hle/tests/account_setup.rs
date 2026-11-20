//! In-game account setup against the real embedded memory-backed server:
//! create, recover with the TOTP code, refusals, the accounts file, and a
//! saved account signing in without a restart. Synthetic guest memory only;
//! no XBE, image build or game run.
use live_hle::{
    Hle, LiveConfig, NetworkConfig, ServerOptions,
    account_setup::{self, SetupError},
    entries::ENTRIES,
    status::*,
};
use live_proto::{recovery::TotpSecret, sign};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, exec::HARNESS_EXIT,
    guest::Permissions,
};
use serde_json::Value;
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
        "h2-hle-setup-{what}-{}-{}",
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

#[test]
fn create_then_recover_on_another_machine_with_the_code() {
    let server = Server::new();
    let (entry, recovery) = account_setup::create(server.url(), " Pete ", Some("  ")).unwrap();
    assert_eq!(entry.gamertag, "Pete");
    assert_eq!(entry.server, server.url());
    assert_eq!(entry.id, format!("acct:{}@{}", entry.xuid, entry.server_id));
    let recovery = recovery.expect("a new account comes with recovery");
    assert_eq!(recovery.code.len(), 7);

    // The signed-in machine reads the same secret back.
    let shown = account_setup::recovery_code(server.url(), &entry).unwrap();
    assert_eq!(shown.secret, recovery.secret);

    // Same gamertag again: taken.
    assert_eq!(
        account_setup::create(server.url(), "pete", None).unwrap_err(),
        SetupError::GamertagTaken
    );

    // A wrong code, then the right one, as typed with a dash.
    let right = code_of(&recovery.secret);
    let wrong = format!(
        "{:06}",
        (right.parse::<u32>().unwrap() + 500_000) % 1_000_000
    );
    assert_eq!(
        account_setup::recover(server.url(), "Pete", &wrong).unwrap_err(),
        SetupError::WrongCodeLeft { left: 4 }
    );
    let typed = format!("{}-{}", &right[..3], &right[3..]);
    let recovered = account_setup::recover(server.url(), "pete", &typed).unwrap();
    assert_eq!(recovered.xuid, entry.xuid);
    assert_eq!(recovered.gamertag, "Pete");
    assert_ne!(recovered.key.kid(), entry.key.kid());
    // The new key is the account's: it reads the recovery secret too.
    assert_eq!(
        account_setup::recovery_code(server.url(), &recovered)
            .unwrap()
            .secret,
        recovery.secret
    );
    // A code is used once.
    assert_eq!(
        account_setup::recover(server.url(), "Pete", &typed).unwrap_err(),
        SetupError::WrongCode
    );
    assert_eq!(
        account_setup::recover(server.url(), "Nobody", "123456").unwrap_err(),
        SetupError::NoSuchGamertag
    );

    // A reset makes a new secret; the old one stops working.
    let reset = account_setup::reset_recovery(server.url(), &recovered).unwrap();
    assert_ne!(reset.secret, recovery.secret);
}

#[test]
fn repeated_wrong_codes_lock_the_gamertag_out() {
    let server = Server::new();
    let (_, recovery) = account_setup::create(server.url(), "Locked", None).unwrap();
    let right: u32 = code_of(&recovery.unwrap().secret).parse().unwrap();
    let wrong = format!("{:06}", (right + 333_333) % 1_000_000);
    let mut errors = Vec::new();
    for _ in 0..8 {
        errors.push(account_setup::recover(server.url(), "Locked", &wrong).unwrap_err());
    }
    assert_eq!(errors[0], SetupError::WrongCodeLeft { left: 4 });
    assert!(
        errors
            .iter()
            .any(|e| matches!(e, SetupError::LockedOut { seconds } if *seconds >= 1)),
        "{errors:?}"
    );
}

#[test]
fn an_unreachable_server_is_one_short_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    assert_eq!(
        account_setup::create(&url, "Pete", None).unwrap_err(),
        SetupError::Unreachable
    );
    assert_eq!(
        account_setup::needs_invite(&url).unwrap_err(),
        SetupError::Unreachable
    );
}

#[test]
fn an_open_server_needs_no_invite() {
    let server = Server::new();
    assert_eq!(account_setup::needs_invite(server.url()), Ok(Some(false)));
}

#[test]
fn saving_keeps_other_entries_and_their_fields_at_0600() {
    let server = Server::new();
    let dir = temp("file");
    let path = dir.join("accounts.json");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        &path,
        r#"[{"xuid":"0009000000000099","gamertag":"Other","server":"http://x","key":"old-seed-form","future":{"a":1}},
            {"xuid":"0009000000000098","gamertag":"Stale","server":"http://x","server_id":"SERVER"}]"#
            .replace("SERVER", &account_setup_server_id(&server)),
    )
    .unwrap();
    let (entry, _) = account_setup::create(server.url(), "Stale", None).unwrap();
    account_setup::save(&path, &entry).unwrap();
    // Saving again replaces, never duplicates.
    account_setup::save(&path, &entry).unwrap();
    let all: Vec<Value> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(all.len(), 2, "{all:?}");
    assert_eq!(all[0]["future"]["a"], 1);
    assert_eq!(all[0]["key"], "old-seed-form");
    let loaded = account_setup::load(&path).unwrap();
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].key.kid(), entry.key.kid());
    assert_eq!(loaded[0].server_id, entry.server_id);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let leftovers = std::fs::read_dir(&dir).unwrap().count();
    assert_eq!(leftovers, 1, "no temporary file is left");
    std::fs::remove_dir_all(&dir).unwrap();
}

fn account_setup_server_id(server: &Server) -> String {
    let d: live_proto::Discovery = serde_json::from_reader(
        ureq::get(&format!("{}/.well-known/h2live.json", server.url()))
            .call()
            .unwrap()
            .into_reader(),
    )
    .unwrap();
    d.id
}

// A saved account signs in without a restart.

const DATA: u32 = 0x700000;
const OUT: u32 = DATA + 0x1000;
const USERS: u32 = DATA + 0x2000;
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
    fn copy(&self, from: u32, to: u32, n: usize) {
        let mut b = vec![0; n];
        let mut m = self.rt.memory();
        m.read_bytes(from, &mut b, 1).unwrap();
        m.write_bytes(to, &b, 1).unwrap();
    }
}

fn hle_with(path: &Path, url: &str) -> Arc<Hle> {
    Hle::with_server_options(
        LiveConfig {
            enabled: true,
            accounts_path: Some(path.into()),
            server_url: Some(url.into()),
            offer_new_account: false,
        },
        NetworkConfig::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 31900)),
        ServerOptions {
            request_timeout: Duration::from_secs(2),
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn a_saved_account_signs_in_after_a_reload() {
    let server = Server::new();
    let dir = temp("signin");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("play-accounts.json");
    std::fs::write(&path, "[]").unwrap();
    let hle = hle_with(&path, server.url());
    let mut w = World::new(&hle);
    assert_eq!(w.call("XNetStartup", &[0]), 0);
    assert_eq!(w.call("WSAStartup", &[2, DATA]), 0);
    assert_eq!(w.call("XOnlineStartup", &[0]), 0);
    assert_eq!(w.call("XOnlineGetUsers", &[DATA, OUT]), 0);
    assert_eq!(w.u32(OUT), 0, "no accounts yet");

    let (entry, _) = account_setup::create(server.url(), "Fresh", None).unwrap();
    account_setup::save(&path, &entry).unwrap();
    assert_eq!(hle.reload_accounts().unwrap(), 1);

    assert_eq!(w.call("XOnlineGetUsers", &[DATA, OUT]), 0);
    assert_eq!(w.u32(OUT), 1);
    assert_eq!(
        u64::from(w.u32(DATA)) | u64::from(w.u32(DATA + 4)) << 32,
        entry.xuid.0
    );
    w.copy(DATA, USERS, live_hle::USER_SIZE);
    w.rt.memory().set_u32(DATA + 0x4000, 2).unwrap();
    assert_eq!(
        w.call("XOnlineLogon", &[USERS, DATA + 0x4000, 1, 0, OUT]),
        0
    );
    let task = w.u32(OUT);
    let limit = Instant::now() + Duration::from_secs(6);
    let result = loop {
        let r = w.call("XOnlineTaskContinue", &[task]);
        if r != 0 {
            break r;
        }
        assert!(Instant::now() < limit, "logon timed out");
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(result, CONNECTED);

    // A broken file changes nothing.
    std::fs::write(&path, "not json").unwrap();
    assert!(hle.reload_accounts().is_err());
    assert_eq!(w.call("XOnlineGetUsers", &[DATA, OUT]), 0);
    assert_eq!(w.u32(OUT), 1);
    std::fs::remove_dir_all(&dir).unwrap();
}
