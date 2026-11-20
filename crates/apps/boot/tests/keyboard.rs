//! Synthetic screen memory and native-call fixtures; no retail bytes or game.
use boot::keyboard_glue::{self as k, Screen};
use input::{
    backend::keyboard::{KeyboardMouse, TextAction},
    bindings::Bindings,
};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, exec::HARNESS_EXIT,
    guest::Permissions,
};
use std::sync::{Arc, Mutex};

const OBJECT: u32 = 0x700000;
const TEXT: u32 = 0x710000;
const FRAME: u32 = 0x600e00;
const PUMP: u32 = 0x402000;

fn memory() -> Guest {
    let mut m = Guest::new();
    for (base, len, perm) in [
        (0x540000, 0x10000, Permissions::RW),
        (OBJECT, 0x4000, Permissions::RW),
        (TEXT, 0x1000, Permissions::RW),
        (0x600000, 0x1000, Permissions::RW),
        (0x400000, 0x5000, Permissions::RX),
        (0x230000, 0x10000, Permissions::RX),
    ] {
        m.map_zeroed(base, len, perm).unwrap();
    }
    m.set_u32(k::CHANNEL + 8, OBJECT).unwrap();
    m.set_u32(OBJECT, k::VTABLE).unwrap();
    m.set_u32(OBJECT + 0x70, k::SCREEN_ID).unwrap();
    m.set_u16(OBJECT + 8, 1).unwrap();
    m.set_u32(OBJECT + 0x610, 14).unwrap();
    m.set_u32(OBJECT + 0x372c, u32::MAX).unwrap();
    m.set_u32(OBJECT + k::BUFFER, TEXT).unwrap();
    m.set_u32(OBJECT + k::CARET, TEXT + 2).unwrap();
    m.set_u16(OBJECT + k::CAPACITY, 16).unwrap();
    m.set_u16(TEXT, u16::from(b'A')).unwrap();
    m
}
#[test]
fn focused_screen_requires_the_retail_identity_a_writable_terminated_buffer_and_a_valid_caret() {
    let good = memory();
    let screen = k::detect(&good).unwrap();
    assert_eq!(
        screen,
        Screen {
            address: OBJECT,
            buffer: TEXT,
            caret: TEXT + 2,
            capacity: 16,
            context: 14,
            controller: 0
        }
    );
    for (offset, value) in [
        (0, 1),
        (4, 1),
        (0x70, 1),
        (k::BUFFER, 0),
        (k::CARET, TEXT + 1),
        (k::CARET, TEXT + 4),
        (0x610, 21),
        (0x372c, 3),
    ] {
        let mut m = memory();
        m.set_u32(OBJECT + offset, value).unwrap();
        assert!(k::detect(&m).is_none(), "{offset:#x}");
    }
    for cap in [0, 151, 65535] {
        let mut m = memory();
        m.set_u16(OBJECT + k::CAPACITY, cap).unwrap();
        assert!(k::detect(&m).is_none());
    }
    let mut m = memory();
    m.set_u8(OBJECT + 0x42, 2).unwrap();
    assert!(k::detect(&m).is_none());
    let mut m = memory();
    m.set_u32(k::CHANNEL + 12, OBJECT + 0x100).unwrap();
    assert!(k::detect(&m).is_none());
    for modal in [0x54d99c, 0x54d964] {
        let mut m = memory();
        m.set_u32(modal + 8, OBJECT + 0x100).unwrap();
        assert!(k::detect(&m).is_none());
    }
    let mut m = memory();
    for i in 0..16 {
        m.set_u16(TEXT + i * 2, 65).unwrap();
    }
    assert!(k::detect(&m).is_none());
    let mut m = memory();
    m.unmap(TEXT, 0x1000).unwrap();
    assert!(k::detect(&m).is_none());
    assert!(k::detect(&Guest::new()).is_none());
}
type Calls = Arc<Mutex<Vec<(u32, u32)>>>;

fn world() -> (Arc<Runtime>, KeyboardMouse, Calls) {
    world_with_tape(None)
}

