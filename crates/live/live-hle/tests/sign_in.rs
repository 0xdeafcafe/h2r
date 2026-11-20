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
        server_url: None,
        offer_new_account: false,
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
    let doc = include_str!("../../../../docs/live/live-boundary.md");
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
        offer_new_account: false,
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
    assert_eq!(w.bytes(DATA + 10, 6), vec![2, 104, 1, 42, 12, 2]);
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
    assert_eq!(w.call("WSACleanup", &[]), u32::MAX);
    assert_eq!(w.call("WSAGetLastError", &[]), WSA_NOT_INITIALIZED);
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
        "XOnlineTeamMemberAnswerRecruit",
        "XOnlineTeamMemberRemove",
        "XOnlineTeamMemberSetProperties",
        "XOnlineTeamDelete",
        "XOnlineTeamCreate",
        "XOnlineMatchSessionCreate",
        "XOnlineMatchSessionUpdate",
        "XOnlineMatchSessionGetInfo",
        "XOnlineMatchSessionDelete",
        "XOnlineMatchSessionFindFromID",
        "XOnlineMatchSearch",
        "XOnlineMatchSearchGetResults",
        "XOnlineMatchSearchParse",
        "XOnlineMatchSearchResultsLen",
        "XOnlineMessageCreate",
        "XOnlineMessageDestroy",
        "XOnlineMessageSetProperty",
        "XOnlineMessageSend",
        "XOnlineFriendsRequestEx",
        "XOnlineFriendsRequestByNameEx",
        "XOnlineGameInviteSend",
        "XOnlineGameInviteAnswer",
        "XOnlineFriendsGameInvite",
        "XOnlineFriendsAnswerGameInvite",
        "XOnlineFriendsJoinGame",
        "XOnlineTeamMemberRecruit",
        "XOnlineTeamMemberRecruitByName",
        "XOnlineStatWrite",
        "XOnlineStatWriteGetResult",
        "XOnlineStorageCreateServerPath",
        "XOnlineStorageDownload",
        "XOnlineStorageDownloadToMemory",
        "XOnlineStorageDownloadToMemoryGetResults",
        "XOnlineStorageUploadByServerPath",
        "XOnlineStorageUploadFromMemory",
        "XOnlineStorageGetProgress",
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
        "XOnlineMutelistAdd",
        "XOnlineMutelistRemove",
        "XNetRandom",
        "XNetCreateKey",
        "XNetRegisterKey",
        "XNetUnregisterKey",
        "XNetXnAddrToInAddr",
        "XNetInAddrToXnAddr",
        "XNetUnregisterInAddr",
        "XNetConnect",
        "XNetGetConnectStatus",
        "XNetQosListen",
        "XNetQosLookup",
        "socket",
        "closesocket",
        "shutdown",
        "ioctlsocket",
        "setsockopt",
        "getsockopt",
        "bind",
        "connect",
        "select",
        "recv",
        "recvfrom",
        "send",
        "sendto",
        "WSAGetLastError",
        "__WSAFDIsSet",
        "htonl/ntohl",
        "htons/ntohs",
        "XOnlineFriendsGetAcceptedGameInvite",
        "XOnlineMessageEnumerate",
        "XOnlineMessageDetails",
        "XOnlineMessageDetailsGetResultsProperty",
        "XOnlineMessageDownloadAttachmentToMemory",
        "XOnlineMessageDownloadAttachmentToMemoryGetResults",
        "XOnlineMessageDelete",
        "XOnlineGetNotificationEx",
        "XOnlineFriendsGetLatest",
        "XOnlineFriendsEnumerate",
        "XOnlineFriendsEnumerateFinish",
        "XOnlineFriendsGetTitleName",
        "XOnlineGetNatType",
        "XOnlineTitleIdIsSameTitle",
        "XOnlineTitleIdIsSamePublisher",
        "XOnlineTeamEnumerateByUserXUID",
        "XOnlineTeamEnumerateGetResults",
        "XOnlineTeamMembersEnumerate",
        "XOnlineTeamMembersEnumerateGetResults",
        "XOnlineTeamGetDetails",
        "XOnlineTeamMemberGetDetails",
        "XOnlineFeedbackSend",
        "XOnlineStatRead",
        "XOnlineStatReadGetResult",
        "XOnlineArbitrationCreateRoundID",
        "XOnlineArbitrationRegister",
        "XOnlineArbitrationReport",
        "XOnlineArbitrationExtendRound",
        "XOnlineQuerySearch",
        "XOnlineQuerySearchGetResults",
        "XOnlinePresenceClear",
        "XOnlinePresenceGetLatest",
        "XOnlineSaveLogonState",
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

#[test]
fn no_accepted_game_invite_is_e_fail_and_clears_the_156_byte_record() {
    let mut w = World::new();
    w.put(DATA, &[0xcc; 160]);
    assert_eq!(
        w.call("XOnlineFriendsGetAcceptedGameInvite", &[DATA]),
        NOT_INITIALIZED
    );
    assert_eq!(w.bytes(DATA, 160), vec![0xcc; 160]);
    w.startup();
    let result = w.call("XOnlineFriendsGetAcceptedGameInvite", &[DATA]);
    assert_eq!(result, NO_ACCEPTED_INVITE);
    assert!((result as i32) < 0, "0x6d210 must report no invite");
    assert_eq!(w.bytes(DATA, 156), vec![0; 156]);
    assert_eq!(w.bytes(DATA + 156, 4), vec![0xcc; 4]);
}

#[test]
fn signed_in_menu_polling_has_correct_empty_counts_bools_and_records() {
    let mut w = World::new();
    w.logon();
    w.put(DATA, &[0xcc; 32]);
    w.word(OUTPUT, u32::MAX);
    assert_eq!(w.call("XOnlineGetNotificationEx", &[0, DATA, OUTPUT]), 0);
    assert_eq!(w.bytes(DATA, 9), vec![0; 9]);
    assert_eq!(w.bytes(DATA + 9, 4), vec![0xcc; 4]);
    assert_eq!(w.u32(OUTPUT), 0);
    assert_eq!(w.call("XOnlineFriendsGetLatest", &[0, 0, 0]), 0);
    assert_eq!(w.call("XOnlineGetNatType", &[]), 0);
    assert_eq!(w.call("XOnlineTitleIdIsSameTitle", &[0x4d530064]), 1);
    assert_eq!(w.call("XOnlineTitleIdIsSameTitle", &[0x4d530001]), 0);
    assert_eq!(w.call("XOnlineTitleIdIsSamePublisher", &[0x4d530001]), 1);
    assert_eq!(w.call("XOnlineTitleIdIsSamePublisher", &[0x54450001]), 0);
    w.put(DATA, &[0xcc; 32]);
    assert_eq!(
        w.call("XOnlineFriendsGetTitleName", &[0x4d530064, 0, 7, DATA]),
        0
    );
    assert_eq!(
        w.bytes(DATA, 14),
        "Halo 2\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>()
    );
    assert_eq!(w.bytes(DATA + 14, 2), vec![0xcc; 2]);
    assert_eq!(
        w.call("XOnlineFriendsGetTitleName", &[0x12345678, 0, 7, DATA]),
        0
    );
    assert_eq!(w.bytes(DATA, 2), vec![0; 2]);
    assert_eq!(w.call("XOnlineFriendsEnumerate", &[0, 0, OUTPUT]), 0);
    let friends = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[friends]), 0x1500f1);
    assert_eq!(w.call("XOnlineFriendsEnumerateFinish", &[friends]), 0);
    assert_eq!(w.call("XOnlineTaskContinue", &[friends]), TASK_SUCCESS);
    assert_eq!(w.call("XOnlineTaskClose", &[friends]), 0);
    assert_eq!(
        w.call("XOnlineFriendsEnumerateFinish", &[friends]),
        INVALID_ARGUMENT
    );
}
#[test]
fn empty_team_enumeration_is_successful_and_missing_team_members_fail_in_the_task() {
    let mut w = World::new();
    w.logon();
    assert_eq!(
        w.call("XOnlineTeamEnumerateByUserXUID", &[0, 1, 0, 0, 0, OUTPUT]),
        0
    );
    let task = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
    w.word(OUTPUT, 0xdeadbeef);
    assert_eq!(
        w.call("XOnlineTeamEnumerateGetResults", &[task, OUTPUT, 0]),
        0
    );
    assert_eq!(w.u32(OUTPUT), 0);
    assert_eq!(
        w.call("XOnlineTeamGetDetails", &[task, 1, 0, 0, 0]),
        0x80004005
    );
    assert_eq!(
        w.call("XOnlineTeamMembersEnumerate", &[0, 1, 0, 0, 1, 0, OUTPUT]),
        0
    );
    let members = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[members]), TEAM_NOT_FOUND);
    assert_eq!(
        w.call(
            "XOnlineTeamMembersEnumerateGetResults",
            &[members, OUTPUT, 0]
        ),
        TEAM_NOT_FOUND
    );
}

#[test]
fn presence_latest_returns_packed_offline_watch_records_and_clear_removes_them() {
    let mut w = World::new();
    w.logon();
    let h = w.task("XOnlinePresenceInit");
    w.put(DATA, &ID.to_le_bytes());
    w.word(DATA + 8, 3);
    assert_eq!(w.call("XOnlinePresenceAdd", &[h, 42, 1, DATA]), 0);
    w.put(USERS, &[0xcc; 126]);
    assert_eq!(w.call("XOnlinePresenceGetLatest", &[h, 42, 2, USERS]), 0);
    assert_eq!(w.bytes(USERS, 12), w.bytes(DATA, 12));
    assert_eq!(w.bytes(USERS + 12, 110), vec![0; 110]);
    assert_eq!(w.bytes(USERS + 122, 4), vec![0xcc; 4]);
    assert_eq!(w.call("XOnlinePresenceClear", &[h]), 0);
    assert_eq!(w.call("XOnlinePresenceGetLatest", &[h, 42, 1, USERS]), 0);
    assert_eq!(w.bytes(USERS, 61), vec![0; 61]);
    assert_eq!(w.call("XOnlineTaskClose", &[h]), 0);
    assert_eq!(
        w.call("XOnlinePresenceGetLatest", &[h, 42, 0, 0]),
        INVALID_ARGUMENT
    );
}
#[test]
fn absent_query_fails_its_task_and_feedback_without_a_server_is_refused() {
    let mut w = World::new();
    w.logon();
    assert_eq!(
        w.call("XOnlineQuerySearch", &[0, 0, 0, 0, 0, 0, 0, 0, 0, OUTPUT]),
        0
    );
    let query = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[query]), 0x80156109);
    assert_eq!(
        w.call("XOnlineQuerySearchGetResults", &[query, 0, 0, 0, 0]),
        0x80156109
    );
    // Feedback goes to the server (tests/server.rs); with none it is
    // refused at the call, so no task claims it was sent.
    w.put(OUTPUT, &[0xcc; 4]);
    assert_eq!(
        w.call("XOnlineFeedbackSend", &[0, 1, 0, 0, 3, 0, 0, OUTPUT]),
        NO_SESSION
    );
    assert_eq!(
        w.call("XOnlineFeedbackSend", &[4, 1, 0, 0, 3, 0, 0, OUTPUT]),
        INVALID_ARGUMENT
    );
    assert_eq!(w.bytes(OUTPUT, 4), vec![0xcc; 4]);
}
#[test]
fn saved_logon_state_has_public_header_and_no_signing_key_material() {
    let mut w = World::new();
    w.logon();
    w.put(DATA, &[0xcc; 520]);
    assert_eq!(w.call("XOnlineSaveLogonState", &[DATA]), 0);
    assert_eq!(w.bytes(DATA, 4), vec![0x4c, 1, 4, 2]);
    assert_eq!(w.bytes(DATA + 4, 448), w.bytes(USERS, 448));
    assert_eq!(w.bytes(DATA + 516, 4), vec![0xcc; 4]);
}

