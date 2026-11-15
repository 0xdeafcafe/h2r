//! Vertical-blank delivery through the real runtime: after `Swap`
//! (`crate::vblank::deliver_then_return`) and in the opt-in frame-wait
//! override (`register_frame_wait`). The guest callback is a Rust override
//! standing in for Halo's `0x12b2a0` -> `0x14280`, which increments the
//! 64-bit counter at `0x485AB0`.

mod common;

use common::{H, MISC, PARAMS, PB, PP_DEVICE, SCRATCH, W, synthetic_encode_bytes};
use d3d::abi::SharedDevice;
use d3d::runtime_glue;
use d3d::{DeviceConfig, FrameWait, PresentParameters, RecordingBackend, SparseMemory};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi, abi as rt};

const DEV: u32 = 0x0040_4FE0;
const STACK: u32 = MISC + 0xF000;
/// A stand-in for `0x12b2a0`: logs the record, bumps the counter.
const CALLBACK: u32 = 0x003F_0100;
/// A callback that returns without counting.
const STUCK: u32 = 0x003F_0200;
const LOG: u32 = MISC + 0x8000;
const FW: FrameWait = FrameWait::HALO2;

fn guest(cfg: &DeviceConfig) -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0x003F_0000, 0x1_4000, Permissions::RX)
        .unwrap();
    g.map_zeroed(0x0012_B000, 0x1000, Permissions::RX).unwrap();
    g.map_zeroed(0x0040_4000, 0x5000, Permissions::RW).unwrap();
    g.map_zeroed(0x0048_5000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(0x004E_6000, 0x1000, Permissions::RW).unwrap();
    let encode = d3d::GuestLayout::HALO2.simple_encode;
    g.map_zeroed(encode & !0xFFF, 0x1000, Permissions::RW)
        .unwrap();
    g.write_bytes(encode, &synthetic_encode_bytes(), 1).unwrap();
    g.map_zeroed(MISC, 0x1_0000, Permissions::RW).unwrap();
    runtime_glue::map_device_memory(&mut g, cfg).unwrap();
    let mut params = SparseMemory::new();
    params.map(PARAMS, 0x100);
    PresentParameters {
        width: W,
        height: H,
        back_buffer_format: 0x06,
        back_buffer_count: 0,
        multisample_type: 0,
        swap_effect: 0,
        enable_auto_depth_stencil: true,
        auto_depth_stencil_format: 0x2A,
        flags: 0,
        refresh_rate: 0,
        presentation_interval: 0x8000_0000,
        buffer_surface: 0,
        depth_stencil_surface: 0,
    }
    .write(&mut params, PARAMS)
    .unwrap();
    let mut bytes = vec![0; PresentParameters::SIZE as usize];
    d3d::GuestMemory::read(&params, PARAMS, &mut bytes).unwrap();
    g.write_bytes(PARAMS, &bytes, 1).unwrap();
    g
}

fn callback(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    count: bool,
) -> Result<TransferAbi, runtime::GuestFault> {
    let mut m = ctx.memory();
    let record = m.u32(cpu.esp + 4)?;
    let n = m.u32(LOG)?;
    for i in 0..3 {
        let w = m.u32(record + 4 * i)?;
        m.set_u32(LOG + 4 + 12 * n + 4 * i, w)?;
    }
    m.set_u32(LOG, n + 1)?;
    if count {
        let lo = m.u32(FW.counter)?;
        let hi = m.u32(FW.counter + 4)?;
        let v = ((hi as u64) << 32 | lo as u64) + 1;
        m.set_u32(FW.counter, v as u32)?;
        m.set_u32(FW.counter + 4, (v >> 32) as u32)?;
    }
    // A cdecl callee may clobber the caller-saved registers (and this one
    // clobbers the rest too, to show they are restored).
    cpu.eax = 0xDEAD_0001;
    cpu.ecx = 0xDEAD_0002;
    cpu.edx = 0xDEAD_0003;
    cpu.ebx = 0xDEAD_0004;
    cpu.esi = 0xDEAD_0005;
    m.ret(cpu, 0)
}

struct Harness {
    ctx: ExecCtx,
    device: SharedDevice<RecordingBackend>,
}

fn harness(frame_wait: bool) -> Harness {
    let cfg = DeviceConfig::halo2(PB, SCRATCH);
    let g = guest(&cfg);
    let device = runtime_glue::shared(cfg, RecordingBackend::new());
    let mut dispatch = Dispatch::default();
    d3d::register(&mut dispatch, device.clone());
    if frame_wait {
        d3d::register_frame_wait(
            &mut dispatch,
            device.clone(),
            FW,
            std::sync::Arc::new(runtime_glue::default_stop),
        );
    }
    let _ = dispatch.register_override(CALLBACK, "test vblank callback", |cpu, ctx, _| {
        callback(cpu, ctx, true)
    });
    let _ = dispatch.register_override(STUCK, "test stuck callback", |cpu, ctx, _| {
        callback(cpu, ctx, false)
    });
    Harness {
        ctx: ExecCtx::new(Runtime::new(g, dispatch), 1),
        device,
    }
}

