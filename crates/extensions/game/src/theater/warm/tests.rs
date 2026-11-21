use super::*;
use crate::snapshot_glue::{Host, Shelf};
use crate::theater::{
    Presentation, Replay, Session,
    tests::{arena, host_facts, options},
};
use kernel::{Kernel, Machine, RunOutcome, snapshot::Rebuilders};
use runtime::{CpuAbi, Dispatch, Guest, Runtime, guest::Permissions};
use std::{fs, path::PathBuf};

const CODE: u32 = 0x40_0000;
const COUNTER: u32 = 0x1_0000;
const INTERVAL: u32 = COUNTER + 8;
const STACK: u32 = 0x7000_0000;

struct Fixture {
    dir: PathBuf,
    rt: Arc<Runtime>,
    plan: hooks::Plan,
    rebuilders: Rebuilders,
    shelf: Shelf,
    warm: Warm<u64>,
}

impl Fixture {
    fn new() -> (Self, Machine) {
        let dir = std::env::temp_dir().join(format!("film-ring-{}", live_proto::sign::random_id()));
        fs::create_dir_all(dir.join("disk")).unwrap();
        let disk = film::manifest::Manifest::capture(&dir)
            .unwrap()
            .disk_tracker(&dir);
        let mut guest = Guest::new();
        guest.map_zeroed(CODE, 4096, Permissions::RWX).unwrap();
        guest.map_zeroed(COUNTER, 4096, Permissions::RW).unwrap();
        guest
            .map_zeroed(STACK - 65536, 65536, Permissions::RW)
            .unwrap();
        guest.set_u64(INTERVAL, (-10000i64) as u64).unwrap();
        guest
            .set_u32(STACK - 4, runtime::exec::HARNESS_EXIT)
            .unwrap();
        let rt = Runtime::new(guest, Dispatch::default());
        let kernel = Kernel::install(&rt, Default::default()).unwrap();
        kernel.set_vfs(vfs::Vfs::new()).unwrap();
        rt.edit_dispatch(|dispatch| {
            dispatch.register_override(CODE, "count and sleep", |cpu, ctx, _| {
                let mut guest = ctx.memory();
                let count = guest.u32(COUNTER)?.wrapping_add(1);
                guest.set_u32(COUNTER, count)?;
                // KeDelayExecutionThread(0, 0, &interval), then repeat.
                cpu.esp -= 12;
                guest.set_u32(cpu.esp, 0)?;
                guest.set_u32(cpu.esp + 4, 0)?;
                guest.set_u32(cpu.esp + 8, INTERVAL)?;
                guest.call(cpu, runtime::kernel::function_token(99), CODE)
            });
        });
        let cpu = CpuAbi {
            eip: CODE,
            esp: STACK - 4,
            ..CpuAbi::xbox_thread()
        };
        let machine = Machine::new(rt.clone(), kernel, cpu, STACK - 65536, STACK).unwrap();
        let plan = hooks::Registry::new(hooks::Switches::defaults())
            .build(&hooks::Base)
            .unwrap();
        let shelf = Shelf::new(
            &dir,
            BuildKey {
                stamp: "synthetic".into(),
                switches: String::new(),
            },
        );
        (
            Self {
                dir,
                rt,
                plan,
                rebuilders: Rebuilders::new(),
                shelf,
                warm: Warm::new(96 << 20, disk),
            },
            machine,
        )
    }

    fn barrier(&self, tick: u64) -> SnapshotWriter {
        SnapshotWriter::new(snapshot::Header {
            build: self.shelf.build().clone(),
            name: String::new(),
            tick,
            presents: 0,
            notes: vec![],
        })
    }

    fn capture(&mut self, machine: &mut Machine, tick: u64) {
        let host = host(&self.rt);
        let mut machine = host.machine(machine, &self.plan, &self.rebuilders, &self.shelf, 0);
        let barrier = self.barrier(tick);
        assert!(
            self.warm
                .capture(&mut machine, tick, tick, 8, barrier)
                .unwrap()
        );
    }