fn wide_bytes(value: &str) -> Vec<u8> {
    value
        .encode_utf16()
        .chain([0])
        .flat_map(u16::to_le_bytes)
        .collect()
}
fn storage_path(w: &mut World, facility: u32, name: &str, capacity: u32, output: u32) -> u32 {
    w.put(USERS, &wide_bytes(name));
    w.word(OUTPUT, capacity);
    w.call(
        "XOnlineStorageCreateServerPath",
        &[
            facility,
            ID as u32,
            (ID >> 32) as u32,
            0x87654321,
            0x01234567,
            USERS,
            output,
            OUTPUT,
        ],
    )
}
#[test]
fn storage_paths_format_all_three_facilities_and_split_64_bit_ids() {
    let mut w = World::new();
    w.startup();
    for (facility, prefix) in [
        (
            3,
            "//tteam.4d530064/u:0123456787654321/4d530064/00091234567890ab/",
        ),
        (4, "//title.4d530064/t:4d530064/"),
        (5, "//tuser.4d530064/u:00091234567890ab/4d530064/"),
    ] {
        let expected = wide_bytes(&format!("{prefix}settings.bin"));
        w.put(DATA, &vec![0xcc; expected.len() + 4]);
        assert_eq!(
            storage_path(
                &mut w,
                facility,
                "settings.bin",
                expected.len() as u32 / 2,
                DATA
            ),
            0
        );
        assert_eq!(w.u32(OUTPUT), expected.len() as u32 / 2);
        assert_eq!(w.bytes(DATA, expected.len()), expected);
        assert_eq!(w.bytes(DATA + expected.len() as u32, 4), vec![0xcc; 4]);
    }
}
#[test]
fn storage_path_size_queries_and_prefix_only_overflow_match_retail() {
    let mut w = World::new();
    w.startup();
    let prefix = "//tuser.4d530064/u:00091234567890ab/4d530064/";
    let required = prefix.len() as u32 + "settings.bin".len() as u32 + 1;
    assert_eq!(
        storage_path(&mut w, 5, "settings.bin", 0, 0),
        INSUFFICIENT_BUFFER
    );
    assert_eq!(w.u32(OUTPUT), required);
    assert_eq!(storage_path(&mut w, 5, "settings.bin", required, 0), 0);
    w.put(DATA, &[0xcc; 256]);
    assert_eq!(
        storage_path(&mut w, 5, "settings.bin", prefix.len() as u32, DATA),
        INSUFFICIENT_BUFFER
    );
    assert_eq!(w.bytes(DATA, 256), vec![0xcc; 256]);
    assert_eq!(w.u32(OUTPUT), required);
    assert_eq!(
        storage_path(&mut w, 5, "settings.bin", prefix.len() as u32 + 1, DATA),
        INSUFFICIENT_BUFFER
    );
    assert_eq!(w.bytes(DATA, (prefix.len() + 1) * 2), wide_bytes(prefix));
    assert_eq!(
        w.bytes(DATA + (prefix.len() as u32 + 1) * 2, 4),
        vec![0xcc; 4]
    );
    assert_eq!(w.u32(OUTPUT), required);
}
#[test]
fn storage_path_preserves_unicode_and_does_not_impose_a_256_character_format_cap() {
    let mut w = World::new();
    w.startup();
    let name = format!("{}雪🙂", "x".repeat(280));
    let expected = wide_bytes(&format!("//title.4d530064/t:4d530064/{name}"));
    assert_eq!(storage_path(&mut w, 4, &name, 1024, DATA), 0);
    assert_eq!(w.bytes(DATA, expected.len()), expected);
    assert_eq!(w.u32(OUTPUT), expected.len() as u32 / 2);
}
#[test]
fn storage_path_invalid_facilities_and_uninitialized_calls_leave_outputs_untouched() {
    let mut w = World::new();
    w.word(OUTPUT, 256);
    assert_eq!(
        w.call(
            "XOnlineStorageCreateServerPath",
            &[5, 0, 0, 0, 0, 0, 0, OUTPUT]
        ),
        NOT_INITIALIZED
    );
    w.startup();
    for facility in [0, 1, 2, 6, u32::MAX] {
        assert_eq!(
            w.call(
                "XOnlineStorageCreateServerPath",
                &[facility, 0, 0, 0, 0, 0, 0, OUTPUT]
            ),
            STORAGE_INVALID_FACILITY
        );
        assert_eq!(w.u32(OUTPUT), 256);
    }
}
#[test]
fn missing_storage_download_tasks_complete_without_touching_the_receive_buffer_or_results() {
    let mut w = World::new();
    w.logon();
    assert_eq!(storage_path(&mut w, 5, "settings.bin", 256, DATA), 0);
    w.put(USERS, &[0xcc; 64]);
    assert_eq!(
        w.call(
            "XOnlineStorageDownloadToMemory",
            &[5, 0, DATA, USERS, 64, 0, 0, OUTPUT]
        ),
        0
    );
    let h = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[h]), STORAGE_FILE_NOT_FOUND);
    assert_eq!(w.bytes(USERS, 64), vec![0xcc; 64]);
    w.put(OUTPUT, &[0xdd; 32]);
    assert_eq!(
        w.call(
            "XOnlineStorageDownloadToMemoryGetResults",
            &[h, OUTPUT, OUTPUT + 4, OUTPUT + 8, OUTPUT + 12, OUTPUT + 20]
        ),
        STORAGE_FILE_NOT_FOUND
    );
    assert_eq!(w.bytes(OUTPUT, 32), vec![0xdd; 32]);
    assert_eq!(
        w.call(
            "XOnlineStorageGetProgress",
            &[h, OUTPUT, OUTPUT + 4, OUTPUT + 12]
        ),
        STORAGE_FILE_NOT_FOUND
    );
    assert_eq!(w.bytes(OUTPUT, 20), vec![0; 20]);
    assert_eq!(w.bytes(OUTPUT + 20, 4), vec![0xdd; 4]);
    assert_eq!(w.call("XOnlineTaskClose", &[h]), 0);
    assert_eq!(
        w.call(
            "XOnlineStorageDownloadToMemoryGetResults",
            &[h, 0, 0, 0, 0, 0]
        ),
        INVALID_ARGUMENT
    );
    assert_eq!(
        w.call("XOnlineStorageGetProgress", &[h, 0, 0, 0]),
        INVALID_ARGUMENT
    );
}
#[test]
fn storage_package_reads_and_both_upload_forms_fail_cleanly_in_tasks() {
    let mut w = World::new();
    let logon = w.logon();
    assert_eq!(storage_path(&mut w, 5, "settings.bin", 256, DATA), 0);
    let cases = [
        (
            "XOnlineStorageDownload",
            vec![5, 0, DATA, 0, 0, 0, OUTPUT],
            STORAGE_FILE_NOT_FOUND,
        ),
        (
            "XOnlineStorageUploadByServerPath",
            vec![5, 0, DATA, 0, 0, 0, 0, 0, OUTPUT],
            STORAGE_ACCESS_DENIED,
        ),
        (
            "XOnlineStorageUploadFromMemory",
            vec![5, 0, DATA, 0, 0, USERS, 16, 0, 0, OUTPUT],
            STORAGE_ACCESS_DENIED,
        ),
    ];
    for (name, args, status) in cases {
        assert_eq!(w.call(name, &args), 0);
        let task = w.u32(OUTPUT);
        assert_eq!(w.call("XOnlineTaskContinue", &[task]), status);
        assert_eq!(
            w.call("XOnlineStorageGetProgress", &[task, 0, 0, 0]),
            status
        );
        assert_eq!(
            w.call(
                "XOnlineStorageDownloadToMemoryGetResults",
                &[task, 0, 0, 0, 0, 0]
            ),
            INVALID_ARGUMENT
        );
        assert_eq!(w.call("XOnlineTaskClose", &[task]), 0);
    }
    assert_eq!(w.call("XOnlineTaskContinue", &[logon]), CONNECTED);
    assert_eq!(
        w.call(
            "XOnlineStorageDownloadToMemory",
            &[5, 0, DATA, USERS, 16, 0, 0, OUTPUT]
        ),
        0
    );
    let task = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineCleanup", &[]), 0);
    w.startup();
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), INVALID_ARGUMENT);
}

