//! Pending audio wakes an otherwise idle machine: a thread waits on a PCM
//! packet's completion event with nothing else scheduled, and the boot
//! moves logical time on to the audio deadline (`dsound_glue::wake_idle`)
//! instead of reporting a deadlock.
//!
//! The DirectSound install reads the WMA tables from the retail image:
//! `H2_DISC=/path/halo2.iso` (or `H2_XBE=/path/default.xbe`)
//! `cargo test -p boot --test audio_idle -- --ignored`

#[path = "../../../runtime/kernel/tests/common/mod.rs"]
mod common;
use common::*;
use runtime::{CpuAbi, Event, ExecCtx, Runner};

fn ds_call(h: &Harness, addr: u32, args: &[u32]) {
    let mut cpu = CpuAbi { esp: STACK_TOP - 0x100, ..CpuAbi::xbox_thread() };
    {
        let mut m = h.runtime.memory();
        m.set_u32(cpu.esp, runtime::exec::HARNESS_EXIT).unwrap();
        for (i, a) in args.iter().enumerate() {
            m.set_u32(cpu.esp + 4 + i as u32 * 4, *a).unwrap();
        }
    }
    let mut ctx = ExecCtx::new(h.runtime.clone(), 1);
    let t = Runner::default().run(&mut cpu, &mut ctx, addr);
    assert_eq!(ctx.event(t.event_id), Some(&Event::HarnessExit));
    assert_eq!(cpu.eax, 0);
}