fn saved_regs() -> CpuAbi {
    CpuAbi {
        ebx: 0x1111,
        ebp: 0x2222,
        esi: 0x3333,
        edi: 0x4444,
        ..CpuAbi::default()
    }
}

impl Harness {
    fn call(&mut self, entry: u32, mut cpu: CpuAbi, stack: &[u32]) -> (TransferAbi, CpuAbi) {
        let esp = STACK - 4 - 4 * stack.len() as u32;
        {
            let mut m = self.ctx.memory();
            m.set_u32(esp, HARNESS_EXIT).unwrap();
            for (i, v) in stack.iter().enumerate() {
                m.set_u32(esp + 4 + 4 * i as u32, *v).unwrap();
            }
        }
        cpu.esp = esp;
        let t = Runner::default().run(&mut cpu, &mut self.ctx, entry);
        (t, cpu)
    }

    fn ok(&mut self, entry: u32, cpu: CpuAbi, stack: &[u32]) -> CpuAbi {
        let (t, cpu) = self.call(entry, cpu, stack);
        assert_eq!(t.kind, rt::STOP, "{entry:#x}: {t:?}");
        assert_eq!(
            self.ctx.event(t.event_id),
            Some(&Event::HarnessExit),
            "{entry:#x} did not return: {:?}",
            self.device.lock().unwrap().last_stop()
        );
        assert_eq!(cpu.esp, STACK, "{entry:#x} balances the stack");
        cpu
    }

    fn create(&mut self) {
        let cpu = CpuAbi {
            eax: 0x40,
            ecx: PP_DEVICE,
            ..CpuAbi::default()
        };
        assert_eq!(self.ok(0x3f5240, cpu, &[PARAMS]).eax, 0);
    }

    fn u32(&mut self, addr: u32) -> u32 {
        self.ctx.memory().u32(addr).unwrap()
    }

    fn set(&mut self, addr: u32, v: u32) {
        self.ctx.memory().set_u32(addr, v).unwrap();
    }

    fn set64(&mut self, addr: u32, v: u64) {
        self.set(addr, v as u32);
        self.set(addr + 4, (v >> 32) as u32);
    }

    fn u64(&mut self, addr: u32) -> u64 {
        (self.u32(addr + 4) as u64) << 32 | self.u32(addr) as u64
    }

    fn log(&mut self) -> Vec<[u32; 3]> {
        let n = self.u32(LOG);
        (0..n)
            .map(|i| {
                let b = LOG + 4 + 12 * i;
                [self.u32(b), self.u32(b + 4), self.u32(b + 8)]
            })
            .collect()
    }