const STAT_SPECS: u32 = DATA + 0x4000;
const STAT_VALUES: u32 = DATA + 0x5000;
const STAT_NAME: u32 = DATA + 0x5800;
const STAT_EXTRA: u32 = DATA + 0x6000;
fn put_stat_spec(w: &World, p: u32, xuid: u64, board: u32, count: u32, values: u32) {
    w.put(p, &[0; 24]);
    w.put(p, &xuid.to_le_bytes());
    w.word(p + 12, board);
    w.word(p + 16, count);
    w.word(p + 20, values);
}
fn put_stat(w: &World, p: u32, id: u16, kind: u32, bits: u64) {
    w.put(p, &[0xcc; 16]);
    w.put(p, &id.to_le_bytes());
    w.word(p + 4, kind);
    w.put(p + 8, &bits.to_le_bytes());
}
fn stat_world() -> (World, Arc<Hle>) {
    let h = hle(config());
    let mut w = World::with(h.clone());
    w.logon();
    (w, h)
}
#[test]
fn stats_write_parses_all_xdk_values_and_owns_the_accepted_copy() {
    use live_hle::{Stat, StatValue};
    let (mut w, h) = stat_world();
    put_stat_spec(&w, STAT_SPECS, ID, 0x1234, 5, STAT_VALUES);
    w.word(STAT_SPECS + 8, 0x12345678);
    w.put(STAT_NAME, &wide_bytes("Player 雪"));
    for (i, (id, kind, bits)) in [
        (10, 0, u64::MAX),
        (11, 1, 0x12345678ffffffefu64),
        (12, 2, (-9876543210123i64) as u64),
        (13, 3, (-2.5f64).to_bits()),
        (0xfffd, 4, 0xdeadbeef00000000u64 | u64::from(STAT_NAME)),
    ]
    .into_iter()
    .enumerate()
    {
        put_stat(&w, STAT_VALUES + i as u32 * 16, id, kind, bits);
    }
    let input = w.bytes(STAT_VALUES, 80);
    assert_eq!(w.call("XOnlineStatWrite", &[1, STAT_SPECS, 0, OUTPUT]), 0);
    let task = w.u32(OUTPUT);
    assert_eq!(w.bytes(STAT_VALUES, 80), input);
    w.put(STAT_VALUES, &[0xdd; 80]);
    w.put(STAT_NAME, &wide_bytes("Changed"));
    for _ in 0..3 {
        assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
    }
    let captures = h.stat_writes();
    assert_eq!(captures.len(), 1);
    let captured = &captures[0];
    assert_eq!(captured.sequence, 1);
    assert!(captured.captured_at_unix_ms > 0);
    let spec = &captured.specs[0];
    assert_eq!(
        (spec.xuid, spec.user_flags, spec.leaderboard),
        (ID, 0x12345678, 0x1234)
    );
    assert_eq!(
        spec.stats,
        vec![
            Stat {
                id: 10,
                value: StatValue::None
            },
            Stat {
                id: 11,
                value: StatValue::Long(-17)
            },
            Stat {
                id: 12,
                value: StatValue::LongLong(-9876543210123)
            },
            Stat {
                id: 13,
                value: StatValue::Double(-2.5)
            },
            Stat {
                id: 0xfffd,
                value: StatValue::String("Player 雪".into())
            },
        ]
    );
    assert_eq!(w.call("XOnlineTaskClose", &[task]), 0);
    assert_eq!(h.stat_writes(), captures);
}
#[test]
fn stats_upload_results_succeed_without_attachments_and_preserve_acknowledged_inputs() {
    let (mut w, h) = stat_world();
    put_stat_spec(&w, STAT_SPECS, ID, 7, 4, STAT_VALUES);
    w.put(STAT_NAME, &wide_bytes("Synthetic"));
    put_stat(&w, STAT_VALUES, 0xfffd, 4, u64::from(STAT_NAME));
    for i in 1..4 {
        put_stat(&w, STAT_VALUES + i * 16, i as u16, 1, u64::from(i));
    }
    let before = w.bytes(STAT_VALUES, 64);
    assert_eq!(w.call("XOnlineStatWrite", &[1, STAT_SPECS, 0, OUTPUT]), 0);
    let task = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
    w.put(OUTPUT, &[0xaa; 16]);
    assert_eq!(
        w.call(
            "XOnlineStatWriteGetResult",
            &[task, OUTPUT, OUTPUT + 4, OUTPUT + 8]
        ),
        0
    );
    assert_eq!(w.bytes(OUTPUT, 12), vec![0; 12]);
    assert_eq!(w.bytes(OUTPUT + 12, 4), vec![0xaa; 4]);
    assert_eq!(w.bytes(STAT_VALUES, 64), before);
    assert_eq!(w.call("XOnlineStatWriteGetResult", &[task, 0, 0, 0]), 0);
    assert_eq!(h.stat_writes().len(), 1);
    assert_eq!(w.call("XOnlineTaskClose", &[task]), 0);
    assert_eq!(
        w.call("XOnlineStatWriteGetResult", &[task, 0, 0, 0]),
        INVALID_ARGUMENT
    );
}
#[test]
fn stats_write_rejects_bad_values_and_limits_atomically() {
    let (mut w, h) = stat_world();
    for case in 0..7 {
        put_stat_spec(&w, STAT_SPECS, ID, 7, 1, STAT_VALUES);
        put_stat(&w, STAT_VALUES, 1, 1, 12);
        w.word(OUTPUT, 0xdeadbeef);
        let mut count = 1;
        let mut ptr = STAT_SPECS;
        let expected = match case {
            0 => {
                count = 21;
                ptr = 0;
                STAT_TOO_MANY_SPECS
            }
            1 => {
                w.word(STAT_SPECS + 16, 65);
                STAT_TOO_MANY_STATS
            }
            2 => {
                w.put(STAT_SPECS, &[0; 8]);
                STAT_PERMISSION_DENIED
            }
            3 => {
                w.word(STAT_VALUES + 4, 5);
                STAT_BAD_REQUEST
            }
            4 => {
                put_stat(&w, STAT_VALUES, 1, 3, f64::NAN.to_bits());
                STAT_BAD_REQUEST
            }
            5 => {
                w.put(STAT_NAME, &wide_bytes(&"x".repeat(33)));
                put_stat(&w, STAT_VALUES, 0xfffd, 4, u64::from(STAT_NAME));
                STAT_BAD_REQUEST
            }
            _ => {
                w.put(STAT_NAME, &[0x00, 0xd8, 0, 0]);
                put_stat(&w, STAT_VALUES, 0xfffd, 4, u64::from(STAT_NAME));
                STAT_BAD_REQUEST
            }
        };
        assert_eq!(
            w.call("XOnlineStatWrite", &[count, ptr, 0, OUTPUT]),
            expected,
            "case {case}"
        );
        assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
        assert!(h.stat_writes().is_empty());
    }
    // A valid first spec must not be committed if a later spec is malformed.
    put_stat_spec(&w, STAT_SPECS, ID, 7, 1, STAT_VALUES);
    put_stat(&w, STAT_VALUES, 1, 1, 9);
    put_stat_spec(&w, STAT_SPECS + 24, ID, 8, 1, STAT_VALUES + 16);
    put_stat(&w, STAT_VALUES + 16, 2, 99, 0);
    assert_eq!(
        w.call("XOnlineStatWrite", &[2, STAT_SPECS, 0, OUTPUT]),
        STAT_BAD_REQUEST
    );
    assert!(h.stat_writes().is_empty());
}
#[test]
fn stats_nested_pointer_faults_do_not_accept_a_partial_write_or_publish_a_handle() {
    let (mut w, h) = stat_world();
    for pointer in [0xfffffff0, DATA + 0xfff8] {
        put_stat_spec(&w, STAT_SPECS, ID, 7, 1, pointer);
        w.word(OUTPUT, 0xdeadbeef);
        let t = w.raw("XOnlineStatWrite", &[1, STAT_SPECS, 0, OUTPUT]);
        assert!(matches!(
            w.ctx.event(t.event_id),
            Some(Event::MemoryFault { .. })
        ));
        assert!(h.stat_writes().is_empty());
        assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
    }
    put_stat_spec(&w, STAT_SPECS, ID, 7, 1, STAT_VALUES);
    put_stat(&w, STAT_VALUES, 0xfffd, 4, 0xfffffff0);
    let t = w.raw("XOnlineStatWrite", &[1, STAT_SPECS, 0, OUTPUT]);
    assert!(matches!(
        w.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert!(h.stat_writes().is_empty());
}
#[test]
fn stats_reads_return_none_and_zero_in_the_callers_existing_stat_arrays() {
    let (mut w, h) = stat_world();
    put_stat_spec(&w, STAT_SPECS, ID, 7, 3, STAT_VALUES);
    for i in 0..3 {
        put_stat(
            &w,
            STAT_VALUES + i * 16,
            (11 + i) as u16,
            u32::MAX,
            u64::MAX,
        );
    }
    let headers = w.bytes(STAT_SPECS, 24);
    w.put(STAT_EXTRA, &[0xdd; 32]);
    assert_eq!(w.call("XOnlineStatRead", &[1, STAT_SPECS, 0, OUTPUT]), 0);
    let task = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
    assert_eq!(
        w.call(
            "XOnlineStatReadGetResult",
            &[task, 1, STAT_SPECS, 32, STAT_EXTRA]
        ),
        0
    );
    assert_eq!(w.bytes(STAT_SPECS, 24), headers);
    for i in 0..3 {
        assert_eq!(
            w.bytes(STAT_VALUES + i * 16, 2),
            (11 + i as u16).to_le_bytes()
        );
        assert_eq!(w.bytes(STAT_VALUES + i * 16 + 2, 2), vec![0xcc; 2]);
        assert_eq!(w.bytes(STAT_VALUES + i * 16 + 4, 12), vec![0; 12]);
    }
    assert_eq!(w.bytes(STAT_EXTRA, 32), vec![0xdd; 32]);
    assert!(h.stat_writes().is_empty());
    assert_eq!(
        w.call("XOnlineStatReadGetResult", &[task, 1, STAT_SPECS, 0, 0]),
        0
    );
    assert_eq!(
        w.call("XOnlineStatWriteGetResult", &[task, 0, 0, 0]),
        INVALID_ARGUMENT
    );
    assert_eq!(w.call("XOnlineTaskClose", &[task]), 0);
    assert_eq!(
        w.call("XOnlineStatReadGetResult", &[task, 1, STAT_SPECS, 0, 0]),
        INVALID_ARGUMENT
    );
}
#[test]
fn stats_read_results_validate_all_specs_before_writing_and_own_request_ids() {
    let (mut w, _) = stat_world();
    for i in 0..2 {
        put_stat_spec(
            &w,
            STAT_SPECS + i * 24,
            ID + u64::from(i),
            7,
            1,
            STAT_VALUES + i * 16,
        );
        put_stat(&w, STAT_VALUES + i * 16, (11 + i) as u16, 1, 99);
    }
    assert_eq!(w.call("XOnlineStatRead", &[2, STAT_SPECS, 0, OUTPUT]), 0);
    let task = w.u32(OUTPUT);
    // Relocate output arrays, then destroy the source: the task owns its query.
    let out_specs = STAT_SPECS + 0x200;
    let out_values = STAT_VALUES + 0x200;
    for i in 0..2 {
        put_stat_spec(
            &w,
            out_specs + i * 24,
            ID + u64::from(i),
            7,
            1,
            out_values + i * 16,
        );
        put_stat(&w, out_values + i * 16, (11 + i) as u16, 1, 77);
    }
    w.put(STAT_SPECS, &[0xbb; 48]);
    w.put(STAT_VALUES, &[0xbb; 32]);
    w.word(out_specs + 24 + 12, 8);
    let before = w.bytes(out_values, 32);
    assert_eq!(
        w.call("XOnlineStatReadGetResult", &[task, 2, out_specs, 0, 0]),
        RESULTS_UNAVAILABLE
    );
    assert_eq!(w.bytes(out_values, 32), before);
    w.word(out_specs + 24 + 12, 7);
    assert_eq!(
        w.call("XOnlineStatReadGetResult", &[task, 2, out_specs, 0, 0]),
        0
    );
    assert_eq!(w.bytes(out_values + 4, 12), vec![0; 12]);
    assert_eq!(w.bytes(out_values + 20, 12), vec![0; 12]);
}
#[test]
fn stats_reads_accept_the_managers_32_spec_batch_and_empty_queries() {
    let (mut w, _) = stat_world();
    for i in 0..32 {
        put_stat_spec(
            &w,
            STAT_SPECS + i * 24,
            ID,
            100 + i,
            1,
            STAT_VALUES + i * 16,
        );
        put_stat(&w, STAT_VALUES + i * 16, 1, 0, u64::MAX);
    }
    assert_eq!(w.call("XOnlineStatRead", &[32, STAT_SPECS, 0, OUTPUT]), 0);
    let task = w.u32(OUTPUT);
    assert_eq!(
        w.call("XOnlineStatReadGetResult", &[task, 32, STAT_SPECS, 0, 0]),
        0
    );
    assert_eq!(w.call("XOnlineStatRead", &[0, 0, 0, OUTPUT]), 0);
    let empty = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[empty]), TASK_SUCCESS);
    assert_eq!(w.call("XOnlineStatReadGetResult", &[empty, 0, 0, 0, 0]), 0);
}
#[test]
fn stats_captures_survive_cleanup_and_draining_does_not_reuse_sequences() {
    let (mut w, h) = stat_world();
    put_stat_spec(&w, STAT_SPECS, ID, 7, 1, STAT_VALUES);
    put_stat(&w, STAT_VALUES, 1, 1, 42);
    assert_eq!(w.call("XOnlineStatWrite", &[1, STAT_SPECS, 0, OUTPUT]), 0);
    let task = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineCleanup", &[]), 0);
    assert_eq!(h.stat_writes().len(), 1);
    w.startup();
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), INVALID_ARGUMENT);
    let first = h.drain_stat_writes();
    assert_eq!(first[0].sequence, 1);
    assert!(h.stat_writes().is_empty());
    w.logon();
    assert_eq!(w.call("XOnlineStatWrite", &[1, STAT_SPECS, 0, OUTPUT]), 0);
    assert_eq!(h.stat_writes()[0].sequence, 2);
}

