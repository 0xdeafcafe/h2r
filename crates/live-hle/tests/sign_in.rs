//! Synthetic public ABI calls through the real Runner/Dispatch; no XBE or game.
use live_hle::{Hle, LiveConfig, entries::ENTRIES, register, status::*, unregister};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi, abi,
    exec::HARNESS_EXIT,
    guest::{Access, Permissions},
};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

const DATA: u32 = 0x0070_0000;
const OUTPUT: u32 = DATA + 0x1000;
const USERS: u32 = DATA + 0x2000;
const SERVICES: u32 = DATA + 0x3000;
const FRAME: u32 = 0x0060_0f00;
const ID: u64 = 0x0009_1234_5678_90ab;
static FILE: AtomicU64 = AtomicU64::new(0);

fn config() -> LiveConfig {
    let path = std::env::temp_dir().join(format!(
        "live-hle-{}-{}.json",
        std::process::id(),
        FILE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, r#"[
        {"xuid":"00091234567890ab","gamertag":"First Player","server":"https://home.example","key":{"kty":"OKP","crv":"Ed25519","x":"public-test-marker","d":"private-test-marker"}},
        {"xuid":"0009000011112222","gamertag":"Second"}
    ]"#).unwrap();
    LiveConfig {
        enabled: true,
        accounts_path: Some(path),
        server_url: Some("https://live.example/base".into()),
    }
}
fn hle(config: LiveConfig) -> Arc<Hle> {
    let h = Hle::with_address(
        config.clone(),
        SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 42), 3074),
    )
    .unwrap();
    if config.enabled
        && let Some(p) = config.accounts_path
    {
        std::fs::remove_file(p).unwrap();
    }
    h
}
struct World {
    rt: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
}
impl World {
    fn new() -> Self {
        Self::with(hle(config()))
    }
    fn with(hle: Arc<Hle>) -> Self {
        let mut guest = Guest::new();
        guest
            .map_zeroed(0x0036_c000, 0x1000, Permissions::RX)
            .unwrap();
        guest
            .map_zeroed(0x003a_0000, 0x40000, Permissions::RX)
            .unwrap();
        guest
            .map_zeroed(0x0060_0000, 0x1000, Permissions::RW)
            .unwrap();
        guest.map_zeroed(DATA, 0x10000, Permissions::RW).unwrap();
        let mut dispatch = Dispatch::default();
        register(&mut dispatch, &hle).unwrap();
        let rt = Runtime::new(guest, dispatch);
        let ctx = ExecCtx::new(Arc::clone(&rt), 1);
        Self {
            rt,
            ctx,
            cpu: CpuAbi::default(),
        }
    }
    fn entry(name: &str) -> live_hle::entries::Entry {
        *ENTRIES.iter().find(|e| e.name == name).unwrap()
    }
    fn prepare(&mut self, name: &str, args: &[u32]) -> u32 {
        let entry = Self::entry(name);
        assert_eq!(
            entry.stack_words as usize,
            args.len(),
            "{name} argument frame"
        );
        let mut m = self.rt.memory();
        m.write_bytes(FRAME, &[0xcd; 0x100], 1).unwrap();
        m.set_u32(FRAME, HARNESS_EXIT).unwrap();
        for (i, &arg) in args.iter().enumerate() {
            m.set_u32(FRAME + 4 + i as u32 * 4, arg).unwrap();
        }
        self.cpu = CpuAbi {
            esp: FRAME,
            eax: 0xdead_beef,
            ecx: 0x1234,
            edx: 0x5678,
            ebx: 0x1111,
            esi: 0x2222,
            edi: 0x3333,
            ebp: 0x4444,
            eip: entry.va,
            ..CpuAbi::default()
        };
        entry.va
    }
    fn raw(&mut self, name: &str, args: &[u32]) -> TransferAbi {
        let va = self.prepare(name, args);
        Runner::default().run(&mut self.cpu, &mut self.ctx, va)
    }
    fn call(&mut self, name: &str, args: &[u32]) -> u32 {
        let result = self.raw(name, args);
        assert_eq!(result.kind, abi::STOP);
        assert!(
            matches!(self.ctx.event(result.event_id), Some(Event::HarnessExit)),
            "{name}: {:?}",
            self.ctx.event(result.event_id)
        );
        assert_eq!(
            self.cpu.esp,
            FRAME + 4 + args.len() as u32 * 4,
            "{name} stdcall return"
        );
        assert_eq!(
            (
                self.cpu.ecx,
                self.cpu.edx,
                self.cpu.ebx,
                self.cpu.esi,
                self.cpu.edi,
                self.cpu.ebp
            ),
            (0x1234, 0x5678, 0x1111, 0x2222, 0x3333, 0x4444)
        );
        self.cpu.eax
    }
    fn put(&self, p: u32, bytes: &[u8]) {
        self.rt.memory().write_bytes(p, bytes, 1).unwrap();
    }
    fn word(&self, p: u32, v: u32) {
        self.rt.memory().set_u32(p, v).unwrap();
    }
    fn u32(&self, p: u32) -> u32 {
        self.rt.memory().u32(p).unwrap()
    }
    fn bytes(&self, p: u32, size: usize) -> Vec<u8> {
        let mut out = vec![0; size];
        self.rt.memory().read_bytes(p, &mut out, 1).unwrap();
        out
    }
    fn startup(&mut self) {
        assert_eq!(self.call("XNetStartup", &[0]), OK);
        assert_eq!(self.call("WSAStartup", &[2, DATA + 0x4000]), OK);
        assert_eq!(self.call("XOnlineStartup", &[0]), OK);
    }
    fn logon(&mut self) -> u32 {
        self.startup();
        assert_eq!(self.call("XOnlineGetUsers", &[DATA, OUTPUT]), OK);
        self.put(USERS, &self.bytes(DATA, 112));
        for i in 0..13 {
            self.word(SERVICES + 4 * i, 2 + i);
        }
        assert_eq!(
            self.call("XOnlineLogon", &[USERS, SERVICES, 13, 0, OUTPUT]),
            OK
        );
        self.u32(OUTPUT)
    }
    fn task(&mut self, name: &str) -> u32 {
        let args = match name {
            "XOnlineFriendsStartup" | "XOnlineMutelistStartup" => vec![0, OUTPUT],
            "XOnlineOfferingIsNewContentAvailable" | "XOnlinePresenceInit" => vec![0, 0, OUTPUT],
            _ => panic!("not a simple empty task"),
        };
        assert_eq!(self.call(name, &args), OK);
        self.u32(OUTPUT)
    }
}

