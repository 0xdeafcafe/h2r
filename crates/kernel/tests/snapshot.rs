//! Machine snapshots, phase 3 (docs/runtime/snapshots.md): a synthetic
//! multi-threaded guest runs N slices and is saved (guest RAM, TSC, kernel
//! core, threads); the original runs M more. A second machine booted the
//! same way loads the snapshot and runs M; the two are compared slice by
//! slice.
mod common;
use common::*;
use kernel::snapshot::{Rebuilders, SnapshotError};
use kernel::{Machine, RunOutcome};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};
use snapshot::{BuildKey, Header, Snapshot, SnapshotWriter, guest};
use std::hash::{Hash, Hasher};

const EVENT: u32 = DATA + 0x40;
const COUNTER: u32 = DATA + 0x80;
const MIX_A: u32 = DATA + 0x84;
const MIX_B: u32 = DATA + 0x88;
const ROUNDS_B: u32 = DATA + 0x8c;
const ONE_MS: u32 = DATA + 0x120;
const THREE_MS: u32 = DATA + 0x128;
/// Thread B returns from its start routine after this many rounds, so its
/// rebuilt return continuation runs after a load.
const B_ROUNDS: u32 = 12;

fn build() -> BuildKey {
    BuildKey {
        stamp: "synthetic kernel machine".into(),
        switches: String::new(),
    }
}

fn call(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    ordinal: u32,
    args: &[u32],
    then: u32,
) -> Result<TransferAbi, GuestFault> {
    guest_call(
        cpu,
        ctx,
        runtime::kernel::function_token(ordinal),
        args,
        then,
    )
}

fn create_thread(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    start: u32,
    then: u32,
) -> Result<TransferAbi, GuestFault> {
    // PsCreateSystemThreadEx: start routine `start`, its context 0.
    call(
        cpu,
        ctx,
        255,
        &[DATA, 0, 0x4000, 0, DATA + 4, start, 0, 0, 0, 0],
        then,
    )
}

