//! The registered overrides through the real runtime Runner and Dispatch.
mod common;

use common::{Pads, Utility};
use input::{
    InputManager,
    gamepad::{Feedback, Rumble},
    host::ConvertConfig,
};
use runtime::{
    CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi, abi, exec::HARNESS_EXIT,
    guest::Permissions,
};
use std::{
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};
use xapi::{
    Config, Error, Events, GuestLayout, RegisterError, Result, Xapi, register, service_input,
    unregister,
};

/// The 16 override addresses and their `ret N` sizes, written out here
/// independently of `xapi::register::OVERRIDES` and `stack_words()`.
const PINNED: [(u32, u32); 16] = [
    (0x002d07f2, 8),  // XLaunchNewImageA
    (0x002d5b77, 4),  // XMountUtilityDrive
    (0x002d5c7c, 0),  // XFormatUtilityDrive
    (0x002d5d0a, 0),  // XMountCacheDrive (N:)
    (0x002d6004, 0),  // XAutoPowerDownResetTimer
    (0x002d6031, 0),  // XapiInitAutoPowerDown
    (0x00408c57, 8),  // XInitDevices
    (0x004098c0, 8),  // XInitDevices thunk
    (0x00409884, 12), // XPeekDevices
    (0x004098c5, 12), // XGetDeviceChanges
    (0x00409932, 16), // XInputOpen
    (0x00409988, 4),  // XInputClose
    (0x00409994, 8),  // XInputGetCapabilities
    (0x00409b6c, 8),  // XInputGetState
    (0x00409bdf, 8),  // XInputSetState
    (0x0041136f, 20), // XReadMUMetaData
];

const GAMEPAD: u32 = 0x004086f8;
const FS: u32 = 0x0050_0000;
const LAST_ERROR: u32 = 0x0050_2008;
const STACK_TOP: u32 = 0x0061_0000;
const SCRATCH: u32 = 0x0070_0000;

type State = Arc<Mutex<Xapi<Utility>>>;

fn xapi() -> State {
    let mut input = InputManager::new(ConvertConfig::default()).unwrap();
    input.add_backend(Box::new(Pads::new()));
    Arc::new(Mutex::new(
        Xapi::new(Config::for_gamepad(GAMEPAD), input, Utility::default()).unwrap(),
    ))
}

struct World {
    runtime: Arc<Runtime>,
    ctx: ExecCtx,
    cpu: CpuAbi,
    xapi: State,
    registration: xapi::Registration,
    /// The whole stack mapping just before the last `call` ran.
    before: Vec<u8>,
}

fn world() -> World {
    let x = xapi();
    let mut d = Dispatch::default();
    register(&mut d, &x, GuestLayout::default()).unwrap();
    world_with(d, x)
}