#[test]
fn boundary_inventory_matches_the_report_and_has_no_duplicate_addresses() {
    let doc = include_str!("../../../docs/live-boundary.md");
    let table = doc
        .split("## Verified target inventory")
        .nth(1)
        .unwrap()
        .split("## Sign-in as driven")
        .next()
        .unwrap();
    let mut addresses = std::collections::BTreeSet::new();
    for e in ENTRIES {
        assert!(addresses.insert(e.va));
        let row = table
            .lines()
            .find(|l| l.starts_with(&format!("| `0x{:08x}`", e.va)))
            .unwrap();
        assert!(row.contains(&format!("`{}`", e.name)));
        assert_eq!(
            row.split('|')
                .nth(6)
                .unwrap()
                .trim()
                .split('/')
                .nth(1)
                .unwrap()
                .trim()
                .parse::<u32>()
                .unwrap(),
            e.stack_words
        );
    }
    assert_eq!(ENTRIES.len(), 130);
}
#[test]
fn disabled_registers_nothing_and_never_reads_an_accounts_file() {
    let h = hle(LiveConfig {
        enabled: false,
        accounts_path: Some("/does/not/exist".into()),
        server_url: None,
    });
    let mut d = Dispatch::default();
    assert!(register(&mut d, &h).unwrap().installed.is_empty());
    for e in ENTRIES {
        assert!(d.override_name(e.va).is_none());
    }
}
#[test]
fn conflicts_are_rejected_without_partial_registration() {
    let h = hle(config());
    let mut d = Dispatch::default();
    let last = ENTRIES.last().unwrap().va;
    d.register_override(last, "prior", |_, _, _| Ok(TransferAbi::stop(0)));
    assert!(register(&mut d, &h).is_err());
    assert_eq!(d.override_name(last), Some("prior"));
    assert!(d.override_name(ENTRIES[0].va).is_none());
}
#[test]
fn unregister_preserves_other_overrides() {
    let h = hle(config());
    let mut d = Dispatch::default();
    let reg = register(&mut d, &h).unwrap();
    d.register_override(0x10000, "other", |_, _, _| Ok(TransferAbi::stop(0)));
    unregister(&mut d, &reg);
    assert_eq!(d.override_name(0x10000), Some("other"));
}
#[test]
fn link_status_is_active() {
    assert_eq!(World::new().call("XNetGetEthernetLinkStatus", &[]), 0x0d);
}
#[test]
fn xnet_startup_accepts_default_and_twelve_byte_parameters() {
    let mut w = World::new();
    w.put(DATA, &[0; 12]);
    w.put(DATA, &[12]);
    assert_eq!(w.call("XNetStartup", &[DATA]), OK);
    w.word(DATA, 11);
    assert_eq!(w.call("XNetStartup", &[DATA]), WSA_INVALID_ARGUMENT);
    assert_eq!(w.call("XNetStartup", &[0]), OK);
}
#[test]
fn winsock_startup_writes_versioned_wsadata_without_overrun() {
    let mut w = World::new();
    assert_eq!(w.call("WSAStartup", &[2, DATA]), WSA_NOT_INITIALIZED);
    w.call("XNetStartup", &[0]);
    w.put(DATA, &[0xaa; 404]);
    assert_eq!(w.call("WSAStartup", &[2, DATA]), OK);
    assert_eq!(&w.bytes(DATA, 4), &[2, 0, 2, 2]);
    assert!(w.bytes(DATA + 4, 396).iter().all(|b| *b == 0));
    assert_eq!(w.bytes(DATA + 400, 4), vec![0xaa; 4]);
    assert_eq!(w.call("WSAStartup", &[1, DATA]), WSA_VERSION);
}
#[test]
fn online_startup_requires_transport_and_accepts_parameters() {
    let mut w = World::new();
    assert_eq!(w.call("XOnlineStartup", &[0]), NOT_INITIALIZED);
    w.call("XNetStartup", &[0]);
    assert_eq!(w.call("XOnlineStartup", &[0]), NOT_INITIALIZED);
    w.call("WSAStartup", &[2, DATA]);
    assert_eq!(w.call("XOnlineStartup", &[OUTPUT]), OK);
}
#[test]
fn title_xnaddr_contains_host_ipv4_network_order_port_and_static_bit() {
    let mut w = World::new();
    assert_eq!(w.call("XNetGetTitleXnAddr", &[DATA]), 1);
    w.call("XNetStartup", &[0]);
    w.put(DATA, &[0xaa; 40]);
    assert_eq!(w.call("XNetGetTitleXnAddr", &[DATA]), 4);
    assert_eq!(
        w.bytes(DATA, 10),
        vec![192, 168, 1, 42, 192, 168, 1, 42, 12, 2]
    );
    assert_eq!(w.bytes(DATA + 10, 6), vec![2, 0, 0, 0, 0, 1]);
    assert_eq!(w.bytes(DATA + 36, 4), vec![0xaa; 4]);
}
#[test]
fn get_users_writes_112_byte_records_and_output_only_count() {
    let mut w = World::new();
    w.startup();
    w.put(DATA, &[0xaa; 1796]);
    w.word(OUTPUT, u32::MAX);
    assert_eq!(w.call("XOnlineGetUsers", &[DATA, OUTPUT]), OK);
    assert_eq!(w.u32(OUTPUT), 2);
    assert_eq!(w.bytes(DATA, 8), ID.to_le_bytes());
    assert_eq!(w.bytes(DATA + 12, 16), b"First Player\0\0\0\0");
    assert_eq!(w.u32(DATA + 108), 0);
    assert_eq!(
        w.bytes(DATA + 112, 8),
        0x0009_0000_1111_2222u64.to_le_bytes()
    );
    assert!(w.bytes(DATA + 224, 1792 - 224).iter().all(|b| *b == 0));
    assert_eq!(w.bytes(DATA + 1792, 4), vec![0xaa; 4]);
}
#[test]
fn logon_creates_a_persistent_guest_handle_and_rejects_duplicates() {
    let mut w = World::new();
    let h = w.logon();
    w.rt.memory().check(h, 8, Access::Read, 1).unwrap();
    assert_eq!(
        w.call("XOnlineLogon", &[USERS, SERVICES, 13, 0, OUTPUT]),
        TASK_BUSY
    );
    for _ in 0..4 {
        assert_eq!(w.call("XOnlineTaskContinue", &[h]), CONNECTED);
    }
}
#[test]
fn logon_results_reads_without_pumping() {
    let mut w = World::new();
    let h = w.logon();
    assert_eq!(w.call("XOnlineLogonTaskGetResults", &[h]), CONNECTED);
    assert_eq!(
        w.call("XOnlineLogonTaskGetResults", &[h + 1]),
        INVALID_ARGUMENT
    );
}
#[test]
fn logged_on_users_are_stable_guest_memory_with_success_per_user() {
    let mut w = World::new();
    let h = w.logon();
    let p = w.call("XOnlineGetLogonUsers", &[]);
    assert_ne!(p, 0);
    assert_eq!(w.bytes(p, 8), ID.to_le_bytes());
    assert_eq!(w.u32(p + 108), 0);
    w.call("XOnlineTaskContinue", &[h]);
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), p);
    assert!(w.bytes(p + 112, 336).iter().all(|b| *b == 0));
}
#[test]
fn service_info_accepts_every_requested_id_and_optional_null_output() {
    let mut w = World::new();
    w.logon();
    for id in 2..15 {
        assert_eq!(w.call("XOnlineGetServiceInfo", &[id, 0]), OK);
    }
    w.put(OUTPUT, &[0xaa; 16]);
    assert_eq!(w.call("XOnlineGetServiceInfo", &[13, OUTPUT]), OK);
    assert_eq!(w.u32(OUTPUT), 13);
    assert_eq!(w.bytes(OUTPUT + 4, 6), vec![192, 168, 1, 42, 12, 2]);
    assert_eq!(w.bytes(OUTPUT + 12, 4), vec![0xaa; 4]);
    assert_eq!(
        w.call("XOnlineGetServiceInfo", &[99, 0]),
        SERVICE_NOT_REQUESTED
    );
}
#[test]
fn qos_lookup_completes_in_guest_memory_with_no_data() {
    let mut w = World::new();
    w.call("XNetStartup", &[0]);
    assert_eq!(w.call("XNetQosServiceLookup", &[0, 0, OUTPUT]), OK);
    let p = w.u32(OUTPUT);
    assert_eq!(w.u32(p), 1); // One complete XNQOSINFO, which Halo reads directly.
    assert_eq!(w.u32(p + 4), 0); // No pending work.
    assert_eq!(w.bytes(p + 8, 1), vec![1]); // COMPLETE, without TARGET_CONTACTED.
    assert_eq!(w.bytes(p + 9, 23), vec![0; 23]);
    w.call("XNetQosServiceLookup", &[0, 0, OUTPUT]);
    assert!(w.u32(OUTPUT) >= p + 32); // A later allocation cannot occupy entry 0.
}
#[test]
fn qos_release_rejects_stale_and_unknown_records() {
    let mut w = World::new();
    w.call("XNetStartup", &[0]);
    w.call("XNetQosServiceLookup", &[0, 0, OUTPUT]);
    let p = w.u32(OUTPUT);
    assert_eq!(w.call("XNetQosRelease", &[p]), OK);
    assert_eq!(w.call("XNetQosRelease", &[p]), WSA_INVALID_ARGUMENT);
    w.call("XNetQosServiceLookup", &[0, 0, OUTPUT]);
    assert_ne!(p, w.u32(OUTPUT));
}
#[test]
fn notification_reads_by_value_session_id_and_state_buffer() {
    let mut w = World::new();
    w.logon();
    w.word(OUTPUT, 0x12345678);
    assert_eq!(
        w.call(
            "XOnlineNotificationSetState",
            &[0, 1, 0x1234, 0x5678, 4, OUTPUT]
        ),
        OK
    );
    assert_eq!(w.u32(OUTPUT), 0x12345678);
    assert_eq!(
        w.call("XOnlineNotificationSetState", &[4, 1, 0, 0, 0, 0]),
        INVALID_ARGUMENT
    );
}
#[test]
fn change_users_commits_then_completes_and_keeps_the_array_address() {
    let mut w = World::new();
    let h = w.logon();
    let p = w.call("XOnlineGetLogonUsers", &[]);
    w.put(USERS, &[0; 448]);
    w.put(USERS + 112, &w.bytes(DATA + 112, 112));
    assert_eq!(w.call("XOnlineChangeLogonUsers", &[USERS, 0, OUTPUT]), OK);
    let change = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[change]), USER_CHANGE_COMMIT);
    assert_eq!(
        w.call("XOnlineTaskContinue", &[change]),
        USER_CHANGE_COMPLETE
    );
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), p);
    assert_eq!(w.bytes(p, 112), vec![0; 112]);
    assert_ne!(w.u32(p + 112), 0);
    assert_eq!(w.call("XOnlineLogonTaskGetResults", &[h]), CONNECTED);
}
#[test]
fn change_results_writes_four_success_hresults_only() {
    let mut w = World::new();
    w.logon();
    w.call("XOnlineChangeLogonUsers", &[USERS, 0, OUTPUT]);
    let c = w.u32(OUTPUT);
    w.put(OUTPUT, &[0xaa; 20]);
    assert_eq!(
        w.call("XOnlineChangeLogonUsersTaskGetResults", &[c, OUTPUT]),
        OK
    );
    assert_eq!(w.bytes(OUTPUT, 16), vec![0; 16]);
    assert_eq!(w.bytes(OUTPUT + 16, 4), vec![0xaa; 4]);
}
#[test]
fn task_close_signs_out_and_invalidates_old_task_handles() {
    let mut w = World::new();
    let h = w.logon();
    let t = w.task("XOnlinePresenceInit");
    assert_eq!(w.call("XOnlineTaskClose", &[h]), OK);
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
    assert_eq!(w.call("XOnlineTaskContinue", &[t]), INVALID_ARGUMENT);
    assert_eq!(w.call("XOnlineTaskClose", &[h]), INVALID_ARGUMENT);
    assert_eq!(
        w.call("XOnlineLogon", &[USERS, SERVICES, 13, 0, OUTPUT]),
        OK
    );
    assert_ne!(w.u32(OUTPUT), h);
}
#[test]
fn online_cleanup_is_reference_counted_and_invalidates_users_and_tasks() {
    let mut w = World::new();
    let h = w.logon();
    w.call("XOnlineStartup", &[0]);
    assert_eq!(w.call("XOnlineCleanup", &[]), OK);
    assert_eq!(w.call("XOnlineLogonTaskGetResults", &[h]), CONNECTED);
    assert_eq!(w.call("XOnlineCleanup", &[]), OK);
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), NOT_INITIALIZED);
    assert_eq!(w.call("XOnlineCleanup", &[]), NOT_INITIALIZED);
}
#[test]
fn net_cleanup_pairs_startups_and_clears_qos() {
    let mut w = World::new();
    w.call("XNetStartup", &[0]);
    w.call("XNetStartup", &[0]);
    w.call("XNetQosServiceLookup", &[0, 0, OUTPUT]);
    let p = w.u32(OUTPUT);
    assert_eq!(w.call("XNetCleanup", &[]), OK);
    assert_eq!(w.call("XNetGetTitleXnAddr", &[DATA]), 4);
    assert_eq!(w.call("XNetCleanup", &[]), OK);
    assert_eq!(w.call("XNetGetTitleXnAddr", &[DATA]), 1);
    assert_eq!(w.call("XNetQosRelease", &[p]), WSA_INVALID_ARGUMENT);
    assert_eq!(w.call("XNetCleanup", &[]), WSA_NOT_INITIALIZED);
}
#[test]
fn winsock_cleanup_is_reference_counted() {
    let mut w = World::new();
    w.startup();
    w.call("WSAStartup", &[2, DATA]);
    assert_eq!(w.call("WSACleanup", &[]), OK);
    assert_eq!(w.call("WSACleanup", &[]), OK);
    assert_eq!(w.call("WSACleanup", &[]), WSA_NOT_INITIALIZED);
}
#[test]
fn friends_startup_completes_then_idles_for_the_disposal_pump() {
    let mut w = World::new();
    w.logon();
    let h = w.task("XOnlineFriendsStartup");
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), TASK_SUCCESS);
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), RUNNING_IDLE);
    assert_eq!(w.call("XOnlineTaskClose", &[h]), OK);
}
#[test]
fn offering_check_reports_no_new_content() {
    let mut w = World::new();
    w.logon();
    let h = w.task("XOnlineOfferingIsNewContentAvailable");
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), NO_NEW_CONTENT);
}
#[test]
fn presence_init_returns_a_completing_empty_task() {
    let mut w = World::new();
    w.logon();
    let h = w.task("XOnlinePresenceInit");
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), TASK_SUCCESS);
}
#[test]
fn presence_add_reads_a_packed_xuid_array() {
    let mut w = World::new();
    w.logon();
    let h = w.task("XOnlinePresenceInit");
    assert_eq!(w.call("XOnlinePresenceAdd", &[h, 7, 2, USERS]), OK);
    let t = w.raw("XOnlinePresenceAdd", &[h, 7, 1, 0xfffffff0]);
    assert!(matches!(
        w.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
}
#[test]
fn presence_submit_completes_without_data() {
    let mut w = World::new();
    w.logon();
    let h = w.task("XOnlinePresenceInit");
    assert_eq!(w.call("XOnlinePresenceSubmit", &[h]), OK);
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), TASK_SUCCESS);
}
#[test]
fn mutelist_startup_completes_empty() {
    let mut w = World::new();
    w.logon();
    let h = w.task("XOnlineMutelistStartup");
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), TASK_SUCCESS);
}
#[test]
fn mutelist_get_zeroes_count_and_returns_a_completing_task() {
    let mut w = World::new();
    w.logon();
    w.word(OUTPUT + 4, u32::MAX);
    assert_eq!(
        w.call(
            "XOnlineMutelistGet",
            &[0, 250, 0, OUTPUT, DATA + 0x5000, OUTPUT + 4]
        ),
        OK
    );
    let h = w.u32(OUTPUT);
    assert_eq!(w.u32(OUTPUT + 4), 0);
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), TASK_SUCCESS);
}
#[test]
fn unimplemented_entries_stop_by_name_without_reading_stack_or_running_library() {
    let mut w = World::new();
    let implemented = [
        "XNetGetEthernetLinkStatus",
        "XNetStartup",
        "WSAStartup",
        "XOnlineStartup",
        "XNetGetTitleXnAddr",
        "XOnlineGetUsers",
        "XOnlineLogon",
        "XOnlineTaskContinue",
        "XOnlineLogonTaskGetResults",
        "XOnlineGetLogonUsers",
        "XOnlineGetServiceInfo",
        "XNetQosServiceLookup",
        "XNetQosRelease",
        "XOnlineNotificationSetState",
        "XOnlineChangeLogonUsers",
        "XOnlineChangeLogonUsersTaskGetResults",
        "XOnlineTaskClose",
        "XOnlineCleanup",
        "XNetCleanup",
        "WSACleanup",
        "XOnlineFriendsStartup",
        "XOnlineOfferingIsNewContentAvailable",
        "XOnlinePresenceInit",
        "XOnlinePresenceAdd",
        "XOnlinePresenceSubmit",
        "XOnlineMutelistStartup",
        "XOnlineMutelistGet",
    ];
    for e in ENTRIES.iter().filter(|e| !implemented.contains(&e.name)) {
        w.cpu = CpuAbi {
            esp: 0,
            eax: 0x1234,
            eip: e.va,
            ..CpuAbi::default()
        };
        let before = w.cpu;
        let t = Runner::default().run(&mut w.cpu, &mut w.ctx, e.va);
        assert!(
            matches!(w.ctx.event(t.event_id),Some(Event::Unsupported{detail,..}) if detail==&format!("unimplemented Live API {}",e.name)),
            "{}",
            e.name
        );
        assert_eq!(w.cpu, before);
    }
}
#[test]
fn faults_do_not_create_tasks_and_events_stop_without_side_effects() {
    let mut w = World::new();
    w.startup();
    w.call("XOnlineGetUsers", &[DATA, OUTPUT]);
    w.put(USERS, &w.bytes(DATA, 112));
    let t = w.raw("XOnlineLogon", &[USERS, 0, 0, 0, 0xfffffff0]);
    assert!(matches!(
        w.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
    let t = w.raw("XOnlineLogon", &[USERS, 0, 0, 99, OUTPUT]);
    assert!(
        matches!(w.ctx.event(t.event_id),Some(Event::Unsupported{detail,..}) if detail.contains("nonzero work event"))
    );
    assert_eq!(w.cpu.esp, FRAME);
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
}
#[test]
fn guest_storage_skips_existing_mappings_and_is_claimed() {
    let mut w = World::new();
    w.rt.memory()
        .map_zeroed(0xa0000000, 0x1000, Permissions::RW)
        .unwrap();
    w.word(0xa0000000, 0x12345678);
    w.logon();
    let p = w.call("XOnlineGetLogonUsers", &[]);
    assert!(p >= 0xa0001000);
    assert_eq!(w.u32(0xa0000000), 0x12345678);
    assert_eq!(
        w.rt.memory().unmap(p, 0x1000),
        Err(runtime::guest::MapError::Claimed)
    );
}
#[test]
fn invalid_users_do_not_publish_a_logon_or_overwrite_the_handle_output() {
    let mut w = World::new();
    w.startup();
    w.word(USERS, 123);
    w.word(OUTPUT, 0xabcdef);
    assert_eq!(
        w.call("XOnlineLogon", &[USERS, 0, 0, 0, OUTPUT]),
        INVALID_USER
    );
    assert_eq!(w.u32(OUTPUT), 0xabcdef);
    w.word(USERS, 0);
    assert_eq!(w.call("XOnlineLogon", &[USERS, 0, 0, 0, OUTPUT]), NO_USER);
}

#[test]
fn controller_slots_and_guest_flags_survive_logon() {
    let mut w = World::new();
    w.startup();
    w.call("XOnlineGetUsers", &[DATA, OUTPUT]);
    w.put(USERS + 3 * 112, &w.bytes(DATA, 112));
    w.word(USERS + 3 * 112 + 8, 0x102);
    assert_eq!(w.call("XOnlineLogon", &[USERS, 0, 0, 0, OUTPUT]), OK);
    let p = w.call("XOnlineGetLogonUsers", &[]);
    assert_eq!(w.bytes(p, 336), vec![0; 336]);
    assert_eq!(w.bytes(p + 336, 8), ID.to_le_bytes());
    assert_eq!(w.u32(p + 336 + 8), 0x102);
    assert_eq!(w.u32(p + 336 + 108), 0);
}
#[test]
fn bounded_guest_storage_reports_exhaustion_without_reusing_released_tokens() {
    let mut w = World::new();
    w.call("XNetStartup", &[0]);
    let mut seen = std::collections::BTreeSet::new();
    let mut exhausted = false;
    for _ in 0..8192 {
        let result = w.call("XNetQosServiceLookup", &[0, 0, OUTPUT]);
        if result == WSA_NO_BUFFERS {
            exhausted = true;
            break;
        }
        assert_eq!(result, OK);
        let p = w.u32(OUTPUT);
        assert!(seen.insert(p));
        assert_eq!(w.call("XNetQosRelease", &[p]), OK);
    }
    assert!(exhausted);
    assert_eq!(
        w.rt.memory()
            .virtual_claims()
            .iter()
            .filter(|c| c.owner == "Live HLE")
            .count(),
        1
    );
}
#[test]
fn unverified_qos_flags_stop_before_allocating_or_writing_output() {
    let mut w = World::new();
    w.call("XNetStartup", &[0]);
    w.word(OUTPUT, 0x12345678);
    let t = w.raw("XNetQosServiceLookup", &[1, 0, OUTPUT]);
    assert!(
        matches!(w.ctx.event(t.event_id),Some(Event::Unsupported{detail,..}) if detail.contains("QoS service lookup flags"))
    );
    assert_eq!(w.u32(OUTPUT), 0x12345678);
    assert!(w.rt.memory().virtual_claims().is_empty());
}
#[test]
fn allocation_failure_is_a_guest_error_and_does_not_publish_a_logon() {
    let mut w = World::new();
    w.rt.memory()
        .claim_virtual(0xa0000000, 0x1000000, "occupied")
        .unwrap();
    w.call("XNetStartup", &[0]);
    w.call("WSAStartup", &[2, DATA]);
    assert_eq!(w.call("XOnlineStartup", &[0]), OUT_OF_MEMORY);
    assert_eq!(w.call("XOnlineGetLogonUsers", &[]), 0);
}