/// Boot a machine: three threads that share an auto-reset event, sleep on
/// different periods and mix what they see into guest memory. Every
/// override is stateless: the state is in guest memory and the CPUs.
fn boot(epoch: u64) -> (Harness, Machine) {
    let h = Harness::config(kernel::Config {
        epoch_100ns: epoch,
        ..kernel::Config::default()
    });
    let mut m = h.runtime.memory();
    m.set_u64(ONE_MS, (-10_000i64) as u64).unwrap();
    m.set_u64(THREE_MS, (-30_000i64) as u64).unwrap();
    drop(m);
    // Thread 1: NtCreateEvent(synchronization), two threads, then forever
    // count, signal and sleep 1 ms.
    h.code(0, |cpu, ctx, _| {
        call(cpu, ctx, 189, &[EVENT, 0, 1, 0], CODE + 0x10)
    });
    h.code(0x10, |cpu, ctx, _| {
        create_thread(cpu, ctx, CODE + 0x200, CODE + 0x20)
    });
    h.code(0x20, |cpu, ctx, _| {
        create_thread(cpu, ctx, CODE + 0x300, CODE + 0x30)
    });
    h.code(0x30, |cpu, ctx, _| {
        let event = {
            let mut m = ctx.memory();
            let n = m.u32(COUNTER)? + 1;
            m.set_u32(COUNTER, n)?;
            m.u32(EVENT)?
        };
        // NtSetEvent
        call(cpu, ctx, 225, &[event, 0], CODE + 0x40)
    });
    h.code(0x40, |cpu, ctx, _| {
        call(cpu, ctx, 99, &[0, 0, ONE_MS], CODE + 0x30)
    });
    // Thread A: wait for the event, then mix the counter and the tick count
    // the kernel publishes.
    h.code(0x200, |cpu, ctx, _| {
        let event = ctx.memory().u32(EVENT)?;
        call(cpu, ctx, 233, &[event, 0, 0], CODE + 0x210)
    });
    h.code(0x210, |_, ctx, _| {
        let mut m = ctx.memory();
        let v = m.u32(MIX_A)?.rotate_left(5) ^ m.u32(COUNTER)?;
        m.set_u32(MIX_A, v.wrapping_mul(0x9e37_79b1))?;
        Ok(TransferAbi::jump(CODE + 0x200))
    });
    // Thread B: sleep 3 ms, mix, and after B_ROUNDS return from the start
    // routine into the kernel's thread-return continuation.
    h.code(0x300, |cpu, ctx, _| {
        call(cpu, ctx, 99, &[0, 0, THREE_MS], CODE + 0x310)
    });
    h.code(0x310, |cpu, ctx, _| {
        let rounds = {
            let mut m = ctx.memory();
            let v = m.u32(MIX_B)?.wrapping_mul(31) ^ m.u32(COUNTER)?;
            m.set_u32(MIX_B, v)?;
            let rounds = m.u32(ROUNDS_B)? + 1;
            m.set_u32(ROUNDS_B, rounds)?;
            rounds
        };
        if rounds == B_ROUNDS {
            cpu.eax = 0;
            return ctx.memory().ret(cpu, 4);
        }
        Ok(TransferAbi::jump(CODE + 0x300))
    });
    let machine = Machine::new(
        h.runtime.clone(),
        h.kernel.clone(),
        h.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    (h, machine)
}

/// What one slice left behind: every mapped guest byte, the clock, the
/// threads' CPUs and which thread ran.
fn digest(h: &Harness, machine: &Machine) -> u64 {
    let mut s = std::collections::hash_map::DefaultHasher::new();
    let memory = h.runtime.memory();
    let ram = memory.physical_ram();
    for map in memory.layout().mappings {
        let start = map.physical_start as usize;
        if start < ram.len() {
            map.virtual_start.hash(&mut s);
            ram[start..(start + map.len as usize).min(ram.len())].hash(&mut s);
        }
    }
    drop(memory);
    h.kernel.clock().counter().hash(&mut s);
    machine.schedule().last().hash(&mut s);
    for t in machine.threads() {
        (t.id, format!("{:?}", t.cpu)).hash(&mut s);
    }
    s.finish()
}

fn run(h: &Harness, machine: &mut Machine, slices: usize) -> Vec<u64> {
    (0..slices)
        .map(|_| {
            assert_eq!(machine.run(1).unwrap(), RunOutcome::SliceLimit);
            digest(h, machine)
        })
        .collect()
}

fn save(h: &Harness, machine: &Machine) -> Result<Vec<u8>, SnapshotError> {
    let mut w = SnapshotWriter::new(Header {
        build: build(),
        name: "fight".into(),
        tick: 0,
        presents: 0,
        notes: Vec::new(),
    });
    guest::save_guest(&mut w, &h.runtime.memory(), None)?;
    guest::save_tsc(&mut w, &h.runtime)?;
    machine.save_snapshot(&mut w)?;
    Ok(w.finish()?)
}

fn load(h: &Harness, machine: &mut Machine, bytes: &[u8]) -> Result<(), SnapshotError> {
    let snap = Snapshot::open(bytes, &build())?;
    guest::load_guest(&snap, &mut h.runtime.memory(), None)?;
    guest::load_tsc(&snap, &h.runtime)?;
    machine.load_snapshot(&snap, &Rebuilders::new())
}

/// Threads whose start routine has not returned: their rebuildable
/// thread-return continuations.
fn live_returns(machine: &Machine) -> Vec<u32> {
    machine
        .quiescent()
        .rebuildable
        .iter()
        .filter(|(_, c)| c.origin == kernel::quiesce::THREAD_RETURN)
        .map(|(thread, _)| *thread)
        .collect()
}

const N: usize = 9;
const M: usize = 120;

#[test]
fn a_loaded_machine_runs_the_same_slices_as_the_saved_one() {
    let (a, mut machine_a) = boot(0);
    run(&a, &mut machine_a, N);
    // The save point has a created thread whose start routine has not
    // returned yet (B), so its return continuation is live and rebuildable.
    assert!(machine_a.quiescent().is_quiescent());
    assert_eq!(live_returns(&machine_a), vec![2, 3]);
    assert!(a.get(ROUNDS_B) < B_ROUNDS);
    let bytes = save(&a, &machine_a).unwrap();
    let expected = run(&a, &mut machine_a, M);
    // Thread B returned during the M slices, through its continuation.
    assert_eq!(a.get(ROUNDS_B), B_ROUNDS);

    let (b, mut machine_b) = boot(0);
    // A fresh machine has run nothing: it is not where A was.
    assert_ne!(digest(&b, &machine_b), digest(&a, &machine_a));
    load(&b, &mut machine_b, &bytes).unwrap();
    let got = run(&b, &mut machine_b, M);
    for (slice, (want, got)) in expected.iter().zip(&got).enumerate() {
        assert_eq!(want, got, "slice {slice} after the load diverges");
    }
    assert_eq!(
        a.runtime.memory().physical_ram(),
        b.runtime.memory().physical_ram()
    );
    assert_eq!(machine_a.schedule(), machine_b.schedule());
    assert_eq!(machine_a.preemptions(), machine_b.preemptions());
    assert_eq!(a.kernel.clock(), b.kernel.clock());
    assert_eq!(a.runtime.tsc_last(), b.runtime.tsc_last());
    // Thread B (3) ended on both, through the rebuilt continuation on the
    // loaded side; thread A (2) never returns, so its own is still live.
    for m in [&machine_a, &machine_b] {
        assert_eq!(live_returns(m), vec![2]);
    }
}

#[test]
fn the_same_snapshot_loads_twice_into_the_same_machine() {
    let (a, mut machine_a) = boot(0);
    run(&a, &mut machine_a, N);
    let bytes = save(&a, &machine_a).unwrap();
    let (b, mut machine_b) = boot(0);
    load(&b, &mut machine_b, &bytes).unwrap();
    let first = run(&b, &mut machine_b, 40);
    load(&b, &mut machine_b, &bytes).unwrap();
    assert_eq!(run(&b, &mut machine_b, 40), first);
}

#[test]
fn a_differently_booted_machine_refuses_the_kernel_sections() {
    let (a, mut machine_a) = boot(0);
    run(&a, &mut machine_a, N);
    let bytes = save(&a, &machine_a).unwrap();
    // A different data-export table: the kernel core's boot check fails.
    let mut config = kernel::Config::default();
    config.data_exports.insert(156, DATA + 0x400);
    let other = Harness::config(config);
    let mut machine = Machine::new(
        other.runtime.clone(),
        other.kernel.clone(),
        other.cpu,
        STACK_BOTTOM,
        STACK_TOP,
    )
    .unwrap();
    let snap = Snapshot::open(&bytes, &build()).unwrap();
    let err = machine.load_snapshot(&snap, &Rebuilders::new()).unwrap_err();
    assert!(
        matches!(&err, SnapshotError::Mismatch(m) if m.contains("data exports")),
        "{err}"
    );
}

#[test]
fn a_live_closure_continuation_refuses_the_save() {
    let (h, mut machine) = boot(0);
    // Thread 1 starts inside an override's guest callback that sleeps.
    h.code(0, |cpu, ctx, _| {
        let rt = std::sync::Arc::clone(ctx.runtime());
        ctx.call_guest_named(
            cpu,
            &mut rt.memory(),
            CODE + 0x400,
            "test: override callback",
            |cpu, ctx| ctx.memory().ret(cpu, 0),
        )
    });
    h.code(0x400, |cpu, ctx, _| {
        call(cpu, ctx, 99, &[0, 0, ONE_MS], CODE + 0x410)
    });
    h.code(0x410, |cpu, ctx, _| ctx.memory().ret(cpu, 0));
    machine.run(1).unwrap();
    let err = save(&h, &machine).unwrap_err();
    assert!(
        matches!(&err, SnapshotError::Busy(b) if b[0].contains("test: override callback")),
        "{err}"
    );
}