#[test]
fn stats_journal_exhaustion_preserves_accepted_batches_and_draining_allows_writes() {
    let (mut w, h) = stat_world();
    put_stat_spec(&w, STAT_SPECS, ID, 7, 1, STAT_VALUES);
    put_stat(&w, STAT_VALUES, 1, 1, 42);
    for _ in 0..256 {
        assert_eq!(w.call("XOnlineStatWrite", &[1, STAT_SPECS, 0, OUTPUT]), 0);
        let task = w.u32(OUTPUT);
        assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
        assert_eq!(w.call("XOnlineTaskClose", &[task]), 0);
    }
    w.word(OUTPUT, 0xdeadbeef);
    assert_eq!(
        w.call("XOnlineStatWrite", &[1, STAT_SPECS, 0, OUTPUT]),
        OUT_OF_MEMORY
    );
    assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
    let captured = h.drain_stat_writes();
    assert_eq!(captured.len(), 256);
    assert_eq!(captured.first().unwrap().sequence, 1);
    assert_eq!(captured.last().unwrap().sequence, 256);
    assert_eq!(w.call("XOnlineStatWrite", &[1, STAT_SPECS, 0, OUTPUT]), 0);
    assert_eq!(h.stat_writes()[0].sequence, 257);
}

fn outbox_world() -> (World, Arc<Hle>) {
    let h = hle(config());
    let mut w = World::with(Arc::clone(&h));
    w.logon();
    (w, h)
}
fn message_create(w: &mut World, ty: u32, context: u64, flags: u32) -> u32 {
    assert_eq!(
        w.call(
            "XOnlineMessageCreate",
            &[
                ty,
                0,
                0,
                context as u32,
                (context >> 32) as u32,
                flags,
                0x4ec0,
                OUTPUT
            ]
        ),
        0
    );
    w.u32(OUTPUT)
}
fn message_set(w: &mut World, h: u32, tag: u32, value: &[u8]) {
    w.put(DATA + 0x4000, value);
    assert_eq!(
        w.call(
            "XOnlineMessageSetProperty",
            &[h, tag, value.len() as u32, DATA + 0x4000, 0]
        ),
        0
    );
}
#[test]
fn outbox_friend_by_name_composer_owns_text_voice_and_successful_send() {
    use live_hle::{MessageRecipient, MessageSendKind, MessageValue};
    let (mut w, h) = outbox_world();
    let msg = message_create(&mut w, 2, 0, 0x0c);
    message_set(&mut w, msg, 0x6c4, &wide_bytes("Hello 雪"));
    message_set(&mut w, msg, 0x4c5, &3u32.to_le_bytes());
    message_set(&mut w, msg, 0x9c1, &[9, 4, 2, 1]);
    message_set(&mut w, msg, 0x3c2, &1u16.to_le_bytes());
    message_set(&mut w, msg, 0x4c3, &1000u32.to_le_bytes());
    w.put(DATA + 0x5000, b"Other Player\0");
    assert_eq!(
        w.call(
            "XOnlineFriendsRequestByNameEx",
            &[0, DATA + 0x5000, msg, 0, OUTPUT]
        ),
        0
    );
    let task = w.u32(OUTPUT);
    // The title may dispose the builder while its game task is still pending.
    assert_eq!(w.call("XOnlineMessageDestroy", &[msg]), 0);
    w.put(DATA + 0x4000, &[0xdd; 32]);
    w.put(DATA + 0x5000, &[0xee; 16]);
    for _ in 0..3 {
        assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
    }
    let sends = h.message_sends();
    assert_eq!(sends.len(), 1);
    assert_eq!(sends[0].sender_xuid, ID);
    assert_eq!(sends[0].kind, MessageSendKind::FriendRequest);
    assert_eq!(
        sends[0].recipients,
        vec![MessageRecipient::Gamertag("Other Player".into())]
    );
    assert_eq!(
        sends[0].message.properties[&0x6c4].value,
        MessageValue::String("Hello 雪".into())
    );
    assert_eq!(
        sends[0].message.properties[&0x9c1].value,
        MessageValue::Attachment(vec![9, 4, 2, 1])
    );
    assert_eq!(
        sends[0].message.properties[&0x3c2].value,
        MessageValue::I2(1)
    );
    assert_eq!(
        sends[0].message.properties[&0x4c3].value,
        MessageValue::I4(1000)
    );
    assert_eq!(sends[0].message.expire_minutes, 0x4ec0);
    assert_eq!(w.call("XOnlineTaskClose", &[task]), 0);
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), INVALID_ARGUMENT);
    assert_eq!(h.message_sends(), sends);
}
#[test]
fn outbox_generic_send_parses_all_property_types_and_twelve_byte_recipients() {
    use live_hle::{MessageRecipient, MessageUser, MessageValue};
    let (mut w, h) = outbox_world();
    let msg = message_create(&mut w, 1, 0x9988776655443322, 0x01000008);
    for (tag, bytes) in [
        (0x101, vec![]),
        (0x202, vec![0xfe]),
        (0x303, 0xfffdu16.to_le_bytes().to_vec()),
        (0x404, u32::MAX.to_le_bytes().to_vec()),
        (0x505, u64::MAX.to_le_bytes().to_vec()),
        (0x606, wide_bytes("Text")),
        (0x707, 1234567u64.to_le_bytes().to_vec()),
        (0x808, vec![3, 1, 4]),
        (0x909, vec![7, 8]),
        (0xa0a, 7u32.to_le_bytes().to_vec()),
        (0xb0b, 0x1234u32.to_le_bytes().to_vec()),
    ] {
        message_set(&mut w, msg, tag, &bytes);
    }
    let peers = DATA + 0x5000;
    for i in 0..2 {
        w.put(peers + i * 12, &(ID + 1 + u64::from(i)).to_le_bytes());
        w.word(peers + i * 12 + 8, 0x100 + i);
    }
    assert_eq!(
        w.call("XOnlineMessageSend", &[0, msg, 2, peers, 0, OUTPUT]),
        0
    );
    let task = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
    let capture = h.message_sends().remove(0);
    assert_eq!(capture.message.context, 0x9988776655443322);
    assert_eq!(capture.message.flags, 0x01000008);
    assert_eq!(
        capture.recipients[1],
        MessageRecipient::User(MessageUser {
            xuid: ID + 2,
            flags: 0x101
        })
    );
    let values: Vec<_> = capture
        .message
        .properties
        .values()
        .map(|p| p.value.clone())
        .collect();
    assert_eq!(
        values,
        vec![
            MessageValue::Null,
            MessageValue::I1(0xfe),
            MessageValue::I2(0xfffd),
            MessageValue::I4(u32::MAX),
            MessageValue::I8(u64::MAX),
            MessageValue::String("Text".into()),
            MessageValue::FileTime(1234567),
            MessageValue::Binary(vec![3, 1, 4]),
            MessageValue::Attachment(vec![7, 8]),
            MessageValue::Bool(true),
            MessageValue::StringId(0x1234)
        ]
    );
    message_set(&mut w, msg, 0x606, &wide_bytes("Changed"));
    w.put(peers, &[0; 24]);
    assert_eq!(h.message_sends()[0], capture);
    assert_eq!(w.call("XOnlineMessageDestroy", &[msg]), 0);
    assert_eq!(w.call("XOnlineMessageDestroy", &[msg]), INVALID_ARGUMENT);
    assert_eq!(
        w.call("XOnlineMessageSetProperty", &[msg, 0x101, 0, 0, 0]),
        INVALID_ARGUMENT
    );
}
#[test]
fn outbox_friend_xuid_and_game_invite_use_their_complete_stdcall_frames() {
    use live_hle::{MessageRecipient, MessageSendKind, MessageUser};
    let (mut w, h) = outbox_world();
    let friend = message_create(&mut w, 2, 0, 0);
    assert_eq!(
        w.call(
            "XOnlineFriendsRequestEx",
            &[0, 0x12345678, 0x0009abcd, 0x200, friend, 0, OUTPUT]
        ),
        0
    );
    let friend_task = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[friend_task]), TASK_SUCCESS);
    let invite = message_create(&mut w, 3, 0, 0);
    w.put(DATA + 0x5000, &(ID + 1).to_le_bytes());
    w.word(DATA + 0x5008, 0);
    message_set(&mut w, invite, 0x581, &0x1122334455667788u64.to_le_bytes());
    assert_eq!(
        w.call(
            "XOnlineGameInviteSend",
            &[
                0,
                1,
                DATA + 0x5000,
                0x55667788,
                0x11223344,
                0,
                invite,
                0,
                OUTPUT
            ]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TASK_SUCCESS
    );
    let sends = h.message_sends();
    assert_eq!(
        sends[0].recipients,
        vec![MessageRecipient::User(MessageUser {
            xuid: 0x0009abcd12345678,
            flags: 0x200
        })]
    );
    assert_eq!(
        sends[1].kind,
        MessageSendKind::GameInvite {
            session_id: 0x1122334455667788,
            flags: 0
        }
    );
    assert_eq!(w.call("XOnlineTaskClose", &[friend_task]), 0);
}
#[test]
fn outbox_clan_recruitment_by_name_and_xuid_own_packed_member_properties() {
    use live_hle::{MessageRecipient, MessageSendKind, MessageUser, RecruitProperties};
    let (mut w, h) = outbox_world();
    let team = team_create_local(&mut w, &h, "Test Clan");
    let [lo, hi] = team_words(team);
    let msg = message_create(&mut w, 4, team.0, 0x40);
    message_set(&mut w, msg, 0x681, &wide_bytes("Test Clan"));
    let props = DATA + 0x6000;
    w.put(props, &[0; 106]);
    w.word(props, 0x10);
    w.put(props + 4, &3u16.to_le_bytes());
    w.put(props + 6, &[1, 2, 3]);
    w.put(DATA + 0x5000, b"Second\0");
    assert_eq!(
        w.call(
            "XOnlineTeamMemberRecruitByName",
            &[0, lo, hi, 0x22, DATA + 0x5000, props, msg, 0, OUTPUT]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TASK_SUCCESS
    );
    h.local_team_action(
        0x0009000011112222,
        &live_proto::team::Action::Answer {
            id: team,
            accept: false,
        },
    )
    .unwrap();
    assert_eq!(
        w.call(
            "XOnlineTeamMemberRecruit",
            &[
                0, lo, hi, 0x22, 0x11112222, 0x00090000, 0, props, msg, 0, OUTPUT
            ]
        ),
        0
    );
    w.put(props, &[0xff; 106]);
    let sends = h.message_sends();
    assert_eq!(
        sends[0].recipients,
        vec![MessageRecipient::Gamertag("Second".into())]
    );
    assert_eq!(
        sends[0].kind,
        MessageSendKind::TeamRecruit {
            team: MessageUser {
                xuid: team.0,
                flags: 0x22
            },
            properties: RecruitProperties {
                privileges: 0x10,
                data: vec![1, 2, 3]
            }
        }
    );
    assert_eq!(sends[1].kind, sends[0].kind);
}
#[test]
fn outbox_rejects_invalid_sizes_flags_and_strings_without_corrupting_a_builder() {
    use live_hle::MessageValue;
    let (mut w, h) = outbox_world();
    w.word(OUTPUT, 0xdeadbeef);
    assert_eq!(
        w.call("XOnlineMessageCreate", &[4, 0, 0, 0, 0, 0, 0, OUTPUT]),
        INVALID_ARGUMENT
    );
    assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
    let msg = message_create(&mut w, 1, 0, 0);
    message_set(&mut w, msg, 0x6c4, &wide_bytes("Original"));
    w.put(DATA + 0x4000, &[0; 64]);
    for (tag, n, flags, expected) in [
        (0xc80, 4, 0, INVALID_ARGUMENT),
        (0x480, 3, 0, INVALID_ARGUMENT),
        (0x680, 3, 0, INVALID_ARGUMENT),
        (0x680, 0, 0, INVALID_ARGUMENT),
        (0x980, 4, 2, INVALID_ARGUMENT),
        (0x480, 4, 1, INVALID_ARGUMENT),
        (0x880, 4096, 0, MESSAGE_MAX_DETAILS),
    ] {
        assert_eq!(
            w.call(
                "XOnlineMessageSetProperty",
                &[msg, tag, n, DATA + 0x4000, flags]
            ),
            expected
        );
    }
    w.put(DATA + 0x4000, &[0, 0xd8, 0, 0]);
    assert_eq!(
        w.call(
            "XOnlineMessageSetProperty",
            &[msg, 0x6c4, 4, DATA + 0x4000, 0]
        ),
        INVALID_ARGUMENT
    );
    w.put(DATA + 0x5000, &(ID + 1).to_le_bytes());
    w.word(DATA + 0x5008, 0);
    assert_eq!(
        w.call("XOnlineMessageSend", &[0, msg, 0, 0, 0, OUTPUT]),
        INVALID_ARGUMENT
    );
    assert_eq!(
        w.call("XOnlineMessageSend", &[0, msg, 101, 0, 0, OUTPUT]),
        INVALID_ARGUMENT
    );
    assert!(h.message_sends().is_empty());
    assert_eq!(
        w.call("XOnlineMessageSend", &[0, msg, 1, DATA + 0x5000, 0, OUTPUT]),
        0
    );
    assert_eq!(h.message_sends()[0].message.properties.len(), 1);
    assert_eq!(
        h.message_sends()[0].message.properties[&0x6c4].value,
        MessageValue::String("Original".into())
    );
}
#[test]
fn outbox_pointer_faults_do_not_publish_tasks_or_partial_properties() {
    let (mut w, h) = outbox_world();
    let msg = message_create(&mut w, 1, 0, 0);
    for (name, args) in [
        (
            "XOnlineMessageCreate",
            vec![1, 0, 0, 0, 0, 0, 0, DATA + 0xfffe],
        ),
        (
            "XOnlineMessageSetProperty",
            vec![msg, 0x680, 8, DATA + 0xfffc, 0],
        ),
        (
            "XOnlineMessageSend",
            vec![0, msg, 1, DATA + 0xfffc, 0, OUTPUT],
        ),
        (
            "XOnlineMessageSend",
            vec![0, msg, 1, DATA + 0x5000, 0, DATA + 0xfffe],
        ),
    ] {
        w.word(OUTPUT, 0xdeadbeef);
        let t = w.raw(name, &args);
        assert!(
            matches!(w.ctx.event(t.event_id), Some(Event::MemoryFault { .. })),
            "{name}"
        );
        assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
        assert!(h.message_sends().is_empty());
    }
}
#[test]
fn clanker_social_sends_refuse_the_whole_outbox_message() {
    let (mut w, h) = outbox_world();
    let msg = message_create(&mut w, 1, 0, 0);
    let voice = message_create(&mut w, 1, 0, 0x0c);
    message_set(&mut w, voice, 0x9c1, &[9, 4, 2, 1]);
    let to = DATA + 0x5000;
    for mark in [
        live_proto::bots::SERVER_XUID_MARK,
        live_proto::bots::LOCAL_XUID_MARK,
    ] {
        let id = mark | 1;
        w.put(to, &1u64.to_le_bytes());
        w.put(to + 12, &id.to_le_bytes());
        for (op, args) in [
            ("XOnlineMessageSend", vec![0, msg, 2, to, 0, OUTPUT]),
            ("XOnlineMessageSend", vec![0, voice, 2, to, 0, OUTPUT]),
            (
                "XOnlineGameInviteSend",
                vec![0, 2, to, 1, 0, 0, 0, 0, OUTPUT],
            ),
            (
                "XOnlineFriendsRequestEx",
                vec![0, 1, (id >> 32) as u32, 0, 0, 0, OUTPUT],
            ),
        ] {
            w.word(OUTPUT, 0xcafe);
            assert_eq!(w.call(op, &args), INVALID_ARGUMENT, "{op}");
            assert_eq!(w.u32(OUTPUT), 0xcafe);
            assert!(h.message_sends().is_empty());
        }
    }
}

#[test]
fn outbox_cleanup_invalidates_builders_and_tasks_but_keeps_captures() {
    let (mut w, h) = outbox_world();
    let msg = message_create(&mut w, 2, 0, 0);
    w.put(DATA + 0x5000, b"Target\0");
    assert_eq!(
        w.call(
            "XOnlineFriendsRequestByNameEx",
            &[0, DATA + 0x5000, msg, 0, OUTPUT]
        ),
        0
    );
    let task = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineCleanup", &[]), 0);
    w.startup();
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), INVALID_ARGUMENT);
    assert_eq!(w.call("XOnlineMessageDestroy", &[msg]), INVALID_ARGUMENT);
    assert_eq!(h.message_sends().len(), 1);
    assert_eq!(h.drain_message_sends()[0].sequence, 1);
    assert!(h.message_sends().is_empty());
    w.logon();
    assert_eq!(
        w.call(
            "XOnlineFriendsRequestByNameEx",
            &[0, DATA + 0x5000, 0, 0, OUTPUT]
        ),
        0
    );
    assert_eq!(h.message_sends()[0].sequence, 2);
}
#[test]
fn outbox_builder_and_capture_limits_fail_without_evicting_accepted_messages() {
    let (mut w, h) = outbox_world();
    let mut builders = Vec::new();
    for _ in 0..64 {
        builders.push(message_create(&mut w, 2, 0, 0));
    }
    w.word(OUTPUT, 0xdeadbeef);
    assert_eq!(
        w.call("XOnlineMessageCreate", &[2, 0, 0, 0, 0, 0, 0, OUTPUT]),
        OUT_OF_MEMORY
    );
    assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
    for msg in builders {
        assert_eq!(w.call("XOnlineMessageDestroy", &[msg]), 0);
    }
    let msg = message_create(&mut w, 2, 0, 0);
    w.put(DATA + 0x5000, b"Target\0");
    for _ in 0..256 {
        assert_eq!(
            w.call(
                "XOnlineFriendsRequestByNameEx",
                &[0, DATA + 0x5000, msg, 0, OUTPUT]
            ),
            0
        );
        assert_eq!(w.call("XOnlineTaskClose", &[w.u32(OUTPUT)]), 0);
    }
    w.word(OUTPUT, 0xdeadbeef);
    assert_eq!(
        w.call(
            "XOnlineFriendsRequestByNameEx",
            &[0, DATA + 0x5000, msg, 0, OUTPUT]
        ),
        OUT_OF_MEMORY
    );
    assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
    assert_eq!(h.drain_message_sends().len(), 256);
    assert_eq!(
        w.call(
            "XOnlineFriendsRequestByNameEx",
            &[0, DATA + 0x5000, msg, 0, OUTPUT]
        ),
        0
    );
    assert_eq!(h.message_sends()[0].sequence, 257);
}