#[test]
#[ignore = "needs the retail image in $H2_DISC or $H2_XBE"]
fn pending_audio_wakes_an_idle_machine() {
    let path = std::env::var_os("H2_XBE").or_else(|| std::env::var_os("H2_DISC")).expect("set H2_DISC or H2_XBE");
    let image = boot::read_xbe(std::path::Path::new(&path)).unwrap();
    let mut h = Harness::with_guest(kernel::Config::default(), |g| {
        g.map_zeroed(0x0037_9000, 0xf000, runtime::guest::Permissions::RX).unwrap();
    });
    let audio = boot::dsound_glue::install(&h.runtime, &h.kernel, &image).unwrap();
    h.success(189, &[DATA, 0, 0, 0]); // NtCreateEvent
    let event = h.get(DATA);
    // Mono 48 kHz PCM16.
    let fmt = DATA + 0x100;
    let mut b = Vec::new();
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(48000u32.to_le_bytes());
    b.extend(96000u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(0u16.to_le_bytes());
    h.bytes(fmt, &b);
    let desc = DATA + 0x200;
    for (i, w) in [0, 4, fmt, 0, 0, 0].iter().enumerate() {
        h.set(desc + i as u32 * 4, *w);
    }
    ds_call(&h, 0x0037_d835, &[desc, DATA + 4, 0]); // DirectSoundCreateStream
    let stream = h.get(DATA + 4);
    let (packet, status) = (DATA + 0x300, DATA + 0x400);
    for (i, w) in [DATA + 0x1000, 512, status + 4, status, event, 0].iter().enumerate() {
        h.set(packet + i as u32 * 4, *w);
    }
    ds_call(&h, 0x0037_ad25, &[stream, packet, 0]); // Process: 256 frames
    assert_eq!(h.get(status), dsound::abi::XMEDIAPACKET_STATUS_PENDING);
    // The only thread waits on the packet's event (KeWaitForSingleObject).
    h.code(0, move |cpu, ctx, _| guest_call(cpu, ctx, runtime::kernel::function_token(233), &[event, 0, 0], CODE + 0x100));
    h.code(0x100, |cpu, ctx, _| {
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.cpu = CpuAbi { eip: CODE, esp: STACK_TOP - 4, ..CpuAbi::xbox_thread() };
    h.set(h.cpu.esp, runtime::exec::HARNESS_EXIT);
    let mut m = kernel::Machine::new(h.runtime.clone(), h.kernel.clone(), h.cpu, STACK_BOTTOM, STACK_TOP).unwrap();
    let mut idle = 0;
    let mut outcome = kernel::RunOutcome::SliceLimit;
    for _ in 0..100 {
        outcome = m.run(1).unwrap();
        if matches!(outcome, kernel::RunOutcome::Deadlock { .. })
            && boot::dsound_glue::wake_idle(&audio, &h.kernel, &h.runtime, &mut idle).unwrap()
        {
            continue;
        }
        if !matches!(outcome, kernel::RunOutcome::SliceLimit) {
            break;
        }
        boot::dsound_glue::advance(&audio, &h.kernel).unwrap();
    }
    audio.output.halt();
    assert!(matches!(outcome, kernel::RunOutcome::Complete { .. }), "{outcome:?}");
    assert_eq!(h.get(status), dsound::abi::XMEDIAPACKET_STATUS_SUCCESS);
    assert!(idle > 0 && idle <= 4 * boot::dsound_glue::QUANTUM_TICKS, "logical time added: {idle}");
}

/// A deadlock with no audio pending is still reported.
#[test]
#[ignore = "needs the retail image in $H2_DISC or $H2_XBE"]
fn idle_without_audio_is_still_a_deadlock() {
    let path = std::env::var_os("H2_XBE").or_else(|| std::env::var_os("H2_DISC")).expect("set H2_DISC or H2_XBE");
    let image = boot::read_xbe(std::path::Path::new(&path)).unwrap();
    let mut h = Harness::with_guest(kernel::Config::default(), |g| {
        g.map_zeroed(0x0037_9000, 0xf000, runtime::guest::Permissions::RX).unwrap();
    });
    let audio = boot::dsound_glue::install(&h.runtime, &h.kernel, &image).unwrap();
    h.success(189, &[DATA, 0, 0, 0]);
    let event = h.get(DATA);
    h.code(0, move |cpu, ctx, _| guest_call(cpu, ctx, runtime::kernel::function_token(233), &[event, 0, 0], CODE + 0x100));
    h.cpu = CpuAbi { eip: CODE, esp: STACK_TOP - 4, ..CpuAbi::xbox_thread() };
    let mut m = kernel::Machine::new(h.runtime.clone(), h.kernel.clone(), h.cpu, STACK_BOTTOM, STACK_TOP).unwrap();
    let mut idle = 0;
    let outcome = m.run(10).unwrap();
    assert!(matches!(outcome, kernel::RunOutcome::Deadlock { .. }), "{outcome:?}");
    assert!(!boot::dsound_glue::wake_idle(&audio, &h.kernel, &h.runtime, &mut idle).unwrap());
    audio.output.halt();
}

/// A paused stream keeps audio pending for ever. The runner's 60 Hz vblank
/// source makes slices that end in SliceLimit without running a guest
/// thread; they must not refill the budget, so the run still ends in
/// Deadlock about 10 s of logical time in (the loop mirrors `boot::run`).
#[test]
#[ignore = "needs the retail image in $H2_DISC or $H2_XBE"]
fn vblank_does_not_refill_the_audio_idle_budget() {
    let path = std::env::var_os("H2_XBE").or_else(|| std::env::var_os("H2_DISC")).expect("set H2_DISC or H2_XBE");
    let image = boot::read_xbe(std::path::Path::new(&path)).unwrap();
    let mut h = Harness::with_guest(kernel::Config::default(), |g| {
        g.map_zeroed(0x0037_9000, 0xf000, runtime::guest::Permissions::RX).unwrap();
    });
    let audio = boot::dsound_glue::install(&h.runtime, &h.kernel, &image).unwrap();
    h.success(189, &[DATA, 0, 0, 0]);
    let event = h.get(DATA);
    let fmt = DATA + 0x100;
    let mut b = Vec::new();
    b.extend(1u16.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(48000u32.to_le_bytes());
    b.extend(96000u32.to_le_bytes());
    b.extend(2u16.to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(0u16.to_le_bytes());
    h.bytes(fmt, &b);
    let desc = DATA + 0x200;
    for (i, w) in [0, 4, fmt, 0, 0, 0].iter().enumerate() {
        h.set(desc + i as u32 * 4, *w);
    }
    ds_call(&h, 0x0037_d835, &[desc, DATA + 4, 0]);
    let stream = h.get(DATA + 4);
    let (packet, status) = (DATA + 0x300, DATA + 0x400);
    for (i, w) in [DATA + 0x1000, 512, status + 4, status, event, 0].iter().enumerate() {
        h.set(packet + i as u32 * 4, *w);
    }
    ds_call(&h, 0x0037_ad25, &[stream, packet, 0]);
    ds_call(&h, 0x0037_b822, &[stream, 1]); // Pause: the packet never finishes.
    h.kernel
        .add_periodic(kernel::periodic::Source {
            name: "vblank",
            rate: kernel::periodic::Rate::HZ_60,
            max_catch_up: 4,
            handler: Box::new(|_, _, _| Ok(None)),
        })
        .unwrap();
    h.code(0, move |cpu, ctx, _| guest_call(cpu, ctx, runtime::kernel::function_token(233), &[event, 0, 0], CODE + 0x100));
    h.code(0x100, |cpu, ctx, _| {
        cpu.eax = 0;
        ctx.memory().ret(cpu, 0)
    });
    h.cpu = CpuAbi { eip: CODE, esp: STACK_TOP - 4, ..CpuAbi::xbox_thread() };
    h.set(h.cpu.esp, runtime::exec::HARNESS_EXIT);
    let mut m = kernel::Machine::new(h.runtime.clone(), h.kernel.clone(), h.cpu, STACK_BOTTOM, STACK_TOP).unwrap();
    let mut idle = 0;
    let mut outcome = kernel::RunOutcome::SliceLimit;
    for _ in 0..5000 {
        let before = boot::dsound_glue::guest_slices(&m);
        outcome = m.run(1).unwrap();
        if matches!(outcome, kernel::RunOutcome::Deadlock { .. })
            && boot::dsound_glue::wake_idle(&audio, &h.kernel, &h.runtime, &mut idle).unwrap()
        {
            continue;
        }
        if !matches!(outcome, kernel::RunOutcome::SliceLimit) {
            break;
        }
        if boot::dsound_glue::guest_slices(&m) != before {
            idle = 0;
        }
        boot::dsound_glue::advance(&audio, &h.kernel).unwrap();
    }
    audio.output.halt();
    let ticks = h.kernel.clock().counter();
    assert!(matches!(outcome, kernel::RunOutcome::Deadlock { .. }), "{outcome:?} at {ticks} ticks");
    assert!(
        ticks <= boot::dsound_glue::IDLE_LIMIT_TICKS + boot::dsound_glue::QUANTUM_TICKS,
        "the cap was refilled by vblank: {ticks} ticks"
    );
}
