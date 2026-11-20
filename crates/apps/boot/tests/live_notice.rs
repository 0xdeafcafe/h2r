//! The cannot-connect dialog hook through the real Runner, with a fixture
//! function standing in for the game's dialog text fill. No XBE or game.
use std::sync::Arc;

use boot::LiveConfig;
use boot::live_notice::{
    self, CANNOT_CONNECT_ERROR, DIALOG_ERROR_ID, DIALOG_MESSAGE, MESSAGE_UNITS, Notice,
};
use runtime::guest::Permissions;
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, RegisteredModule, Runner, Runtime, abi,
    exec::HARNESS_EXIT,
};

/// The fixture module's leaf: sets EAX to 41 and returns.
const FILL: u32 = 0x0040_2000;
const SHADOW: u32 = 0xa0ff_6000;
const FRAME: u32 = 0x0060_0f00;
const DIALOG: u32 = 0x0070_0000;

fn runtime() -> Arc<Runtime> {
    let mut dispatch = Dispatch::default();
    // SAFETY: the fixture's tables, handlers and probes follow its ABI.
    let registered = unsafe {
        dispatch.register_generated_module(
            &runtime_test_probe::h2gen_fixture_module,
            runtime_test_probe::h2gen_fixture_module_abi_version,
        )
    };
    assert!(matches!(registered, Ok(RegisteredModule { .. })));
    let mut guest = Guest::new();
    // The runner checks execute permission on the fixture's code pages.
    guest
        .map_zeroed(0x0040_0000, 0x5000, Permissions::RX)
        .unwrap();
    guest
        .map_zeroed(0x0060_0000, 0x1000, Permissions::RW)
        .unwrap();
    guest.map_zeroed(DIALOG, 0x1000, Permissions::RW).unwrap();
    Runtime::new(guest, dispatch)
}

/// Show a dialog for `error` with stock text "OLD", run the fill as the game
/// calls it (EBX = dialog, one stack word) and return the message buffer.
fn show(rt: &Arc<Runtime>, error: u32) -> Vec<u16> {
    {
        let mut m = rt.memory();
        m.set_u32(DIALOG + DIALOG_ERROR_ID, error).unwrap();
        m.write_bytes(
            DIALOG + DIALOG_MESSAGE,
            &[b'O', 0, b'L', 0, b'D', 0, 0, 0],
            1,
        )
        .unwrap();
        m.set_u32(FRAME, HARNESS_EXIT).unwrap();
        m.set_u32(FRAME + 4, 1).unwrap();
    }
    let mut cpu = CpuAbi {
        esp: FRAME,
        eax: error,
        ebx: DIALOG,
        eip: FILL,
        ..CpuAbi::default()
    };
    let mut ctx = ExecCtx::new(Arc::clone(rt), 1);
    let t = Runner::default().run(&mut cpu, &mut ctx, FILL);
    assert_eq!(t.kind, abi::STOP, "{t:?} {:?}", ctx.event(t.event_id));
    assert!(
        matches!(ctx.event(t.event_id), Some(Event::HarnessExit)),
        "{:?}",
        ctx.event(t.event_id)
    );
    // The original ran (the fixture's result) and returned to the caller.
    assert_eq!(cpu.eax, 41);
    assert_eq!(cpu.ebx, DIALOG);
    assert_eq!(
        cpu.esp,
        FRAME + 4,
        "the fixture's ret pops only the return address"
    );
    let mut b = vec![0; MESSAGE_UNITS * 2];
    rt.memory()
        .read_bytes(DIALOG + DIALOG_MESSAGE, &mut b, 1)
        .unwrap();
    let units: Vec<u16> = b
        .chunks(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    units[..units.iter().position(|&u| u == 0).unwrap()].to_vec()
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

#[test]
fn not_set_up_text_replaces_the_cannot_connect_message() {
    let rt = runtime();
    assert!(live_notice::install_at(&rt, &Notice::NotSetUp, FILL, SHADOW).unwrap());
    assert_eq!(
        show(&rt, CANNOT_CONNECT_ERROR),
        utf16(&Notice::NotSetUp.text())
    );
    // Every other dialog keeps the game's text.
    assert_eq!(show(&rt, 0x38), utf16("OLD"));
    assert_eq!(show(&rt, 0x9a), utf16("OLD"));
}

#[test]
fn unreachable_server_names_the_url() {
    let rt = runtime();
    let notice = Notice::Unreachable("http://127.0.0.1:8074".into());
    assert!(live_notice::install_at(&rt, &notice, FILL, SHADOW).unwrap());
    let text = String::from_utf16(&show(&rt, CANNOT_CONNECT_ERROR)).unwrap();
    assert!(
        text.starts_with("Can't reach the h2 live server at\r\nhttp://127.0.0.1:8074.\r\n"),
        "{text:?}"
    );
}

#[test]
fn install_registers_once_and_skips_a_missing_original() {
    let rt = runtime();
    let (done, wait) = std::sync::mpsc::channel();
    let r = Arc::clone(&rt);
    // install resolves originals outside edit_dispatch; doing it inside would deadlock.
    std::thread::spawn(move || {
        let first =
            live_notice::install_at(&r, &Notice::NotSetUp, FILL, SHADOW).map_err(|e| e.to_string());
        let names =
            r.edit_dispatch(|d| [FILL, SHADOW].map(|a| d.override_name(a).map(str::to_owned)));
        let _ = done.send((first, names));
    });
    let (first, names) = wait
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("install deadlocked");
    assert_eq!(first, Ok(true));
    assert_eq!(
        names,
        [
            Some("live notice: dialog text".into()),
            Some("live notice: original fill".into())
        ]
    );
    // A second install would wrap the hook itself: refused.
    assert!(live_notice::install_at(&rt, &Notice::NotSetUp, FILL, 0xa0ff_7000).is_err());
    // No generated entry: nothing registered, no error.
    assert!(!live_notice::install_at(&rt, &Notice::NotSetUp, 0x0050_0000, 0xa0ff_8000).unwrap());
    rt.edit_dispatch(|d| assert!(d.override_name(0x0050_0000).is_none()));
}

#[test]
fn local_only_live_installs_nothing() {
    let rt = Runtime::new(Guest::new(), Dispatch::default());
    let before = rt.memory().mappings().to_vec();
    let config = LiveConfig {
        enabled: true,
        accounts_path: None,
        server_url: None,
        offer_new_account: false,
    };
    assert!(!live_notice::install(&rt, &config).unwrap());
    assert_eq!(rt.memory().mappings(), before);
}

#[test]
fn without_live_the_console_reports_no_cable() {
    // The menu shows the cannot-connect dialog because transport never
    // starts: XNet starts only on a link, and the default console has none.
    assert_eq!(kernel::Config::default().console.phy_link, 0);
}
