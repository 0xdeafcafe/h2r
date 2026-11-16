//! The installer on a synthetic generated module (no game code): the
//! fixture's leaf (`eax = 41; ret`) at 0x402000 stands in for the hooked
//! guest function, through `Remap`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use engine::points::{self, Point};
use feature::{Flow, InstallError, Registry, Remap, Switches, from_fn};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime, TransferAbi};

const LEAF: u32 = 0x40_2000;
const STACK: u32 = 0x60_0e00;

fn runtime() -> Arc<Runtime> {
    let mut d = Dispatch::default();
    // SAFETY: the fixture's tables, handlers and probes follow its ABI.
    unsafe {
        d.register_generated_module(
            &runtime_test_probe::h2gen_fixture_module,
            runtime_test_probe::h2gen_fixture_module_abi_version,
        )
        .unwrap();
    }
    let mut g = Guest::new();
    g.map_zeroed(0x60_0000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(0x40_0000, 0x5000, Permissions::RX).unwrap();
    Runtime::new(g, d)
}

/// Calls `at` as the game would and returns the CPU after it returned.
fn call(rt: &Arc<Runtime>, at: u32, edi: u32) -> CpuAbi {
    rt.memory().set_u32(STACK, HARNESS_EXIT).unwrap();
    let mut cpu = CpuAbi {
        esp: STACK,
        edi,
        ..Default::default()
    };
    let mut ctx = ExecCtx::new(Arc::clone(rt), 1);
    let t = Runner::default().run(&mut cpu, &mut ctx, at);
    assert!(
        matches!(ctx.event(t.event_id), Some(Event::HarnessExit)),
        "{:?}",
        ctx.event(t.event_id)
    );
    assert_eq!(
        cpu.esp,
        STACK + 4,
        "the caller's stack is as the original leaves it"
    );
    cpu
}

fn at(point: &'static Point) -> Remap {
    Remap::new(&[(point, LEAF)])
}

/// The tick collision: lockstep's hash and Live telemetry's sample used to
/// be two overrides on `game_tick`, and lockstep refused to install when
/// telemetry was there. Both now run, once each per tick, after the
/// original, in a fixed order.
#[test]
fn two_tick_observers_both_run_after_the_tick() {
    let rt = runtime();
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut r = Registry::new(Switches::defaults());
    for id in ["lockstep", "live-telemetry"] {
        let log = Arc::clone(&log);
        r.add(&from_fn(id, move |b| {
            let log = Arc::clone(&log);
            b.frame()
                .on_tick("t", move |o| log.lock().unwrap().push((id, o.cpu.eax)));
            Ok(())
        }))
        .unwrap();
    }
    let plan = r.build(&at(&points::GAME_TICK)).unwrap();
    let installed = plan.install(&rt).unwrap();
    assert_eq!(installed.placed.len(), 2, "the tick and one shadow");
    for _ in 0..2 {
        assert_eq!(
            call(&rt, LEAF, 0).eax,
            41,
            "the original's result comes back"
        );
    }
    assert_eq!(
        *log.lock().unwrap(),
        [
            ("live-telemetry", 41),
            ("lockstep", 41),
            ("live-telemetry", 41),
            ("lockstep", 41)
        ]
    );
}

/// The activation collision (fixed ad hoc in phase 0): the level boot's
/// loaded mark and telemetry's match start both observe a successful
/// activation.
#[test]
fn game_start_reaches_every_listener() {
    let rt = runtime();
    let seen = Arc::new(AtomicU32::new(0));
    let mut r = Registry::new(Switches::defaults());
    for (id, bit) in [("level-boot", 1), ("live-telemetry", 2)] {
        let seen = Arc::clone(&seen);
        r.add(&from_fn(id, move |b| {
            let seen = Arc::clone(&seen);
            b.game().on_start("start", move |_| {
                seen.fetch_or(bit, Ordering::SeqCst);
            });
            Ok(())
        }))
        .unwrap();
    }
    r.build(&at(&points::GAME_ACTIVATE))
        .unwrap()
        .install(&rt)
        .unwrap();
    call(&rt, LEAF, 0);
    assert_eq!(seen.load(Ordering::SeqCst), 3);
}

/// The locale collision: the layouts' string hook answers its own ids,
/// and everything else reaches the Rust lookup (or, without it, the
/// original), where one used to displace the other.
#[test]
fn a_string_hook_and_an_implementation_share_the_locale_lookup() {
    const OURS: u32 = 0x0fff_0001;
    for with_rust in [false, true] {
        let rt = runtime();
        let mut r = Registry::new(Switches::defaults());
        r.add(&from_fn("layouts", |b| {
            b.raw().before(&points::LOCALE_FIND, "strings", |c| {
                if c.arg_u32("id") == Some(OURS) {
                    c.cpu.eax = 0xa2f0_0000;
                    return Ok(Flow::Return(c.ret(0)?));
                }
                Ok(Flow::Continue)
            });
            Ok(())
        }))
        .unwrap();
        if with_rust {
            r.add(&from_fn("rust-locale", |b| {
                b.raw()
                    .implement(&points::LOCALE_FIND, "find", |cpu, ctx, _| {
                        cpu.eax = 7;
                        ctx.memory().ret(cpu, 0)
                    });
                Ok(())
            }))
            .unwrap();
        }
        r.build(&at(&points::LOCALE_FIND))
            .unwrap()
            .install(&rt)
            .unwrap();
        assert_eq!(call(&rt, LEAF, OURS).eax, 0xa2f0_0000, "ours");
        let other = call(&rt, LEAF, 5).eax;
        assert_eq!(other, if with_rust { 7 } else { 41 }, "theirs");
    }
}

#[test]
fn an_after_hook_sees_and_changes_the_result() {
    let rt = runtime();
    let mut r = Registry::new(Switches::defaults());
    r.add(&from_fn("plus-one", |b| {
        b.raw().after(&points::GAME_ACTIVATE, "inc", |c| {
            c.cpu.eax += 1;
            Ok(())
        });
        Ok(())
    }))
    .unwrap();
    r.build(&at(&points::GAME_ACTIVATE))
        .unwrap()
        .install(&rt)
        .unwrap();
    assert_eq!(call(&rt, LEAF, 0).eax, 42);
}

/// Today's silent displacement becomes an error, and nothing is placed.
#[test]
fn an_override_the_plan_did_not_place_is_refused() {
    let rt = runtime();
    rt.edit_dispatch(|d| {
        d.register_override(LEAF, "someone else", |_, _, _| Ok(TransferAbi::stop(0)));
    });
    let mut r = Registry::new(Switches::defaults());
    r.add(&from_fn("lockstep", |b| {
        b.frame().on_tick("t", |_| {});
        Ok(())
    }))
    .unwrap();
    let e = r
        .build(&at(&points::GAME_TICK))
        .unwrap()
        .install(&rt)
        .unwrap_err();
    assert!(
        matches!(&e, InstallError::Occupied { existing, features, .. } if existing == "someone else" && features == &["lockstep"]),
        "{e}"
    );
    rt.edit_dispatch(|d| assert_eq!(d.override_name(LEAF), Some("someone else")));
    assert!(
        rt.memory().u8(feature::arena::ARENA_START).is_err(),
        "nothing mapped"
    );
}

/// A point with no generated original turns a feature off at install (an
/// optional hook is only dropped), and the rest of the plan goes in.
#[test]
fn a_point_without_an_original_turns_its_feature_off() {
    let rt = runtime();
    let mut r = Registry::new(Switches::defaults());
    r.add(&from_fn("needs-both", |b| {
        b.frame().on_tick("t", |_| {});
        b.game().on_start("s", |_| {});
        Ok(())
    }))
    .unwrap();
    r.add(&from_fn("fine", |b| {
        b.game().on_start("s", |_| {});
        b.raw()
            .observe_after(&points::GAME_TICK, "t", |_| {})
            .optional();
        Ok(())
    }))
    .unwrap();
    let addrs = Remap::new(&[
        (&points::GAME_ACTIVATE, LEAF),
        (&points::GAME_TICK, 0x40_4100),
    ]);
    let installed = r.build(&addrs).unwrap().install(&rt).unwrap();
    assert!(
        installed
            .off_reason("needs-both")
            .unwrap()
            .contains("GAME_TICK has no generated original")
    );
    assert!(
        installed
            .notes
            .iter()
            .any(|n| n.starts_with("fine: observe_after t dropped"))
    );
    rt.edit_dispatch(|d| {
        assert_eq!(d.override_name(LEAF), Some("plan:GAME_ACTIVATE[fine]"));
        assert_eq!(d.override_name(0x40_4100), None);
    });
}

/// A string source served through the bridge on a running guest: ours are
/// answered, others reach the original.
#[test]
fn the_locale_bridge_serves_sources() {
    let rt = runtime();
    let mut r = Registry::new(Switches::parse("impl.engine=off").unwrap());
    r.add(&from_fn("ui-strings", |b| {
        b.locale()
            .source("ours", |id| (id == 0x0fff_0001).then_some(0xa2f0_0000));
        Ok(())
    }))
    .unwrap();
    r.build(&at(&points::LOCALE_FIND))
        .unwrap()
        .install(&rt)
        .unwrap();
    assert_eq!(call(&rt, LEAF, 0x0fff_0001).eax, 0xa2f0_0000);
    assert_eq!(call(&rt, LEAF, 5).eax, 41);
}