    fn restore(&mut self, machine: &mut Machine, tick: u64) -> Result<Option<u64>> {
        let host = host(&self.rt);
        let mut machine = host.machine(machine, &self.plan, &self.rebuilders, &self.shelf, 0);
        self.warm.restore(&mut machine, tick)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn host(rt: &Runtime) -> Host<'_> {
    Host {
        rt,
        d3d: None,
        xapi: None,
        bink: None,
        audio: None,
        live: None,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Slice {
    ram: lockstep::RamHash,
    clock: u64,
    tsc: u64,
    threads: Vec<(u32, CpuAbi)>,
}

fn run(machine: &mut Machine, slices: u32) -> Vec<Slice> {
    (0..slices)
        .map(|_| {
            assert_eq!(machine.run(1).unwrap(), RunOutcome::SliceLimit);
            Slice {
                ram: lockstep::hash_guest(&machine.runtime.memory()),
                clock: machine.kernel.clock().counter(),
                tsc: machine.runtime.tsc_last(),
                threads: machine
                    .threads()
                    .map(|t| (t.id, t.cpu))
                    .collect(),
            }
        })
        .collect()
}

#[test]
fn restoring_a_watched_machine_twice_repeats_every_slice_digest() {
    let (mut fixture, mut machine) = Fixture::new();
    run(&mut machine, 3);
    fixture.capture(&mut machine, 3);
    let mut expected = run(&mut machine, 4);
    fixture.capture(&mut machine, 7);
    expected.extend(run(&mut machine, 4));
    assert!(fixture.rt.memory().u32(COUNTER).unwrap() > 1);
    for _ in 0..2 {
        assert_eq!(fixture.restore(&mut machine, 3).unwrap(), Some(3));
        assert_eq!(run(&mut machine, 8), expected);
    }
}

/// Record the tick stream, play it back, and capture the running machine at
/// tick three. Enable the private ring directly to keep the switch and user
/// settings isolated from other tests in this process.
fn watched_session(fixture: &Fixture, machine: &mut Machine) -> Arc<Session> {
    let guest = arena();
    let opts = options(&fixture.dir);
    let path = fixture.dir.join("synthetic.h2film");
    let record = Session::open(Some(path.clone()), None, host_facts(), b"xbe", &opts)
        .unwrap()
        .unwrap();
    for _ in 0..35 {
        record.checkpoint(&guest);
    }
    record.finish(40, true).unwrap();
    let replay = Session::open(
        None,
        Some(Replay {
            path,
            verification: None,
            render_every: 0,
            speed: 1.0,
            seek_tick: 0,
            paused_at_target: false,
            presentation: Default::default(),
            restart: fixture.dir.join("restart"),
            camera: Default::default(),
            window_closed: None,
        }),
        host_facts(),
        b"xbe",
        &opts,
    )
    .unwrap()
    .unwrap();
    let tracker = film::manifest::Manifest::capture(&fixture.dir)
        .unwrap()
        .disk_tracker(&fixture.dir);
    *replay.warm.lock().unwrap() = Some(Warm::new(96 << 20, tracker));
    for tick in 1..=7 {
        run(machine, 1);
        replay.checkpoint(&guest);
        if tick == 3 {
            let host = host(&fixture.rt);
            let mut boot = host.machine(
                machine,
                &fixture.plan,
                &fixture.rebuilders,
                &fixture.shelf,
                0,
            );
            replay.capture_ring(tick, tick, 0, fixture.barrier(tick), &mut boot, None);
            assert_eq!(replay.lock().ring_status.ticks, vec![3]);
        }
    }
    assert_eq!(replay.lock().tick, 7);
    assert!(replay.lock().failure.is_none());
    replay
}

fn cold_restart(
    replay: &Session,
    fixture: &Fixture,
    boot: &mut crate::snapshot_glue::BootMachine<'_>,
) -> Presentation {
    let target = 5;
    replay.lock().restart = Some(target);
    assert_eq!(replay.try_warm_seek(target, boot, None), None);
    assert_eq!(replay.lock().restart, Some(target));
    replay.write_cold_restart(target).unwrap();
    let (restart_tick, _, _, view): (u64, f64, bool, Presentation) =
        serde_json::from_slice(&fs::read(fixture.dir.join("restart")).unwrap()).unwrap();
    assert_eq!(restart_tick, target);
    view
}

#[test]
fn a_disk_write_after_capture_restarts_cold_before_loading() {
    let (fixture, mut machine) = Fixture::new();
    let replay = watched_session(&fixture, &mut machine);
    let before = lockstep::hash_guest(&fixture.rt.memory());
    fs::write(fixture.dir.join("disk/checkpoint"), b"synthetic checkpoint").unwrap();
    let host = host(&fixture.rt);
    let mut boot = host.machine(
        &mut machine,
        &fixture.plan,
        &fixture.rebuilders,
        &fixture.shelf,
        0,
    );
    assert!(!cold_restart(&replay, &fixture, &mut boot).warm_disabled);
    assert_eq!(lockstep::hash_guest(&fixture.rt.memory()), before);
}

#[test]
fn a_forced_ram_mismatch_restarts_cold_and_disables_warm_for_this_viewing() {
    let (fixture, mut machine) = Fixture::new();
    let replay = watched_session(&fixture, &mut machine);
    let host = host(&fixture.rt);
    let mut machine = host.machine(
        &mut machine,
        &fixture.plan,
        &fixture.rebuilders,
        &fixture.shelf,
        0,
    );
    // Model a device restoring stale host state over a guest page AFTER the
    // pool copied it. Exercise the real whole-RAM hash on both sides.
    struct StaleDevice<'a>(&'a Runtime);
    impl crate::snapshot_glue::Part for StaleDevice<'_> {
        fn name(&self) -> &'static str {
            "d3d"
        }
        fn save(&mut self, _: &mut SnapshotWriter) -> Result<(), String> {
            unreachable!()
        }
        fn load(&mut self, _: &Snapshot<'_>) -> Result<(), String> {
            self.0
                .memory()
                .set_u32(COUNTER, u32::MAX)
                .map_err(|e| e.to_string())
        }
    }
    machine.parts.push(Box::new(StaleDevice(&fixture.rt)));
    assert!(cold_restart(&replay, &fixture, &mut machine).warm_disabled);
    assert_eq!(fixture.rt.memory().u32(COUNTER).unwrap(), u32::MAX);
    assert!(replay.lock().failure.is_none());
}