fn world_with_tape(tape: Option<Arc<dyn film::tape::Tap>>) -> (Arc<Runtime>, KeyboardMouse, Calls) {
    let mut d = Dispatch::default();
    unsafe {
        d.register_generated_module(
            &runtime_test_probe::h2gen_fixture_module,
            runtime_test_probe::h2gen_fixture_module_abi_version,
        )
        .unwrap();
    }
    let log = Arc::new(Mutex::new(Vec::new()));
    for entry in [k::INSERT, k::BACKSPACE, k::KEY_ACTION, k::EVENT] {
        let log = log.clone();
        d.register_override(entry, "synthetic keyboard action", move |cpu, ctx, _| {
            let mut m = ctx.runtime().memory();
            let arg = match entry {
                k::INSERT => {
                    assert_eq!(cpu.eax, OBJECT);
                    m.u32(cpu.esp + 4)?
                }
                k::BACKSPACE => {
                    assert_eq!(cpu.eax, OBJECT);
                    0
                }
                k::KEY_ACTION => {
                    assert_eq!(cpu.ecx, OBJECT);
                    assert_eq!(cpu.eax, 0x28);
                    m.set_u32(k::CHANNEL + 8, 0)?;
                    cpu.eax
                }
                k::EVENT => {
                    assert_eq!(cpu.ecx, OBJECT);
                    let e = m.u32(cpu.esp + 4)?;
                    assert_eq!(m.u32(e)?, 5);
                    assert_eq!(m.u32(e + 4)?, 0);
                    assert_eq!(m.u32(e + 8)?, 1);
                    m.set_u32(k::CHANNEL + 8, 0)?;
                    1
                }
                _ => unreachable!(),
            };
            log.lock().unwrap().push((entry, arg));
            cpu.eax = 1;
            m.ret(
                cpu,
                if matches!(entry, k::INSERT | k::EVENT) {
                    4
                } else {
                    0
                },
            )
        });
    }
    let rt = Runtime::new(memory(), d);
    let kb = KeyboardMouse::new(Bindings::default());
    assert!(k::install_with_tape(&rt, kb.clone(), PUMP, tape).unwrap());
    (rt, kb, log)
}
fn pump(rt: &Arc<Runtime>) -> CpuAbi {
    rt.memory().set_u32(FRAME, HARNESS_EXIT).unwrap();
    let mut cpu = CpuAbi {
        esp: FRAME,
        eax: 9,
        ecx: 10,
        edx: 11,
        ebx: 12,
        esi: 13,
        edi: 14,
        ebp: 15,
        ..Default::default()
    };
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    let t = Runner::default().run(&mut cpu, &mut ctx, PUMP);
    assert!(
        matches!(ctx.event(t.event_id), Some(Event::HarnessExit)),
        "{:?}",
        ctx.event(t.event_id)
    );
    assert_eq!(cpu.esp, FRAME + 4);
    assert_eq!(cpu.eax, 41, "preserve original return");
    assert_eq!(
        (cpu.ecx, cpu.edx, cpu.ebx, cpu.esi, cpu.edi, cpu.ebp),
        (10, 11, 12, 13, 14, 15)
    );
    cpu
}
#[test]
fn host_commits_call_native_insert_delete_and_done_with_their_exact_abis() {
    let (rt, kb, log) = world();
    pump(&rt);
    assert!(kb.text_active());
    kb.commit_text("aÉ中😀\u{e240}");
    kb.text_key(Some("Backspace"), true, false, Some(TextAction::Backspace));
    kb.text_key(Some("Enter"), true, false, Some(TextAction::Accept));
    kb.commit_text("must not follow Done");
    pump(&rt);
    assert_eq!(
        *log.lock().unwrap(),
        vec![
            (k::INSERT, 97),
            (k::INSERT, 0xc9),
            (k::INSERT, 0x4e2d),
            (k::INSERT, 0x25a1),
            (k::BACKSPACE, 0),
            (k::KEY_ACTION, 0x28)
        ]
    );
    assert!(!kb.text_active());
    assert_eq!(
        rt.memory().u16(TEXT).unwrap(),
        65,
        "native callee owns editing"
    );
}
#[test]
fn escape_uses_the_native_backup_restore_and_back_event_and_modal_focus_discards_queued_text() {
    let (rt, kb, log) = world();
    pump(&rt);
    kb.commit_text("discarded");
    rt.memory().set_u32(0x54d99c + 8, OBJECT + 0x100).unwrap();
    pump(&rt);
    assert!(!kb.text_active());
    assert!(log.lock().unwrap().is_empty());
    rt.memory().set_u32(0x54d99c + 8, 0).unwrap();
    pump(&rt);
    kb.text_key(Some("Escape"), true, false, Some(TextAction::Cancel));
    pump(&rt);
    assert_eq!(*log.lock().unwrap(), vec![(k::EVENT, 1)]);
    assert!(!kb.text_active());
}

#[test]
fn film_replays_guest_edits_and_ignores_live_typing() {
    use film::tape::{Call, Tap};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Default)]
    struct KeyboardTape {
        replay: AtomicBool,
        calls: Mutex<VecDeque<Call>>,
    }
    impl Tap for KeyboardTape {
        fn replaying(&self) -> bool {
            self.replay.load(Ordering::Relaxed)
        }
        fn next(&self, op: &str, args: &[u32], thread: u32) -> Option<Call> {
            let call = self.calls.lock().unwrap().pop_front().unwrap();
            assert_eq!(call.op, op);
            assert_eq!(call.args, args);
            assert_eq!(call.thread, thread);
            Some(call)
        }
        fn record(&self, tag: film::Tag, call: Call) {
            assert_eq!(tag, film::tape::TEXT);
            self.calls.lock().unwrap().push_back(call);
        }
        fn fault(&self, reason: String) {
            panic!("{reason}");
        }
    }

    let tape = Arc::new(KeyboardTape::default());
    let (game, keys, log) = world_with_tape(Some(tape.clone()));
    pump(&game); // wait one focused UI frame before the player types
    keys.commit_text("R中");
    keys.text_key(Some("Enter"), true, false, Some(TextAction::Accept));
    pump(&game);
    assert_eq!(log.lock().unwrap().len(), 3);

    tape.replay.store(true, Ordering::Relaxed);
    let (game, keys, replayed) = world_with_tape(Some(tape.clone()));
    pump(&game);
    assert!(replayed.lock().unwrap().is_empty());
    keys.commit_text("another name");
    pump(&game);
    assert_eq!(*replayed.lock().unwrap(), *log.lock().unwrap());
    assert!(tape.calls.lock().unwrap().is_empty());
}
