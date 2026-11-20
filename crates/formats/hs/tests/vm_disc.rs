//! Opt-in: run every disc scenario's scripts on the native VM against the
//! recording mock engine, and print builtin coverage.
//!
//! ```sh
//! H2_DISC=disc/halo2.iso cargo test -p hs --release --test vm_disc -- --ignored --nocapture
//! ```
//!
//! `H2_DISC` is your own disc image (maps and `default.xbe` are read from
//! it). `H2_XBE` optionally overrides the executable, `H2_HS_TICKS` the
//! ticks per phase (default 3000, 100 s at 30 Hz), `H2_HS_MAP` limits the run
//! to map paths containing the value. Nothing read here is written anywhere;
//! the report goes to stdout only.
use hs::vm::mock::{Coverage, Policy, RecordingEngine};
use hs::vm::{Category, Vm, VmImage};
use std::{fs::File, io::BufReader};

#[test]
#[ignore = "needs H2_DISC pointing at your own disc image"]
fn disc_scripts_run_on_the_vm_without_faults() {
    let path = std::env::var_os("H2_DISC").expect("set H2_DISC to your disc image");
    let ticks: i32 = std::env::var("H2_HS_TICKS")
        .ok()
        .map_or(3000, |t| t.parse().expect("H2_HS_TICKS is a tick count"));
    let only = std::env::var("H2_HS_MAP").ok();
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let xbe_bytes = match std::env::var_os("H2_XBE") {
        Some(p) => std::fs::read(p).unwrap(),
        None => {
            let e = iso.find("default.xbe").unwrap();
            iso.read(&e).unwrap()
        }
    };
    let xbe = xbe::Xbe::parse(&xbe_bytes).unwrap();
    let mut catalog = hs::Catalog::from_xbe(&xbe).unwrap();
    let mut scenarios = Vec::new();
    for e in iso
        .entries()
        .unwrap()
        .iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
        .filter(|e| only.as_ref().is_none_or(|m| e.path.contains(m.as_str())))
    {
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let mut map = map::Map::from_region(
            BufReader::new(File::open(&path).unwrap()),
            base,
            e.size.into(),
        )
        .unwrap();
        let s = hs::Scenario::read(&mut map).unwrap();
        catalog.observe(&s).unwrap();
        scenarios.push((e.path.clone(), s));
    }
    let image = VmImage::from_xbe(&xbe, &catalog).unwrap();
    println!(
        "{} maps; {} of {} builtins are retail-discarded stubs",
        scenarios.len(),
        image.discarded().len(),
        catalog.functions().len()
    );
    let mut coverage = Coverage::default();
    let mut faults = 0;
    for (path, s) in &scenarios {
        let reached = s.reached_builtins(s.roots()).unwrap();
        for policy in [Policy::Neutral, Policy::Truthy, Policy::Random(0x2009_1109)] {
            let mut engine = RecordingEngine::new(policy);
            let mut vm = Vm::new(s, &catalog, &image);
            vm.initialize(&mut engine).unwrap();
            // Phase 1: the scheduler runs startup, dormant and continuous
            // threads as the game would.
            for tick in 0..ticks {
                engine.time = tick;
                vm.update(&mut engine);
            }
            // Phase 2: call every static and stub script on its own
            // transient thread, as a console call would. Calls that sleep
            // run under the scheduler in batches; any still waiting after a
            // batch's ticks are stopped, to stay inside the thread pool.
            let mut time = ticks;
            let mut pending = Vec::new();
            let statics: Vec<u16> = (0..s.scripts().len() as u16)
                .filter(|&i| matches!(s.scripts()[i as usize].kind(), 3 | 4))
                .collect();
            for (n, &i) in statics.iter().enumerate() {
                match vm.call_script(&mut engine, i) {
                    Ok(Err(slot)) => pending.push(slot),
                    Ok(Ok(_)) => {}
                    Err(error) => {
                        let name = s.scripts()[i as usize].name().unwrap_or("?");
                        println!("{path}: static {name}: {error}");
                        faults += 1;
                    }
                }
                if pending.len() == 16 || (n + 1 == statics.len() && !pending.is_empty()) {
                    for _ in 0..300 {
                        engine.time = time;
                        time += 1;
                        vm.update(&mut engine);
                    }
                    for slot in pending.drain(..) {
                        if vm
                            .thread(slot)
                            .is_some_and(|t| t.category() == Category::Transient)
                        {
                            vm.stop_thread(slot);
                        }
                    }
                }
            }
            // Phase 3: wake every dormant script so its body runs too.
            for (i, script) in s.scripts().iter().enumerate() {
                if script.kind() == 1 {
                    vm.wake_script(i as u16).unwrap();
                }
            }
            for tick in time..time + ticks {
                engine.time = tick;
                vm.update(&mut engine);
            }
            let stats = vm.stats();
            println!(
                "{path} {policy:?}: threads={} executions={} dispatches={} engine-calls={} faults={}",
                vm.threads().count(),
                stats.thread_executions,
                stats.dispatches,
                engine.counts.values().sum::<u64>(),
                vm.faults().len()
            );
            for f in vm.faults().iter().take(10) {
                let script = f
                    .script
                    .and_then(|i| s.scripts().get(i as usize))
                    .and_then(|s| s.name().ok())
                    .unwrap_or("-");
                println!(
                    "  fault in {script} at {:#010x}: {}",
                    f.expression.0, f.error
                );
            }
            faults += vm.faults().len();
            coverage.add(&vm, &reached);
        }
    }
    println!("{}", coverage.report(&catalog));
    assert_eq!(faults, 0, "VM faults on disc scripts");
}