#[test]
fn divergence_on_the_warm_target_tick_retries_that_jump_cold() {
    let (fixture, mut machine) = Fixture::new();
    let replay = watched_session(&fixture, &mut machine);
    let host = host(&fixture.rt);
    let mut boot = host.machine(
        &mut machine,
        &fixture.plan,
        &fixture.rebuilders,
        &fixture.shelf,
        0,
    );
    let mut guest = arena();
    replay.lock().paused = true;
    replay.lock().restart = Some(30);
    assert_eq!(replay.try_warm_seek(30, &mut boot, None), Some((3, 0)));
    for _ in 4..30 {
        replay.checkpoint(&guest);
    }
    guest.set_u32(0x800000, 17).unwrap();
    replay.checkpoint(&guest);
    // A second callback in the interrupted slice cannot replace the retry.
    replay
        .lock()
        .fail("another callback on the abandoned timeline".into());
    {
        let s = replay.lock();
        assert!(s.failure.is_none());
        assert!(s.paused);
        assert!(s.presentation.warm_disabled);
        assert_eq!(s.restart, Some(30));
    }
    assert!(replay.try_warm_seek(30, &mut boot, None).is_none());
    replay.write_cold_restart(30).unwrap();
    let (target, _, paused, view): (u64, f64, bool, Presentation) =
        serde_json::from_slice(&fs::read(fixture.dir.join("restart")).unwrap()).unwrap();
    assert_eq!(target, 30);
    assert!(paused && view.warm_disabled);
}