fn world_with(dispatch: Dispatch, xapi: State) -> World {
    let mut g = Guest::new();
    // Executable pages the overrides sit on; the runner checks Execute.
    g.map_zeroed(0x002d_0000, 0x7000, Permissions::RWX).unwrap();
    g.map_zeroed(0x0040_8000, 0xa000, Permissions::RWX).unwrap();
    // _tls_index = 0 at 0x480100; [fs:4] -> TLS array -> block, cell at +8.
    g.map_zeroed(0x0048_0000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(FS, 0x3000, Permissions::RW).unwrap();
    g.set_u32(FS + 4, FS + 0x1000).unwrap();
    g.set_u32(FS + 0x1000, FS + 0x2000).unwrap();
    g.set_u32(LAST_ERROR, 1234).unwrap();
    g.map_zeroed(0x0046_1000, 0x1000, Permissions::RW).unwrap();
    g.set_u32(0x0046_1d80, u32::MAX).unwrap();
    g.map_zeroed(STACK_TOP - 0x1_0000, 0x1_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(SCRATCH, 0x1_0000, Permissions::RW).unwrap();
    let runtime = Runtime::new(g, dispatch);
    let registration = xapi::Registration {
        installed: xapi::register::OVERRIDES.to_vec(),
    };
    let ctx = ExecCtx::new(Arc::clone(&runtime), 1);
    let cpu = CpuAbi {
        fs_base: FS,
        ..CpuAbi::default()
    };
    World {
        runtime,
        ctx,
        cpu,
        xapi,
        registration,
        before: Vec::new(),
    }
}

impl World {
    /// stdcall `target(args...)` returning to HARNESS_EXIT.
    fn call(&mut self, target: u32, args: &[u32]) -> TransferAbi {
        let esp = STACK_TOP - 0x100;
        {
            let mut m = self.runtime.memory();
            m.set_u32(esp, HARNESS_EXIT).unwrap();
            for (i, a) in args.iter().enumerate() {
                m.set_u32(esp + 4 + 4 * i as u32, *a).unwrap();
            }
        }
        self.cpu.esp = esp;
        self.cpu.eax = 0xdead_beef;
        self.before = self.stack();
        Runner::default().run(&mut self.cpu, &mut self.ctx, target)
    }

    fn stack(&self) -> Vec<u8> {
        let mut v = vec![0; 0x1_0000];
        self.runtime
            .memory()
            .read_bytes(STACK_TOP - 0x1_0000, &mut v, 1)
            .unwrap();
        v
    }

    /// Call and require a return with the pinned cleanup; gives EAX.
    fn returns(&mut self, target: u32, args: &[u32]) -> u32 {
        let cleanup = PINNED.iter().find(|p| p.0 == target).unwrap().1;
        assert_eq!(args.len() as u32 * 4, cleanup, "{target:#x} test arguments");
        let esp = STACK_TOP - 0x100;
        let t = self.call(target, args);
        assert_eq!(t.kind, abi::STOP, "{target:#x}: {t:?}");
        assert!(
            matches!(self.ctx.event(t.event_id), Some(Event::HarnessExit)),
            "{target:#x} did not return: {:?}",
            self.ctx.event(t.event_id)
        );
        assert_eq!(self.cpu.esp, esp + 4 + cleanup, "{target:#x} cleanup");
        self.cpu.eax
    }

    fn u32(&self, a: u32) -> u32 {
        self.runtime.memory().u32(a).unwrap()
    }
    fn put(&self, a: u32, b: &[u8]) {
        self.runtime.memory().write_bytes(a, b, 1).unwrap();
    }
}

/// Signals like the kernel: it relocks guest memory, here on another thread
/// with a timeout, so signalling under a held guard fails instead of hanging.
struct KernelLike {
    runtime: Arc<Runtime>,
    calls: Vec<u32>,
}

impl Events for KernelLike {
    fn signal(&mut self, guest_handle: u32) -> Result<()> {
        let (tx, rx) = mpsc::channel();
        let runtime = Arc::clone(&self.runtime);
        thread::spawn(move || {
            let _guard = runtime.memory();
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(2))
            .map_err(|_| Error::Win32(0xdead))?;
        self.calls.push(guest_handle);
        Ok(())
    }
}

#[test]
fn registration_installs_exactly_the_pinned_addresses() {
    let mut d = Dispatch::default();
    let r = register(&mut d, &xapi(), GuestLayout::default()).unwrap();
    let mut got = r.addresses();
    got.sort_unstable();
    let mut want: Vec<u32> = PINNED.iter().map(|p| p.0).collect();
    want.sort_unstable();
    assert_eq!(got, want);
    for (a, _) in PINNED {
        assert!(
            d.override_name(a).is_some_and(|n| n.starts_with("xapi::")),
            "{a:#x}"
        );
    }
    // The ABI the adapter uses matches the pins.
    for o in &r.installed {
        let pinned = PINNED.iter().find(|p| p.0 == o.addr).unwrap().1;
        assert_eq!(o.function.stack_words() as u32 * 4, pinned, "{}", o.name);
    }
    // Unverified adapters have no address.
    assert!(!r.installed.iter().any(|o| matches!(
        o.function,
        xapi::adapter::Function::XGetDevices | xapi::adapter::Function::XInputPoll
    )));
}

#[test]
fn registering_twice_is_reported_and_changes_nothing() {
    let mut d = Dispatch::default();
    let x = xapi();
    register(&mut d, &x, GuestLayout::default()).unwrap();
    let Err(RegisterError::Occupied(conflicts)) = register(&mut d, &x, GuestLayout::default())
    else {
        panic!("second registration was accepted");
    };
    assert_eq!(conflicts.len(), 16);
    assert!(conflicts.iter().all(|c| c.existing == c.ours));
}

#[test]
fn registering_over_a_foreign_override_installs_nothing() {
    let mut d = Dispatch::default();
    d.register_override(0x0040_9932, "someone-else", |_, _, _| {
        Ok(TransferAbi::ret(0))
    });
    let Err(RegisterError::Occupied(conflicts)) = register(&mut d, &xapi(), GuestLayout::default())
    else {
        panic!("registration over a foreign override was accepted");
    };
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].addr, 0x0040_9932);
    assert_eq!(conflicts[0].existing, "someone-else");
    assert_eq!(d.override_name(0x0040_9932), Some("someone-else"));
    for (a, _) in PINNED.iter().filter(|p| p.0 != 0x0040_9932) {
        assert_eq!(d.override_name(*a), None, "{a:#x} installed anyway");
    }
}

#[test]
fn unregister_disables_and_a_later_register_is_reported() {
    let mut d = Dispatch::default();
    let x = xapi();
    let r = register(&mut d, &x, GuestLayout::default()).unwrap();
    unregister(&mut d, &r);
    assert!(register(&mut d, &x, GuestLayout::default()).is_err());
}

#[test]
fn unregistered_addresses_fall_back_to_the_original_dispatch() {
    let mut w = world();
    let r = w.registration.clone();
    w.runtime.edit_dispatch(|d| unregister(d, &r));
    for (addr, cleanup) in PINNED {
        let args = vec![1; cleanup as usize / 4];
        let t = w.call(addr, &args);
        // No generated original is registered here, so the fallback is the
        // runner's UnsupportedTarget fault, never the disabled override.
        assert_eq!(t.kind, abi::FAULT, "{addr:#x} still dispatched: {t:?}");
        assert!(
            matches!(
                w.ctx.event(t.event_id),
                Some(Event::UnsupportedTarget { target, .. }) if *target == addr
            ),
            "{addr:#x}: {:?}",
            w.ctx.event(t.event_id)
        );
        assert_eq!(w.stack(), w.before, "{addr:#x} touched the stack");
    }
    assert!(w.xapi.lock().unwrap().storage.calls.is_empty());
    assert_eq!(w.xapi.lock().unwrap().take_launch_request(), None);
}

#[test]
fn every_override_decodes_its_frame_and_pops_its_arguments() {
    let mut w = world();
    let sentinel = 0x5a5a_5a5au32.to_le_bytes();
    for off in (0..0x40).step_by(4) {
        w.put(SCRATCH + off, &sentinel);
    }

    // Both init entries reach the same S8 adapter.
    w.returns(0x0040_8c57, &[0, 0]);
    w.returns(0x0040_98c0, &[0, 0]);

    // XGetDeviceChanges(type, &insertions, &removals) -> BOOL.
    let changed = w.returns(0x0040_98c5, &[GAMEPAD, SCRATCH, SCRATCH + 4]);
    let (ins, rem) = (w.u32(SCRATCH), w.u32(SCRATCH + 4));
    assert_ne!(ins, 0x5a5a_5a5a, "insertions not written through arg 2");
    assert_ne!(rem, 0x5a5a_5a5a, "removals not written through arg 3");
    assert_eq!(changed != 0, (ins | rem) != 0);

    // XPeekDevices(type, &previous, &reinserted) -> current mask.
    let current = w.returns(0x0040_9884, &[GAMEPAD, SCRATCH + 8, SCRATCH + 12]);
    assert_eq!(current & 1, 1, "pad on port 0");
    assert_ne!(w.u32(SCRATCH + 8), 0x5a5a_5a5a);

    // XInputOpen(type, port, slot, params) -> handle.
    let handle = w.returns(0x0040_9932, &[GAMEPAD, 0, 0, 0]);
    assert_eq!(handle, 0x5800_0000);

    // Capabilities (25 bytes) and state (22 bytes) land at arg 2.
    assert_eq!(w.returns(0x0040_9994, &[handle, SCRATCH + 0x100]), 0);
    assert_eq!(w.returns(0x0040_9b6c, &[handle, SCRATCH + 0x200]), 0);

    // XInputSetState(handle, feedback) -> ERROR_IO_PENDING, status 997.
    let feedback = Feedback {
        status: 42,
        h_event: 0x44,
        rumble: Rumble {
            left_motor: 1,
            right_motor: 2,
        },
    };
    w.put(SCRATCH + 0x300, &feedback.to_bytes());
    assert_eq!(w.returns(0x0040_9bdf, &[handle, SCRATCH + 0x300]), 997);
    assert_eq!(w.u32(SCRATCH + 0x300), 997);

    // The pump delivers the event with every guard released.
    let mut events = KernelLike {
        runtime: Arc::clone(&w.runtime),
        calls: Vec::new(),
    };
    service_input(&w.runtime, &w.xapi, &mut events).unwrap();
    assert_eq!(events.calls, [0x44]);
    assert_eq!(w.u32(SCRATCH + 0x300), 0, "completion status");
    // Acknowledged: the buffer can be resubmitted.
    assert_eq!(w.returns(0x0040_9bdf, &[handle, SCRATCH + 0x300]), 997);

    // XInputClose(handle) is void: EAX is not a result.
    w.returns(0x0040_9988, &[handle]);
    // A closed handle now reports ERROR_INVALID_HANDLE as a status.
    assert_eq!(w.returns(0x0040_9b6c, &[handle, SCRATCH + 0x200]), 6);

    // A failed XInputOpen returns NULL and sets the guest last-error cell.
    assert_eq!(w.u32(LAST_ERROR), 1234);
    assert_eq!(w.returns(0x0040_9932, &[GAMEPAD, 4, 0, 0]), 0);
    assert_eq!(w.u32(LAST_ERROR), 87);

    // Utility drive: argument 1 is the format flag. The mount also selects
    // the cache-database row of the title's entry; the kernel seeds that
    // entry (Partition5) in row 0, so the row is 0, not partition - 3.
    assert_eq!(w.u32(0x0046_1d80), 0xffff_ffff);
    assert_eq!(w.returns(0x002d_5b77, &[1]), 1);
    assert_eq!(w.u32(0x0046_1d80), 0);
    assert_eq!(w.returns(0x002d_5c7c, &[]), 1);
    assert_eq!(
        w.xapi.lock().unwrap().storage.calls,
        ["mount 5 true", "format"]
    );

    // Power-down pair: S0, void, no side effects.
    w.returns(0x002d_6031, &[]);
    w.returns(0x002d_6004, &[]);

    // XReadMUMetaData: five words, ERROR_DEVICE_NOT_CONNECTED.
    assert_eq!(w.returns(0x0041_136f, &[1, 2, 3, 4, 5]), 1167);
    assert_eq!(
        w.u32(LAST_ERROR),
        87,
        "status-returning APIs keep last error"
    );
}

#[test]
fn launch_stops_with_the_frame_intact() {
    let mut w = world();
    w.put(SCRATCH, b"D:\\default.xbe\0");
    let esp = STACK_TOP - 0x100;
    let t = w.call(0x002d_07f2, &[SCRATCH, 0]);
    assert_eq!(t.kind, abi::STOP);
    let Some(Event::Unsupported {
        pc,
        mnemonic,
        detail,
        ..
    }) = w.ctx.event(t.event_id)
    else {
        panic!("{:?}", w.ctx.event(t.event_id));
    };
    assert_eq!(*pc, 0x002d_07f2);
    assert_eq!(mnemonic, "xapi::XLaunchNewImageA");
    assert!(detail.starts_with("hle:"));
    assert_eq!(
        (w.cpu.esp, w.cpu.eip, w.cpu.eax),
        (esp, 0x002d_07f2, 0xdead_beef)
    );
    assert_eq!(w.stack(), w.before, "stack changed under the stop");
    let request = w.xapi.lock().unwrap().take_launch_request().unwrap();
    assert_eq!(request.path.as_deref(), Some("D:\\default.xbe"));
    assert_eq!(request.data, None);
}

#[test]
fn guest_memory_faults_surface_as_runtime_faults() {
    let mut w = world();
    // XInputGetState into unmapped memory, through a valid open handle.
    w.returns(0x0040_8c57, &[0, 0]);
    let handle = w.returns(0x0040_9932, &[GAMEPAD, 0, 0, 0]);
    let t = w.call(0x0040_9b6c, &[handle, 0x0900_0000]);
    assert_eq!(t.kind, abi::FAULT);
    let Some(Event::MemoryFault { fault, .. }) = w.ctx.event(t.event_id) else {
        panic!("{:?}", w.ctx.event(t.event_id));
    };
    assert_eq!(fault.addr, 0x0900_0000);
    assert_eq!(fault.reason, runtime::guest::FaultReason::Unmapped);

    // A thread without a mapped TLS block faults before the service runs.
    w.cpu.fs_base = 0x0a00_0000;
    let t = w.call(0x002d_5b77, &[0]);
    assert_eq!(t.kind, abi::FAULT);
    assert!(w.xapi.lock().unwrap().storage.calls.is_empty());
}

#[test]
fn cache_row_follows_the_layout() {
    let mut d = Dispatch::default();
    let x = xapi();
    let layout = xapi::GuestLayout {
        cache_row: Some(xapi::CacheRow {
            global: 0x0046_1d80,
            row: 3,
        }),
        ..GuestLayout::default()
    };
    register(&mut d, &x, layout).unwrap();
    let mut w = world_with(d, x);
    assert_eq!(w.returns(0x002d_5b77, &[0]), 1);
    assert_eq!(w.u32(0x0046_1d80), 3);
}

#[test]
fn scripted_campaign_controls_cross_registered_state_capability_and_feedback_entries() {
    use input::backend::scripted::Timeline;
    use input::gamepad::Gamepad;

    let script = Timeline::from_toml(
        "[[frame]]\nat_ticks=0\n[[frame]]\nat_ticks=10\nbuttons=['a','start']\nleft_stick=[0,32767]\nright_stick=[0,-32768]\n[[frame]]\nat_ticks=20",
    ).unwrap().playback();
    let mut input = InputManager::new(Default::default()).unwrap();
    input.add_backend(Box::new(script.clone()));
    let xapi = Arc::new(Mutex::new(
        Xapi::new(Config::for_gamepad(GAMEPAD), input, Utility::default()).unwrap(),
    ));
    let mut dispatch = Dispatch::default();
    register(&mut dispatch, &xapi, GuestLayout::default()).unwrap();
    let mut w = world_with(dispatch, xapi);
    w.returns(0x408c57, &[0, 0]);
    let h = w.returns(0x409932, &[GAMEPAD, 0, 0, 0]);
    assert_eq!(w.returns(0x409994, &[h, SCRATCH]), 0);
    let mut caps = [0; 25];
    w.runtime
        .memory()
        .read_bytes(SCRATCH, &mut caps, 1)
        .unwrap();
    assert_eq!(
        &caps[3..13],
        &[255, 0, 255, 255, 255, 255, 255, 255, 255, 255]
    );
    let mut events = KernelLike {
        runtime: Arc::clone(&w.runtime),
        calls: vec![],
    };
    script.advance_to(10).unwrap();
    service_input(&w.runtime, &w.xapi, &mut events).unwrap();
    // Actual Halo input boundary at 0x124c36 -> 0x409b6c.
    assert_eq!(w.returns(0x409b6c, &[h, SCRATCH]), 0);
    assert_eq!(w.u32(SCRATCH), 1);
    let mut bytes = [0; 18];
    w.runtime
        .memory()
        .read_bytes(SCRATCH + 4, &mut bytes, 1)
        .unwrap();
    let pad = Gamepad::from_bytes(&bytes);
    assert_eq!(pad.buttons.0, 16);
    assert_eq!(pad.analog[0], 255);
    assert_eq!((pad.thumb_ly, pad.thumb_ry), (32767, -32768));
    w.put(
        SCRATCH + 0x100,
        &Feedback {
            status: 42,
            h_event: 0x55,
            rumble: Rumble {
                left_motor: 65535,
                right_motor: 123,
            },
        }
        .to_bytes(),
    );
    assert_eq!(w.returns(0x409bdf, &[h, SCRATCH + 0x100]), 997);
    service_input(&w.runtime, &w.xapi, &mut events).unwrap();
    assert_eq!(w.u32(SCRATCH + 0x100), 0);
    assert_eq!(events.calls, [0x55]);
    assert_eq!(script.rumble().right_motor, 123);
    script.advance_to(20).unwrap();
    service_input(&w.runtime, &w.xapi, &mut events).unwrap();
    assert_eq!(w.returns(0x409b6c, &[h, SCRATCH]), 0);
    assert_eq!(w.u32(SCRATCH), 2);
    w.runtime
        .memory()
        .read_bytes(SCRATCH + 4, &mut bytes, 1)
        .unwrap();
    assert_eq!(Gamepad::from_bytes(&bytes), Gamepad::default());
}