    fn unsupported(&self, t: TransferAbi) -> String {
        assert_eq!(t.kind, rt::STOP);
        match self.ctx.event(t.event_id) {
            Some(Event::Unsupported { pc, detail, .. }) => {
                assert_eq!(*pc, FW.entry);
                detail.clone()
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn swap_delivers_its_vblanks_then_returns_with_registers_restored() {
    let mut h = harness(false);
    h.create();
    h.set(DEV + 0x1DB8, CALLBACK);
    h.device.lock().unwrap().set_vblanks_per_swap(2);
    // Swap(0): EAX = flags.
    let cpu = h.ok(0x3f9d00, saved_regs(), &[]);
    assert_eq!(cpu.eax, 1, "Swap's result survives the callbacks");
    assert_eq!(
        (cpu.ebx, cpu.ebp, cpu.esi, cpu.edi),
        (0x1111, 0x2222, 0x3333, 0x4444)
    );
    // {VBlank, Swap, Flags}: SWAPDONE on the first vblank after the swap.
    assert_eq!(
        h.log(),
        vec![[1, 1, d3d::device::VBLANK_SWAPDONE], [2, 1, 0]]
    );
    assert_eq!(h.u64(FW.counter), 2);
    let cpu = h.ok(0x3f9d00, saved_regs(), &[]);
    assert_eq!(cpu.eax, 2);
    assert_eq!(h.log()[2..], [[3, 2, 1], [4, 2, 0]]);
    assert_eq!(h.device.lock().unwrap().stats().vblanks, 4);
}

#[test]
fn swap_without_a_callback_or_vblanks_returns_directly() {
    let mut h = harness(false);
    h.create();
    // Default: no vblanks per swap.
    h.set(DEV + 0x1DB8, CALLBACK);
    h.ok(0x3f9d00, saved_regs(), &[]);
    assert!(h.log().is_empty());
    // Vblanks but no callback: counted, nothing called.
    h.set(DEV + 0x1DB8, 0);
    h.device.lock().unwrap().set_vblanks_per_swap(3);
    h.ok(0x3f9d00, saved_regs(), &[]);
    assert!(h.log().is_empty());
    assert_eq!(h.device.lock().unwrap().stats().vblanks, 3);
}

#[test]
fn frame_wait_delivers_until_the_counter_reaches_the_target() {
    let mut h = harness(true);
    h.create();
    h.set(DEV + 0x1DB8, CALLBACK);
    h.set64(FW.counter, 5);
    h.set64(FW.target, 8);
    h.set(FW.frame_gate, 1);
    let cpu = h.ok(FW.entry, saved_regs(), &[]);
    // Spun from 5 to 7 (= target - 1): two vblanks.
    assert_eq!(h.log().len(), 2);
    assert_eq!(h.u64(FW.counter), 7);
    assert_eq!(h.u64(FW.entry_count), 6, "counter + 1 on entry");
    assert_eq!(h.u64(FW.exit_count), 8, "counter + 1 on exit");
    // d = 8 - 8 = 0: EAX 0, so with the gate word > 0 the flag is set.
    assert_eq!((cpu.eax, cpu.ecx, cpu.edx), (0, 0, 7));
    assert_eq!(h.u32(FW.late_flag) & 0xFF, 1);
    assert_eq!(
        (cpu.ebx, cpu.ebp, cpu.esi, cpu.edi),
        (0x1111, 0x2222, 0x3333, 0x4444)
    );
}

#[test]
fn frame_wait_past_the_target_returns_the_lateness() {
    let mut h = harness(true);
    h.create();
    h.set(DEV + 0x1DB8, CALLBACK);
    h.set(FW.frame_gate, 1);
    h.set(FW.late_flag, 0xFF);
    h.set64(FW.counter, 20);
    h.set64(FW.target, 8);
    let cpu = h.ok(FW.entry, saved_regs(), &[]);
    assert!(h.log().is_empty(), "no spin");
    assert_eq!(h.u64(FW.exit_count), 21);
    assert_eq!((cpu.eax, cpu.ecx, cpu.edx), (13, 0, 20));
    assert_eq!(h.u32(FW.late_flag) & 0xFF, 0, "AX != 0");
    // Clamped to 0x7FFF; ECX is the high word of the difference.
    h.set64(FW.counter, 0x1_0000_0000);
    h.set64(FW.target, 0);
    let cpu = h.ok(FW.entry, saved_regs(), &[]);
    assert_eq!((cpu.eax, cpu.ecx), (0x7FFF, 1));
    // A gate word <= 0 clears the flag even when AX == 0.
    h.set(FW.frame_gate, 0x8000);
    h.set64(FW.counter, 7);
    h.set64(FW.target, 8);
    let cpu = h.ok(FW.entry, saved_regs(), &[]);
    assert_eq!(cpu.eax, 0);
    assert_eq!(h.u32(FW.late_flag) & 0xFF, 0);
}

#[test]
fn frame_wait_stops_instead_of_spinning_forever() {
    // No device, so no callback.
    let mut h = harness(true);
    h.set64(FW.target, 8);
    let (t, _) = h.call(FW.entry, saved_regs(), &[]);
    assert!(h.unsupported(t).contains("spin forever"));
    assert!(h.device.lock().unwrap().last_stop().is_some());
    // A callback that does not count.
    let mut h = harness(true);
    h.create();
    h.set(DEV + 0x1DB8, STUCK);
    h.set64(FW.target, 8);
    let (t, _) = h.call(FW.entry, saved_regs(), &[]);
    assert!(h.unsupported(t).contains("without advancing"));
    assert_eq!(h.log().len(), 1);
    // A target out of reach.
    let mut h = harness(true);
    h.create();
    h.set(DEV + 0x1DB8, CALLBACK);
    h.set64(FW.target, 10_000);
    let (t, _) = h.call(FW.entry, saved_regs(), &[]);
    assert!(h.unsupported(t).contains("600 vblanks"));
    assert_eq!(h.u64(FW.counter), 600);
}

#[test]
fn callbacks_can_reenter_d3d_and_leave_the_caller_stack_alone() {
    let cfg = DeviceConfig::halo2(PB, SCRATCH);
    let g = guest(&cfg);
    let device = runtime_glue::shared(cfg, RecordingBackend::new());
    let mut dispatch = Dispatch::default();
    d3d::register(&mut dispatch, device.clone());
    let callback_device = device.clone();
    let _ = dispatch.register_override(CALLBACK, "reentrant vblank", move |cpu, ctx, _| {
        assert!(
            callback_device.try_lock().is_ok(),
            "device lock held across the callback"
        );
        // A callback that calls into D3D (KickOff) through the overrides.
        let runtime = std::sync::Arc::clone(ctx.runtime());
        let mut m = runtime.memory();
        ctx.call_guest(cpu, &mut m, 0x3fac30, |cpu, ctx| ctx.memory().ret(cpu, 0))
    });
    let mut h = Harness {
        ctx: ExecCtx::new(Runtime::new(g, dispatch), 1),
        device,
    };
    h.create();
    h.set(DEV + 0x1DB8, CALLBACK);
    h.device.lock().unwrap().set_vblanks_per_swap(2);
    h.set(STACK, 0xAABB_CCDD);
    let cpu = h.ok(0x3f9d00, saved_regs(), &[]);
    assert_eq!(cpu.eax, 1);
    assert_eq!(
        h.u32(STACK),
        0xAABB_CCDD,
        "the caller's live stack is untouched"
    );
}