const MATCH_ATTRS: u32 = DATA + 0x6000;
const MATCH_VALUES: u32 = DATA + 0x7000;
const MATCH_SPECS: u32 = DATA + 0x8000;
const MATCH_KID: u32 = DATA + 0x9000;
const MATCH_KEY: u32 = DATA + 0x9020;
fn match_attr(w: &World, at: u32, id: u32, changed: bool, value: u64) {
    w.word(at, id);
    w.word(at + 4, u32::from(changed));
    w.put(at + 8, &value.to_le_bytes());
}
fn match_create(w: &mut World, count: u32) -> u32 {
    assert_eq!(
        w.call(
            "XOnlineMatchSessionCreate",
            &[1, 7, 2, 3, count, MATCH_ATTRS, 0, OUTPUT]
        ),
        0
    );
    let task = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
    assert_eq!(
        w.call("XOnlineMatchSessionGetInfo", &[task, MATCH_KID, MATCH_KEY]),
        0
    );
    task
}
fn match_id(w: &World) -> [u32; 2] {
    [w.u32(MATCH_KID), w.u32(MATCH_KID + 4)]
}
#[test]
fn match_local_host_lifecycle_keeps_ids_keys_slots_and_attributes_after_task_close() {
    use live_hle::{SessionAttributeValue, SessionSlots};
    let (mut w, h) = outbox_world();
    match_attr(&w, MATCH_ATTRS, 0x101, false, u64::MAX);
    w.put(MATCH_VALUES, &wide_bytes("Synthetic lobby 雪"));
    match_attr(
        &w,
        MATCH_ATTRS + 16,
        0x100102,
        false,
        u64::from(MATCH_VALUES),
    );
    w.put(MATCH_VALUES + 0x200, &[2, 4, 6, 8]);
    match_attr(
        &w,
        MATCH_ATTRS + 32,
        0x200103,
        false,
        u64::from(MATCH_VALUES + 0x200) | (4u64 << 32),
    );
    match_attr(&w, MATCH_ATTRS + 48, 0xf00104, false, u64::MAX);
    let task = match_create(&mut w, 4);
    let id = match_id(&w);
    let key = w.bytes(MATCH_KEY, 16);
    assert_eq!(w.bytes(MATCH_KID, 1)[0] & 0xf0, 0x80);
    assert!(key.iter().any(|b| *b != 0));
    for i in 0..4 {
        assert_eq!(w.u32(MATCH_ATTRS + i * 16 + 4), 0);
    }
    let session = h.local_sessions().remove(0);
    assert_eq!(
        session.slots,
        SessionSlots {
            public_filled: 1,
            public_open: 7,
            private_filled: 2,
            private_open: 3
        }
    );
    assert_eq!(session.host_users, vec![ID]);
    assert_eq!(&session.host_address[..4], &[192, 168, 1, 42]);
    assert_eq!(&session.host_address[8..10], &3074u16.to_be_bytes());
    assert_eq!(
        session.attributes[&0x101],
        SessionAttributeValue::Integer(u64::MAX)
    );
    assert_eq!(
        session.attributes[&0x100102],
        SessionAttributeValue::String("Synthetic lobby 雪".into())
    );
    assert_eq!(
        session.attributes[&0x200103],
        SessionAttributeValue::Blob(vec![2, 4, 6, 8])
    );
    assert_eq!(session.attributes.len(), 3);
    w.put(MATCH_VALUES, &[0xdd; 0x204]);
    for _ in 0..3 {
        assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
        assert_eq!(
            w.call("XOnlineMatchSessionGetInfo", &[task, MATCH_KID, MATCH_KEY]),
            0
        );
        assert_eq!(match_id(&w), id);
        assert_eq!(w.bytes(MATCH_KEY, 16), key);
    }
    // The host wrapper registers exactly the pair returned by GetInfo.
    assert_eq!(w.call("XNetRegisterKey", &[MATCH_KID, MATCH_KEY]), 0);
    assert_eq!(w.call("XNetGetTitleXnAddr", &[MATCH_VALUES]), 4);
    assert_eq!(
        w.call("XNetXnAddrToInAddr", &[MATCH_VALUES, MATCH_KID, OUTPUT]),
        0
    );
    assert_eq!(w.call("XOnlineTaskClose", &[task]), 0);
    assert_eq!(h.local_sessions(), vec![session]);
    assert_eq!(
        w.call("XOnlineMatchSessionGetInfo", &[task, MATCH_KID, MATCH_KEY]),
        INVALID_ARGUMENT
    );
}
#[test]
fn match_updates_consume_dirty_flags_preserve_unchanged_attributes_and_drop_noops() {
    use live_hle::{SessionAttributeValue, SessionSlots};
    let (mut w, h) = outbox_world();
    match_attr(&w, MATCH_ATTRS, 0x101, true, 7);
    match_attr(
        &w,
        MATCH_ATTRS + 16,
        0x100102,
        true,
        u64::from(MATCH_VALUES),
    );
    w.put(MATCH_VALUES, &wide_bytes("Original"));
    match_create(&mut w, 2);
    let [lo, hi] = match_id(&w);
    let key = w.bytes(MATCH_KEY, 16);
    match_attr(&w, MATCH_ATTRS, 0x101, true, 9);
    // Unchanged string data is intentionally invalid: retail does not read it.
    match_attr(&w, MATCH_ATTRS + 16, 0x100102, false, 0xfffffff0);
    assert_eq!(
        w.call(
            "XOnlineMatchSessionUpdate",
            &[lo, hi, 3, 5, 0, 4, 2, MATCH_ATTRS, 0, OUTPUT]
        ),
        0
    );
    let update = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[update]), TASK_SUCCESS);
    assert_eq!(
        w.call(
            "XOnlineMatchSessionGetInfo",
            &[update, MATCH_KID, MATCH_KEY]
        ),
        0
    );
    assert_eq!(match_id(&w), [lo, hi]);
    assert_eq!(w.bytes(MATCH_KEY, 16), key);
    let session = h.local_sessions().remove(0);
    assert_eq!(session.revision, 2);
    assert_eq!(
        session.slots,
        SessionSlots {
            public_filled: 3,
            public_open: 5,
            private_filled: 0,
            private_open: 4
        }
    );
    assert_eq!(
        session.attributes[&0x101],
        SessionAttributeValue::Integer(9)
    );
    assert_eq!(
        session.attributes[&0x100102],
        SessionAttributeValue::String("Original".into())
    );
    assert_eq!(w.u32(MATCH_ATTRS + 4), 0);
    assert_eq!(
        w.call(
            "XOnlineMatchSessionUpdate",
            &[lo, hi, 3, 5, 0, 4, 2, MATCH_ATTRS, 0, OUTPUT]
        ),
        0
    );
    assert_eq!(h.local_sessions()[0].revision, 2);
    match_attr(&w, MATCH_ATTRS, 0xf00101, true, u64::MAX);
    assert_eq!(
        w.call(
            "XOnlineMatchSessionUpdate",
            &[lo, hi, 3, 5, 0, 4, 1, MATCH_ATTRS, 0, OUTPUT]
        ),
        0
    );
    assert!(!h.local_sessions()[0].attributes.contains_key(&0x101));
    assert_eq!(h.local_sessions()[0].revision, 3);
}
#[test]
fn match_delete_completes_independently_and_unknown_sessions_fail_cleanly() {
    let (mut w, h) = outbox_world();
    let create = match_create(&mut w, 0);
    let [lo, hi] = match_id(&w);
    assert_eq!(w.call("XOnlineMatchSessionDelete", &[lo, hi, 0, OUTPUT]), 0);
    let delete = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[delete]), TASK_SUCCESS);
    assert!(h.local_sessions().is_empty());
    assert_eq!(
        w.call(
            "XOnlineMatchSessionGetInfo",
            &[create, MATCH_KID, MATCH_KEY]
        ),
        0
    );
    assert_eq!(
        w.call(
            "XOnlineMatchSessionGetInfo",
            &[delete, MATCH_KID, MATCH_KEY]
        ),
        INVALID_ARGUMENT
    );
    w.word(OUTPUT, 0xdeadbeef);
    assert_eq!(
        w.call(
            "XOnlineMatchSessionUpdate",
            &[lo, hi, 1, 7, 0, 0, 0, 0, 0, OUTPUT]
        ),
        MATCH_INVALID_SESSION
    );
    assert_eq!(
        w.call("XOnlineMatchSessionDelete", &[lo, hi, 0, OUTPUT]),
        MATCH_INVALID_SESSION
    );
    assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
    assert_eq!(w.call("XOnlineTaskClose", &[delete]), 0);
    assert_eq!(w.call("XOnlineTaskContinue", &[delete]), INVALID_ARGUMENT);
}
#[test]
fn match_result_capacity_uses_the_retail_integer_string_and_blob_formula() {
    let mut w = World::new();
    w.startup();
    for (i, (ty, len)) in [(0, 99), (0x100000, 13), (0x200000, 17), (0xf00000, 0)]
        .into_iter()
        .enumerate()
    {
        w.word(MATCH_SPECS + i as u32 * 8, ty);
        w.word(MATCH_SPECS + i as u32 * 8 + 4, len);
    }
    let expected = 0x5c + 3 * (0x54 + (8 + 8) + (8 + 13 * 2 + 2) + (8 + 17 + 2) + 8);
    assert_eq!(
        w.call("XOnlineMatchSearchResultsLen", &[3, 4, MATCH_SPECS]),
        expected
    );
    assert_eq!(w.call("XOnlineMatchSearchResultsLen", &[0, 0, 0]), 0x5c);
    assert_eq!(w.call("XOnlineMatchSearchResultsLen", &[u32::MAX, 0, 0]), 0);
    assert_eq!(w.call("XOnlineMatchSearchResultsLen", &[1, 65, 0]), 0);
    w.word(MATCH_SPECS, 0x300000);
    assert_eq!(
        w.call("XOnlineMatchSearchResultsLen", &[1, 1, MATCH_SPECS]),
        0
    );
}
#[test]
fn match_searches_are_successful_empty_snapshots_and_keep_input_parameters_unchanged() {
    let (mut w, h) = outbox_world();
    // Same argument shapes as the title: procedure 1, up to 50 rows, positional
    // integer/NULL parameters, and seven integer result columns. Synthetic data.
    for i in 0..7 {
        w.word(MATCH_SPECS + i * 8, 0);
        w.word(MATCH_SPECS + i * 8 + 4, 0);
    }
    let len = w.call("XOnlineMatchSearchResultsLen", &[50, 7, MATCH_SPECS]);
    assert_eq!(len, 0x5c + 50 * (0x54 + 7 * 16));
    for i in 0..12 {
        match_attr(
            &w,
            MATCH_ATTRS + i * 16,
            if i % 2 == 0 { 0 } else { 0xf00000 },
            true,
            u64::from(i),
        );
    }
    let before = w.bytes(MATCH_ATTRS, 12 * 16);
    assert_eq!(
        w.call(
            "XOnlineMatchSearch",
            &[1, 50, 12, MATCH_ATTRS, len, 0, OUTPUT]
        ),
        0
    );
    let search = w.u32(OUTPUT);
    assert_eq!(w.bytes(MATCH_ATTRS, 12 * 16), before);
    for _ in 0..2 {
        assert_eq!(w.call("XOnlineTaskContinue", &[search]), TASK_SUCCESS);
        w.put(OUTPUT, &[0xcc; 12]);
        assert_eq!(
            w.call(
                "XOnlineMatchSearchGetResults",
                &[search, OUTPUT, OUTPUT + 4]
            ),
            0
        );
        assert_eq!(w.bytes(OUTPUT, 8), vec![0; 8]);
        assert_eq!(w.bytes(OUTPUT + 8, 4), vec![0xcc; 4]);
    }
    // A local advertisement does not change the deliberately empty discovery view.
    let create = match_create(&mut w, 0);
    let [lo, hi] = match_id(&w);
    for [id_lo, id_hi] in [[lo, hi], [0x12345678, 0x87654321]] {
        assert_eq!(
            w.call("XOnlineMatchSessionFindFromID", &[id_lo, id_hi, 0, OUTPUT]),
            0
        );
        let find = w.u32(OUTPUT);
        assert_eq!(w.call("XOnlineTaskContinue", &[find]), TASK_SUCCESS);
        assert_eq!(
            w.call("XOnlineMatchSearchGetResults", &[find, OUTPUT, OUTPUT + 4]),
            0
        );
        assert_eq!(w.bytes(OUTPUT, 8), vec![0; 8]);
        assert_eq!(w.call("XOnlineTaskClose", &[find]), 0);
    }
    assert_eq!(
        w.call(
            "XOnlineMatchSearchGetResults",
            &[create, OUTPUT, OUTPUT + 4]
        ),
        INVALID_ARGUMENT
    );
    assert_eq!(w.call("XOnlineTaskClose", &[search]), 0);
    assert_eq!(
        w.call(
            "XOnlineMatchSearchGetResults",
            &[search, OUTPUT, OUTPUT + 4]
        ),
        INVALID_ARGUMENT
    );
    assert_eq!(h.local_sessions().len(), 1);
}
#[test]
fn match_empty_parser_matches_retail_early_return_and_rejects_unowned_columns() {
    let mut w = World::new();
    w.startup();
    w.put(MATCH_VALUES, &[0; 0x54]);
    w.put(OUTPUT, &[0xcc; 32]);
    assert_eq!(
        w.call(
            "XOnlineMatchSearchParse",
            &[MATCH_VALUES, u32::MAX, 0, OUTPUT]
        ),
        0
    );
    assert_eq!(w.bytes(OUTPUT, 32), vec![0xcc; 32]);
    w.word(MATCH_VALUES + 0x50, 1);
    assert_eq!(
        w.call(
            "XOnlineMatchSearchParse",
            &[MATCH_VALUES, 1, MATCH_SPECS, OUTPUT]
        ),
        MATCH_INVALID_RESULT_COLUMN
    );
    assert_eq!(w.bytes(OUTPUT, 32), vec![0xcc; 32]);
    assert_eq!(
        w.call("XOnlineMatchSearchParse", &[0, 0, 0, OUTPUT]),
        INVALID_ARGUMENT
    );
}
#[test]
fn match_validation_rejects_oversized_or_malformed_attributes_atomically() {
    let (mut w, h) = outbox_world();
    match_attr(&w, MATCH_ATTRS, 0x101, true, 1);
    for (id, value, expected) in [
        (0x300101, 0, MATCH_INVALID_DATA_TYPE),
        (
            0x200101,
            u64::from(MATCH_VALUES) | (801u64 << 32),
            MATCH_BLOB_TOO_LONG,
        ),
        (0x100101, u64::from(MATCH_VALUES), MATCH_INVALID_STRING),
    ] {
        w.put(MATCH_VALUES, &[0, 0xd8, 0, 0]);
        match_attr(&w, MATCH_ATTRS + 16, id, true, value);
        w.word(OUTPUT, 0xdeadbeef);
        assert_eq!(
            w.call(
                "XOnlineMatchSessionCreate",
                &[1, 7, 0, 0, 2, MATCH_ATTRS, 0, OUTPUT]
            ),
            expected
        );
        assert_eq!(w.u32(MATCH_ATTRS + 4), 1);
        assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
        assert!(h.local_sessions().is_empty());
    }
    w.put(MATCH_VALUES, &wide_bytes(&"x".repeat(401)));
    match_attr(&w, MATCH_ATTRS, 0x100101, true, u64::from(MATCH_VALUES));
    assert_eq!(
        w.call(
            "XOnlineMatchSessionCreate",
            &[1, 7, 0, 0, 1, MATCH_ATTRS, 0, OUTPUT]
        ),
        MATCH_STRING_TOO_LONG
    );
    assert_eq!(
        w.call("XOnlineMatchSessionCreate", &[1, 7, 0, 0, 65, 0, 0, OUTPUT]),
        BUFFER_TOO_SMALL
    );
    assert_eq!(
        w.call("XOnlineMatchSearch", &[1, 50, 0, 0, 0, 0, OUTPUT]),
        MATCH_INVALID_SEARCH
    );
    assert_eq!(
        w.call("XOnlineMatchSearch", &[1, 1001, 0, 0, 0x5c, 0, OUTPUT]),
        MATCH_INVALID_SEARCH
    );
    assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
}
#[test]
fn match_guest_faults_leave_existing_sessions_dirty_flags_and_getter_outputs_untouched() {
    let (mut w, h) = outbox_world();
    match_attr(&w, MATCH_ATTRS, 0x101, true, 1);
    let create = match_create(&mut w, 1);
    let [lo, hi] = match_id(&w);
    let session = h.local_sessions();
    match_attr(&w, MATCH_ATTRS, 0x100101, true, u64::from(DATA + 0xfffe));
    w.put(DATA + 0xfffe, &[1, 0]);
    for (api, args) in [
        (
            "XOnlineMatchSessionCreate",
            vec![1, 7, 0, 0, 1, DATA + 0xfff8, 0, OUTPUT],
        ),
        (
            "XOnlineMatchSessionUpdate",
            vec![lo, hi, 1, 7, 0, 0, 1, MATCH_ATTRS, 0, OUTPUT],
        ),
        (
            "XOnlineMatchSessionCreate",
            vec![1, 7, 0, 0, 0, 0, 0, DATA + 0xfffe],
        ),
        (
            "XOnlineMatchSessionGetInfo",
            vec![create, OUTPUT, DATA + 0xfff8],
        ),
    ] {
        w.put(OUTPUT, &[0xcc; 16]);
        let t = w.raw(api, &args);
        assert!(
            matches!(w.ctx.event(t.event_id), Some(Event::MemoryFault { .. })),
            "{api}"
        );
        assert_eq!(w.bytes(OUTPUT, 16), vec![0xcc; 16]);
        assert_eq!(w.u32(MATCH_ATTRS + 4), 1);
        assert_eq!(h.local_sessions(), session);
    }
    assert_eq!(
        w.call("XOnlineMatchSearch", &[1, 1, 0, 0, 0x5c, 0, OUTPUT]),
        0
    );
    let search = w.u32(OUTPUT);
    w.word(OUTPUT, 0xdeadbeef);
    let t = w.raw(
        "XOnlineMatchSearchGetResults",
        &[search, OUTPUT, DATA + 0xfffe],
    );
    assert!(matches!(
        w.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
}
#[test]
fn match_cleanup_removes_advertisements_and_invalidates_tasks() {
    for close_logon in [false, true] {
        let h = hle(config());
        let mut w = World::with(Arc::clone(&h));
        let logon = w.logon();
        let create = match_create(&mut w, 0);
        assert_eq!(
            w.call("XOnlineMatchSearch", &[1, 1, 0, 0, 0x5c, 0, OUTPUT]),
            0
        );
        let search = w.u32(OUTPUT);
        if close_logon {
            assert_eq!(w.call("XOnlineTaskClose", &[logon]), 0);
        } else {
            assert_eq!(w.call("XOnlineCleanup", &[]), 0);
            w.startup();
        }
        assert!(h.local_sessions().is_empty());
        assert_eq!(w.call("XOnlineTaskContinue", &[create]), INVALID_ARGUMENT);
        assert_eq!(w.call("XOnlineTaskContinue", &[search]), INVALID_ARGUMENT);
        assert_eq!(
            w.call(
                "XOnlineMatchSessionGetInfo",
                &[create, MATCH_KID, MATCH_KEY]
            ),
            INVALID_ARGUMENT
        );
        assert_eq!(
            w.call("XOnlineMatchSessionCreate", &[1, 7, 0, 0, 0, 0, 0, OUTPUT]),
            NO_SESSION
        );
    }
}
#[test]
fn match_session_limit_preserves_advertisements_and_delete_releases_capacity() {
    let (mut w, h) = outbox_world();
    let mut ids = std::collections::BTreeSet::new();
    for _ in 0..32 {
        let task = match_create(&mut w, 0);
        assert!(ids.insert(match_id(&w)));
        assert_eq!(w.call("XOnlineTaskClose", &[task]), 0);
    }
    w.word(OUTPUT, 0xdeadbeef);
    assert_eq!(
        w.call("XOnlineMatchSessionCreate", &[1, 7, 0, 0, 0, 0, 0, OUTPUT]),
        OUT_OF_MEMORY
    );
    assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
    assert_eq!(h.local_sessions().len(), 32);
    let [lo, hi] = *ids.first().unwrap();
    assert_eq!(w.call("XOnlineMatchSessionDelete", &[lo, hi, 0, OUTPUT]), 0);
    match_create(&mut w, 0);
    assert_eq!(h.local_sessions().len(), 32);
}

const TEAM_PROPS: u32 = DATA + 0x4000;
const TEAM_MEMBER_PROPS: u32 = DATA + 0x4700;
const TEAM_IDS: u32 = DATA + 0x5000;
const TEAM_DETAILS: u32 = DATA + 0x6000;
const SECOND: u64 = 0x0009000011112222;
fn team_words(id: live_proto::Xuid) -> [u32; 2] {
    [id.0 as u32, (id.0 >> 32) as u32]
}
fn team_properties(w: &World, name: &str) {
    w.put(TEAM_PROPS, &vec![0; 1670]);
    w.put(TEAM_PROPS, &wide_bytes(name));
    w.put(TEAM_PROPS + 32, &wide_bytes("Description"));
    w.put(TEAM_PROPS + 544, &wide_bytes("Motto"));
    w.put(TEAM_PROPS + 1056, &wide_bytes("https://example.test"));
    w.put(TEAM_PROPS + 1568, &3u16.to_le_bytes());
    w.put(TEAM_PROPS + 1570, &[3, 1, 4]);
    w.put(TEAM_MEMBER_PROPS, &[0; 106]);
    w.word(TEAM_MEMBER_PROPS, u32::MAX);
}
fn team_create_local(w: &mut World, h: &Hle, name: &str) -> live_proto::Xuid {
    team_properties(w, name);
    assert_eq!(
        w.call(
            "XOnlineTeamCreate",
            &[0, TEAM_PROPS, TEAM_MEMBER_PROPS, 100, 0, OUTPUT]
        ),
        0
    );
    let task = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
    assert_eq!(w.call("XOnlineTaskClose", &[task]), 0);
    h.local_teams()
        .of(live_proto::Xuid(ID))
        .into_iter()
        .find(|t| t.properties.name == name)
        .unwrap()
        .id
}
fn teams_two_users() -> (World, Arc<Hle>) {
    let h = hle(config());
    let mut w = World::with(Arc::clone(&h));
    w.startup();
    assert_eq!(w.call("XOnlineGetUsers", &[DATA, OUTPUT]), 0);
    w.put(USERS, &w.bytes(DATA, 224));
    for i in 0..13 {
        w.word(SERVICES + i * 4, i + 2);
    }
    assert_eq!(w.call("XOnlineLogon", &[USERS, SERVICES, 13, 0, OUTPUT]), 0);
    (w, h)
}
fn teams_enumerate(w: &mut World, user: u64) -> u32 {
    assert_eq!(
        w.call(
            "XOnlineTeamEnumerateByUserXUID",
            &[0, user as u32, (user >> 32) as u32, 0, 0, OUTPUT]
        ),
        0
    );
    let task = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), TASK_SUCCESS);
    assert_eq!(
        w.call("XOnlineTeamEnumerateGetResults", &[task, OUTPUT, TEAM_IDS]),
        0
    );
    task
}
#[test]
fn teams_create_enumerate_details_members_and_delete_match_packed_records() {
    let (mut w, h) = outbox_world();
    let id = team_create_local(&mut w, &h, "Synthetic Clan");
    let [lo, hi] = team_words(id);
    w.put(TEAM_PROPS, &[0xdd; 1670]);
    let task = teams_enumerate(&mut w, ID);
    assert_eq!(w.u32(OUTPUT), 1);
    assert_eq!(w.bytes(TEAM_IDS, 8), id.0.to_le_bytes());
    assert_eq!(w.u32(TEAM_IDS + 8), 0);
    w.put(TEAM_DETAILS, &vec![0xcc; 1702]);
    assert_eq!(
        w.call("XOnlineTeamGetDetails", &[task, lo, hi, 0, TEAM_DETAILS]),
        0
    );
    assert_eq!(
        w.bytes(TEAM_DETAILS + 12, wide_bytes("Synthetic Clan").len()),
        wide_bytes("Synthetic Clan")
    );
    assert_eq!(w.u32(TEAM_DETAILS + 1682), 0);
    assert_eq!(w.u32(TEAM_DETAILS + 1694), 1);
    assert!(w.bytes(TEAM_DETAILS + 1686, 8).iter().any(|b| *b != 0));
    assert_eq!(w.bytes(TEAM_DETAILS + 1698, 4), vec![0xcc; 4]);
    assert_eq!(
        w.call("XOnlineTeamMembersEnumerate", &[0, lo, hi, 0, 1, 0, OUTPUT]),
        0
    );
    let members = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[members]), TASK_SUCCESS);
    assert_eq!(
        w.call(
            "XOnlineTeamMembersEnumerateGetResults",
            &[members, OUTPUT, TEAM_IDS]
        ),
        0
    );
    assert_eq!(w.u32(OUTPUT), 1);
    w.put(TEAM_DETAILS, &[0xcc; 150]);
    assert_eq!(
        w.call(
            "XOnlineTeamMemberGetDetails",
            &[members, ID as u32, (ID >> 32) as u32, 0, TEAM_DETAILS]
        ),
        0
    );
    assert_eq!(w.bytes(TEAM_DETAILS + 12, 13), b"First Player\0");
    assert_eq!(w.u32(TEAM_DETAILS + 28), u32::MAX);
    assert_eq!(w.u32(TEAM_DETAILS + 134), 0);
    assert_eq!(w.bytes(TEAM_DETAILS + 146, 4), vec![0xcc; 4]);
    assert_eq!(w.call("XOnlineTeamDelete", &[0, lo, hi, 0, 0, OUTPUT]), 0);
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TASK_SUCCESS
    );
    teams_enumerate(&mut w, ID);
    assert_eq!(w.u32(OUTPUT), 0);
    // Enumeration snapshots continue to own their earlier records.
    assert_eq!(
        w.call("XOnlineTeamGetDetails", &[task, lo, hi, 0, TEAM_DETAILS]),
        0
    );
}
#[test]
fn teams_recruit_accept_promote_remove_and_decline_use_real_member_privileges() {
    use live_proto::team::TeamError;
    let (mut w, h) = teams_two_users();
    let id = team_create_local(&mut w, &h, "Members");
    let [lo, hi] = team_words(id);
    let msg = message_create(&mut w, 4, id.0, 0x40);
    w.word(TEAM_MEMBER_PROPS, 0x10);
    w.put(DATA + 0x4800, b"Second\0");
    assert_eq!(
        w.call(
            "XOnlineTeamMemberRecruitByName",
            &[
                0,
                lo,
                hi,
                0,
                DATA + 0x4800,
                TEAM_MEMBER_PROPS,
                msg,
                0,
                OUTPUT
            ]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TASK_SUCCESS
    );
    let pending = teams_enumerate(&mut w, SECOND);
    assert_eq!(
        w.call("XOnlineTeamGetDetails", &[pending, lo, hi, 0, TEAM_DETAILS]),
        0
    );
    assert_eq!(w.u32(TEAM_DETAILS + 1682), 1);
    assert_eq!(
        w.call(
            "XOnlineTeamMemberAnswerRecruit",
            &[1, lo, hi, 0, 1, 0, OUTPUT]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TASK_SUCCESS
    );
    w.word(TEAM_MEMBER_PROPS, 0x1c);
    assert_eq!(
        w.call(
            "XOnlineTeamMemberSetProperties",
            &[
                1,
                lo,
                hi,
                0,
                SECOND as u32,
                (SECOND >> 32) as u32,
                0,
                TEAM_MEMBER_PROPS,
                0,
                OUTPUT
            ]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TeamError::Privileges.hresult()
    );
    assert_eq!(
        w.call(
            "XOnlineTeamMemberSetProperties",
            &[
                0,
                lo,
                hi,
                0,
                SECOND as u32,
                (SECOND >> 32) as u32,
                0,
                TEAM_MEMBER_PROPS,
                0,
                OUTPUT
            ]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TASK_SUCCESS
    );
    assert_eq!(
        h.local_teams().get(id).unwrap().members[&live_proto::Xuid(SECOND)]
            .properties
            .privileges,
        0x1c
    );
    assert_eq!(
        w.call(
            "XOnlineTeamMemberRemove",
            &[1, lo, hi, 0, ID as u32, (ID >> 32) as u32, 0, 0, OUTPUT]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TeamError::NoAdmin.hresult()
    );
    assert_eq!(
        w.call(
            "XOnlineTeamMemberRemove",
            &[
                1,
                lo,
                hi,
                0,
                SECOND as u32,
                (SECOND >> 32) as u32,
                0,
                0,
                OUTPUT
            ]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TASK_SUCCESS
    );
    w.word(TEAM_MEMBER_PROPS, 0x10);
    assert_eq!(
        w.call(
            "XOnlineTeamMemberRecruit",
            &[
                0,
                lo,
                hi,
                0,
                SECOND as u32,
                (SECOND >> 32) as u32,
                0,
                TEAM_MEMBER_PROPS,
                msg,
                0,
                OUTPUT
            ]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TASK_SUCCESS
    );
    assert_eq!(
        w.call(
            "XOnlineTeamMemberAnswerRecruit",
            &[1, lo, hi, 0, 0, 0, OUTPUT]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TASK_SUCCESS
    );
    assert!(h.local_teams().get(id).unwrap().recruits.is_empty());
}
#[test]
fn teams_properties_seam_updates_owned_fields_and_enumerations_refresh() {
    use live_proto::team::{Action, Properties};
    let (mut w, h) = outbox_world();
    let id = team_create_local(&mut w, &h, "Before");
    let [lo, hi] = team_words(id);
    let properties = Properties {
        name: "After".into(),
        motto: "New motto".into(),
        data: vec![7, 8],
        ..Default::default()
    };
    h.local_team_action(
        ID,
        &Action::Properties {
            id,
            properties: properties.clone(),
        },
    )
    .unwrap();
    let task = teams_enumerate(&mut w, ID);
    assert_eq!(
        w.call("XOnlineTeamGetDetails", &[task, lo, hi, 0, TEAM_DETAILS]),
        0
    );
    assert_eq!(w.bytes(TEAM_DETAILS + 12, 12), wide_bytes("After"));
    assert_eq!(
        w.bytes(TEAM_DETAILS + 12 + 544, 20),
        wide_bytes("New motto")
    );
    assert_eq!(h.local_teams().get(id).unwrap().properties, properties);
}
#[test]
fn teams_validation_and_pointer_faults_never_create_partial_teams() {
    let (mut w, h) = outbox_world();
    team_properties(&w, "Valid");
    w.put(TEAM_PROPS + 1568, &101u16.to_le_bytes());
    w.word(OUTPUT, 0xdeadbeef);
    assert_eq!(
        w.call(
            "XOnlineTeamCreate",
            &[0, TEAM_PROPS, TEAM_MEMBER_PROPS, 100, 0, OUTPUT]
        ),
        INVALID_ARGUMENT
    );
    assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
    assert!(h.local_teams().teams.is_empty());
    team_properties(&w, "Valid");
    for (p, out) in [(DATA + 0xfff0, OUTPUT), (TEAM_PROPS, DATA + 0xfffe)] {
        let t = w.raw("XOnlineTeamCreate", &[0, p, TEAM_MEMBER_PROPS, 100, 0, out]);
        assert!(matches!(
            w.ctx.event(t.event_id),
            Some(Event::MemoryFault { .. })
        ));
        assert!(h.local_teams().teams.is_empty());
    }
    let id = team_create_local(&mut w, &h, "Valid");
    let [lo, hi] = team_words(id);
    let task = teams_enumerate(&mut w, ID);
    w.word(OUTPUT, 0xdeadbeef);
    let t = w.raw(
        "XOnlineTeamEnumerateGetResults",
        &[task, OUTPUT, DATA + 0xfffa],
    );
    assert!(matches!(
        w.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
    assert_eq!(w.u32(OUTPUT), 0xdeadbeef);
    let t = w.raw("XOnlineTeamGetDetails", &[task, lo, hi, 0, DATA + 0xfffa]);
    assert!(matches!(
        w.ctx.event(t.event_id),
        Some(Event::MemoryFault { .. })
    ));
}
#[test]
fn teams_duplicate_names_limits_and_cleanup_preserve_local_store() {
    use live_proto::team::TeamError;
    let (mut w, h) = outbox_world();
    team_create_local(&mut w, &h, "Same Name");
    team_properties(&w, "same name");
    assert_eq!(
        w.call(
            "XOnlineTeamCreate",
            &[0, TEAM_PROPS, TEAM_MEMBER_PROPS, 100, 0, OUTPUT]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TeamError::AlreadyExists.hresult()
    );
    for i in 1..8 {
        team_create_local(&mut w, &h, &format!("Team {i}"));
    }
    team_properties(&w, "Ninth");
    assert_eq!(
        w.call(
            "XOnlineTeamCreate",
            &[0, TEAM_PROPS, TEAM_MEMBER_PROPS, 100, 0, OUTPUT]
        ),
        0
    );
    assert_eq!(
        w.call("XOnlineTaskContinue", &[w.u32(OUTPUT)]),
        TeamError::UserTeamsFull.hresult()
    );
    let task = teams_enumerate(&mut w, ID);
    assert_eq!(w.u32(OUTPUT), 8);
    assert_eq!(w.call("XOnlineCleanup", &[]), 0);
    w.startup();
    assert_eq!(w.call("XOnlineTaskContinue", &[task]), INVALID_ARGUMENT);
    assert_eq!(h.local_teams().of(live_proto::Xuid(ID)).len(), 8);
}
#[test]
fn arbitration_makes_round_ids_and_keeps_registrations_and_reports() {
    use live_hle::{Arbitration, ArbitrationRound};
    const ROUND: u32 = DATA + 0x7000;
    const PROCEDURES: u32 = DATA + 0x8000;
    let h = hle(config());
    let mut w = World::with(h.clone());
    w.logon();
    w.put(OUTPUT, &[0; 8]);
    assert_eq!(w.call("XOnlineArbitrationCreateRoundID", &[OUTPUT]), OK);
    let round_id = u64::from_le_bytes(w.bytes(OUTPUT, 8).try_into().unwrap());
    assert_ne!(round_id, 0);

    let session_id = 0x1122_3344_5566_7788u64;
    w.put(ROUND, &session_id.to_le_bytes());
    w.put(ROUND + 8, &round_id.to_le_bytes());
    assert_eq!(
        w.call("XOnlineArbitrationRegister", &[ROUND, 300, 0xc, 0, OUTPUT]),
        OK
    );
    let registered = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[registered]), TASK_SUCCESS);
    assert_eq!(w.call("XOnlineTaskClose", &[registered]), OK);
    assert_eq!(
        w.call("XOnlineArbitrationExtendRound", &[ROUND, 60, 0, OUTPUT]),
        OK
    );
    let extended = w.u32(OUTPUT);
    assert_eq!(w.call("XOnlineTaskContinue", &[extended]), TASK_SUCCESS);
    assert_eq!(w.call("XOnlineTaskClose", &[extended]), OK);

    for (i, id) in [0x8001u16, 0x8003, 0x8007].into_iter().enumerate() {
        let p = PROCEDURES + i as u32 * 0x88;
        w.put(p, &[0xee; 0x88]);
        w.put(p, &id.to_le_bytes());
    }
    // An event handle, and more than the XDK's 1000 procedures, are refused.
    assert_eq!(
        w.call(
            "XOnlineArbitrationReport",
            &[ROUND, 3, PROCEDURES, 0, 3, 1, OUTPUT]
        ),
        INVALID_ARGUMENT
    );
    assert_eq!(
        w.call(
            "XOnlineArbitrationReport",
            &[ROUND, 1001, PROCEDURES, 0, 3, 0, OUTPUT]
        ),
        INVALID_ARGUMENT
    );
    assert_eq!(
        w.call(
            "XOnlineArbitrationReport",
            &[ROUND, 3, PROCEDURES, 0, 3, 0, OUTPUT]
        ),
        OK
    );
    let reported = w.u32(OUTPUT);
    // The kept copy owns its values: the guest's records can change.
    w.put(PROCEDURES, &[0; 3 * 0x88]);
    assert_eq!(w.call("XOnlineTaskContinue", &[reported]), TASK_SUCCESS);
    assert_eq!(w.call("XOnlineTaskClose", &[reported]), OK);
    assert_eq!(w.call("XOnlineTaskContinue", &[reported]), INVALID_ARGUMENT);

    let round = ArbitrationRound {
        session_id,
        round_id,
    };
    assert_eq!(
        h.arbitration(),
        vec![
            Arbitration::Registered {
                round,
                duration: 300,
                flags: 0xc,
            },
            Arbitration::Extended {
                round,
                duration: 60,
            },
            Arbitration::Reported {
                round,
                flags: 3,
                procedures: vec![0x8001, 0x8003, 0x8007],
            },
        ]
    );
}
