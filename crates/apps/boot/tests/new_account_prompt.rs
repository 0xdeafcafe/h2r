//! The app builds' "New Account" identity through the real Runner: with
//! no accounts file yet, `XOnlineGetUsers` lists only it; signing in with
//! it stays pending and the wrapped XOnlineTaskContinue opens the sign-in
//! prompt, which asks for a gamertag (typed on the game's keyboard). Its
//! own process: the classes' loaders are process-wide. No XBE or game
//! build; the screen open is a stand-in override at 0x149f1e.
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
fn the_new_account_identity_opens_the_prompt_asking_for_a_gamertag() {
    let dir = std::env::temp_dir().join(format!("h2-new-account-{}", std::process::id()));
    // The app's first start: the accounts file is not there yet.
    let path = dir.join("live").join("accounts.json");
    let hle = Hle::with_server_options(
        LiveConfig {
            enabled: true,
            accounts_path: Some(path.clone()),
            server_url: Some("http://127.0.0.1:9".into()),
            offer_new_account: true,
        },
        NetworkConfig::new(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 31921)),
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
            cpu.eax = 0;
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
        assert_eq!(m.u32(OUT).unwrap(), 1, "only the new-account identity");
        let mut b = vec![0; live_hle::USER_SIZE];
        m.read_bytes(DATA, &mut b, 1).unwrap();
        assert_eq!(
            &b[12..12 + live_hle::NEW_ACCOUNT_TAG.len()],
            live_hle::NEW_ACCOUNT_TAG.as_bytes()
        );
        m.write_bytes(USERS, &b, 1).unwrap();
        m.set_u32(SERVICES, 2).unwrap();
    }
    assert_eq!(
        call(&rt, &mut ctx, "XOnlineLogon", &[USERS, SERVICES, 1, 0, OUT]).0,
        0
    );
    let task = rt.memory().u32(OUT).unwrap();
    assert_eq!(
        call(&rt, &mut ctx, "XOnlineTaskContinue", &[task]).0,
        0,
        "pending"
    );
    assert_eq!(
        *opened.lock().unwrap(),
        [(1, loader)],
        "controller 0's prompt"
    );
    let prompt = hle.signin().unwrap();
    assert_eq!(
        prompt.phase,
        live_hle::signin::Phase::Gamertag { error: None }
    );
    // The prompt's page asks for it, A opening the keyboard.
    let (label, body) = signin_screen::page(Some(&prompt), &Default::default(), 0);
    assert_eq!(label, "New Xbox Live account");
    assert!(body.contains("A: type it"), "{body}");
    assert!(!path.exists(), "nothing written before a gamertag is made");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A on the gamertag or invite page opens the retail keyboard: the
/// opener at 0x00238c21 gets the player in CL and (context, buffer,
/// capacity) on the stack, the buffer empty, and the keyboard's title ids
/// answer with our words while it is open.
#[test]
fn a_opens_the_game_keyboard_on_our_buffer() {
    use boot::signin_keyboard::{self as keyboard, Field};
    use hooks::{EntryFuture, Ret};
    const STACK: u32 = 0x600e00;
    let hle = Hle::with_address(
        LiveConfig::default(),
        SocketAddrV4::new(Ipv4Addr::LOCALHOST, 31922),
    )
    .unwrap();
    let mut guest = Guest::new();
    guest
        .map_zeroed(0x230000, 0x10000, Permissions::RX)
        .unwrap();
    guest
        .map_zeroed(0x540000, 0x20000, Permissions::RW)
        .unwrap();
    guest.map_zeroed(0x600000, 0x1000, Permissions::RW).unwrap();
    guest.map_zeroed(DATA, 0x40000, Permissions::RW).unwrap();
    let mut dispatch = Dispatch::default();
    let seen = Arc::new(Mutex::new(Vec::new()));
    {
        let seen = Arc::clone(&seen);
        dispatch.register_override(
            keyboard::OPEN,
            "test: keyboard opener",
            move |cpu, ctx, _| {
                let rt = Arc::clone(ctx.runtime());
                let mut m = rt.memory();
                let args = [
                    m.u32(cpu.esp + 4)?,
                    m.u32(cpu.esp + 8)?,
                    m.u32(cpu.esp + 12)?,
                ];
                seen.lock()
                    .unwrap()
                    .push((cpu.ecx & 0xff, args, m.u16(args[1])?));
                // The keyboard screen, editing the caller's buffer.
                let screen = DATA + 0x8000;
                m.set_u32(boot::keyboard_glue::CHANNEL + 8, screen)?;
                m.set_u32(screen, boot::keyboard_glue::VTABLE)?;
                m.set_u32(screen + boot::keyboard_glue::BUFFER, args[1])?;
                m.ret(cpu, 12)
            },
        );
    }
    let rt = Runtime::new(guest, dispatch);
    let rows = Arc::new(signin_screen::SigninRows::new(Arc::clone(&hle)));
    let mut registry = hooks::Registry::new(hooks::Switches::defaults());
    registry
        .add(&signin_screen::Strings(Arc::clone(&rows)))
        .unwrap();
    let placed = Arc::new(std::sync::atomic::AtomicU32::new(0));
    {
        let (rows, placed) = (Arc::clone(&rows), Arc::clone(&placed));
        registry
            .add(&hooks::from_fn("test-open", move |b| {
                let rows = Arc::clone(&rows);
                let at = b.entry("open", move |cx| -> EntryFuture {
                    let rows = Arc::clone(&rows);
                    Box::pin(async move {
                        signin_screen::open_keyboard(&cx, &rows, Field::Invite).await?;
                        Ok(Ret::new(0, 0))
                    })
                })?;
                placed.store(at, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            }))
            .unwrap();
    }
    let plan = registry.build(&hooks::Base).unwrap();
    let _installed = plan.install(&rt).unwrap();
    let buffer = rows.keyboard_buffer().expect("the page is in");
    let base = buffer - 7 * signin_screen::BUFFER;
    assert_eq!(
        rows.string(base, keyboard::INVITE_TITLE_ID),
        None,
        "not open yet"
    );
    rt.memory()
        .write_bytes(buffer, b"o\0l\0d\0\0\0", 1)
        .unwrap();

    rt.memory().set_u32(STACK, HARNESS_EXIT).unwrap();
    let mut cpu = CpuAbi {
        esp: STACK,
        ..CpuAbi::default()
    };
    let mut ctx = ExecCtx::new(Arc::clone(&rt), 1);
    let at = placed.load(std::sync::atomic::Ordering::SeqCst);
    let t = Runner::default().run(&mut cpu, &mut ctx, at);
    assert!(matches!(ctx.event(t.event_id), Some(Event::HarnessExit)));
    assert_eq!(cpu.esp, STACK + 4, "the opener popped its three arguments");
    assert_eq!(
        *seen.lock().unwrap(),
        [(0, [keyboard::INVITE_CONTEXT, buffer, 65], 0)],
        "controller 0, context 20, our buffer emptied, 64 characters"
    );
    let title = rows
        .string(base, keyboard::INVITE_TITLE_ID)
        .expect("served while open");
    let mut text = [0u8; 12];
    rt.memory().read_bytes(title, &mut text, 1).unwrap();
    assert_eq!(&text, b"Invite code\0");
}
