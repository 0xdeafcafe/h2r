//! The sign-in prompt's opener through the real Runner: a logon of an
//! identity waiting for a code stays pending, and the wrapped
//! XOnlineTaskContinue opens our screen once for that controller, with
//! the pending result kept. No XBE or game build; the screen open is a
//! stand-in override at 0x149f1e that records its arguments.
use boot::signin_screen;
use live_hle::{Hle, LiveConfig, NetworkConfig, ServerOptions, entries::ENTRIES};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, exec::HARNESS_EXIT,
    guest::Permissions,
};
use std::{
    net::{Ipv4Addr, SocketAddrV4},
    sync::{Arc, Mutex},
    time::Duration,
};

const DATA: u32 = 0x700000;
const OUT: u32 = DATA + 0x1000;
const USERS: u32 = DATA + 0x2000;
const SERVICES: u32 = DATA + 0x4000;
const FRAME: u32 = 0x600f00;
const OPEN_SCREEN: u32 = 0x0014_9f1e;

fn call(rt: &Arc<Runtime>, ctx: &mut ExecCtx, name: &str, args: &[u32]) -> (u32, u32) {
    let e = ENTRIES.iter().find(|e| e.name == name).unwrap();
    {
        let mut m = rt.memory();
        m.set_u32(FRAME, HARNESS_EXIT).unwrap();
        for (i, a) in args.iter().enumerate() {
            m.set_u32(FRAME + 4 + i as u32 * 4, *a).unwrap();
        }
    }
    let mut cpu = CpuAbi {
        esp: FRAME,
        eip: e.va,
        ebx: 0x1234_5678,
        ..CpuAbi::default()
    };
    let t = Runner::default().run(&mut cpu, ctx, e.va);
    let ev = ctx.event(t.event_id);
    assert!(matches!(ev, Some(Event::HarnessExit)), "{name}: {ev:?}");
    (cpu.eax, cpu.esp)
}

#[test]
fn a_waiting_sign_in_opens_the_prompt_once_and_stays_pending() {
    let dir = std::env::temp_dir().join(format!("h2-signin-prompt-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("play-accounts.json");
    std::fs::write(
        &path,
        r#"[{"xuid":"0009000000000042","gamertag":"Roamer","needs_code":true}]"#,
    )
    .unwrap();
    let hle = Hle::with_server_options(
        LiveConfig {
            enabled: true,
            accounts_path: Some(path),
            server_url: Some("http://127.0.0.1:9".into()),
            offer_new_account: false,
        },
        NetworkConfig::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 31920)),
        ServerOptions {
            request_timeout: Duration::from_secs(1),
            ..Default::default()
        },
    )
    .unwrap();

    let mut guest = Guest::new();
    guest
        .map_zeroed(0x140000, 0x10000, Permissions::RX)
        .unwrap();
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
    let opened = Arc::new(Mutex::new(Vec::new()));
    {
        let opened = Arc::clone(&opened);
        dispatch.register_override(OPEN_SCREEN, "test: open screen", move |cpu, ctx, _| {
            let rt = Arc::clone(ctx.runtime());
            let m = rt.memory();
            opened
                .lock()
                .unwrap()
                .push((m.u32(cpu.esp + 4)?, m.u32(cpu.esp + 8)?));
            cpu.eax = 0xdead;
            cpu.ebx = 0;
            m.ret(cpu, 8)
        });
    }
    let rt = Runtime::new(guest, dispatch);
    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    assert!(signin_screen::install(&rt, &mut registry, Some(&hle)).unwrap());
    let plan = registry.build(&hooks::Base).unwrap();
    let _installed = plan.install(&rt).unwrap();
    let loader = boot::list_class::loader_of(signin_screen::SCREEN).expect("the class is built");

    let mut ctx = ExecCtx::new(Arc::clone(&rt), 1);
    for (name, args) in [
        ("XNetStartup", vec![0]),
        ("WSAStartup", vec![2, DATA]),
        ("XOnlineStartup", vec![0]),
        ("XOnlineGetUsers", vec![DATA, OUT]),
    ] {
        assert_eq!(call(&rt, &mut ctx, name, &args).0, 0, "{name}");
    }
    {
        let mut m = rt.memory();
        let mut b = vec![0; live_hle::USER_SIZE];
        m.read_bytes(DATA, &mut b, 1).unwrap();
        m.write_bytes(USERS, &b, 1).unwrap();
        m.set_u32(SERVICES, 2).unwrap();
    }
    assert_eq!(
        call(&rt, &mut ctx, "XOnlineLogon", &[USERS, SERVICES, 1, 0, OUT]).0,
        0
    );
    let task = rt.memory().u32(OUT).unwrap();
    let (r, esp) = call(&rt, &mut ctx, "XOnlineTaskContinue", &[task]);
    assert_eq!(r, 0, "still pending, the result kept across the open");
    assert_eq!(esp, FRAME + 8, "stdcall: return address and the task");
    assert_eq!(
        *opened.lock().unwrap(),
        [(1, loader)],
        "controller 0's mask"
    );
    assert_eq!(call(&rt, &mut ctx, "XOnlineTaskContinue", &[task]).0, 0);
    assert_eq!(opened.lock().unwrap().len(), 1, "once per sign-in");
    let prompt = hle.signin().unwrap();
    assert_eq!(prompt.gamertag, "Roamer");
    std::fs::remove_dir_all(&dir).unwrap();
}
