//! `impl.sim.native.objects`: the object update, phase 3 of the simulation
//! rewrite (docs/engine/object-update.md; design
//! docs/engine/sim-rewrite-objects.md). Default off.
//!
//! Every body in `engine::sim_objects::BODIES` gets its entry and its
//! return sites. At either, with the float environment the bodies assume,
//! `engine::sim_objects::drive` runs it and every body it reaches; anywhere
//! else the original runs (the entry, or the resume at the site), which
//! the retail frame the bodies keep makes safe. The per-function ports at
//! a point a body owns (`sim_physics::PORTS`, `sim_native::ENTRIES`) stay
//! test oracles while the switch is on.
//!
//! A body whose point another feature hooks too (the Halo 1 health
//! model's `before` and `after` on the damage update) is guarded when the
//! plan is installed (`hooks::install::guard_shared_bodies`): calls into
//! it go through its chain, so those hooks run around the body as around
//! the original.
//!
//! The tick's combat events (`sim_state::events`) are recorded by
//! observers at the damage report and the player kill credit, and
//! the buffer is emptied at the tick's entry (docs/engine/object-update.md,
//! "Slice 8 body").
//!
//! [`crate::sim_native::SimNative`] calls [`install`]: the bodies hold no
//! state, so the feature's snapshot policy does not change.
use hooks::Builder;
use engine::points;
use engine::sim_objects::{self as objects, Table, damage};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};

/// The switch, without its `impl.` prefix, and the label of the bodies'
/// implementations.
pub const OBJECTS: &str = "sim.native.objects";

pub fn on(switches: &hooks::Switches) -> bool {
    switches.implementation(OBJECTS, false)
}

/// Whether the per-function port at `addr` stands down: the switch is on
/// and a body owns the point.
pub fn owns(switches: &hooks::Switches, addr: u32) -> bool {
    owns_in(on(switches), table(switches), addr)
}

fn owns_in(on: bool, table: &Table, addr: u32) -> bool {
    on && table.owns(addr)
}

// Both registration and the fused call/return driver must agree on who
// handles c7840. Removing it from this table also sends its callers
// through normal dispatch when the image lacks the animation selectors.
fn table(switches: &hooks::Switches) -> &'static Table {
    static WITHOUT_AIM: std::sync::LazyLock<Table> = std::sync::LazyLock::new(|| {
        Table::new(objects::BODIES.iter().filter(|b| b.point.base != points::UNIT_AIM_UPDATE.base))
            .expect("BODIES is consistent")
    });
    if crate::sim_native::anim_on(switches) { &WITHOUT_AIM } else { &objects::TABLE }
}

// Each listed segment reads or writes only its CpuAbi and Guest. Bodies
// with host-visible effects (e.g. finalize records a deletion) run once on
// the live path, including when reached by a checked segment.
pub(crate) const SHADOW_POINTS: &[&points::Point] = &[
    &points::OBJECTS_PRE_PHYSICS,
    &points::OBJECTS_POST_PHYSICS,
    &points::OBJECT_FUNCTION_VALUE,
    &points::LIGHT_RECORD_UPDATE,
    &points::UNIT_CLAMP_DIRECTION,
    &points::UNIT_ACTIONS_UPDATE,
];

pub(crate) fn install(b: &mut Builder) {
    let installed = on(b.switches());
    let bodies = table(b.switches());
    for point in SHADOW_POINTS {
        crate::shadow::register(point.name, OBJECTS, point, installed && bodies.owns(point.base));
    }
    if installed {
        install_table(b, bodies);
        install_events(b);
    }
}

fn drive(
    cpu: &mut CpuAbi,
    g: &mut Guest,
    table: &Table,
    pc: u32,
    shadow: &crate::shadow::Shadow,
) -> objects::Result<TransferAbi> {
    if !shadow.any_enabled() {
        return objects::drive(cpu, g, table, pc);
    }
    objects::drive_with(cpu, g, table, pc, |body, cpu, g, pc| {
        if SHADOW_POINTS.iter().any(|point| point.base == body.point.base) {
            shadow.run(body.point.name, cpu, g, pc, crate::shadow::game_tick, &mut |cpu, g| {
                body.segment(cpu, g, pc)
            })
        } else {
            body.segment(cpu, g, pc)
        }
    })
}

/// The event observers. They write only the game thread's event buffer.
fn install_events(b: &mut Builder) {
    b.raw().observe_before(&points::GAME_TICK, "events: tick", |_| damage::tick_starts());
    b.raw()
        .observe_before(&points::DAMAGE_REPORT, "events: damage", |o| {
            if let (Some(report), Some(victim)) = (o.arg_u32("report"), o.arg_u32("victim")) {
                damage::damage_report(&o.guest(), report, victim);
            }
        })
        .optional();
    b.raw().observe_before(&points::PLAYER_KILLED, "events: kill", |o| {
        damage::player_killed(&o.guest(), o.cpu.esp);
    });
}

fn install_table(b: &mut Builder, table: &'static Table) {
    for &body in table.bodies() {
        let point = body.point;
        b.raw()
            .implement(point, OBJECTS, move |cpu, ctx, pc| {
                let site = pc != point.base && body.sites.contains(&pc);
                if (pc == point.base || site) && objects::supported(cpu) {
                    let mut mem = ctx.memory();
                    return drive(cpu, &mut mem, table, pc, &crate::shadow::SHADOW);
                }
                original(cpu, ctx, pc, site)
            })
            .resumes(body.sites)
            .context_only()
            // Another feature hooks this point too: calls go through the
            // chain (`hooks::install::guard_shared_bodies`).
            .when_shared(move |entry| {
                table.guard(entry);
            });
    }
}

/// The original at an entry, or its resume at a site.
pub(crate) fn original(cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32, site: bool) -> Result<TransferAbi, GuestFault> {
    let original = if site {
        ctx.runtime().resolve_original_resume(pc)
    } else {
        ctx.runtime().resolve_original(pc).map(|h| (h, pc))
    };
    let (handler, selector) = original.ok_or(GuestFault {
        addr: pc,
        len: 1,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::Unmapped,
    })?;
    Ok(handler.invoke(cpu, ctx, selector))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_native::SimNative;
    use hooks::{Base, Registry, Switches};
    use engine::points::{self, Point};
    use engine::sim_objects::{Body, Cx, Next};

    fn ret(_: &mut Cx<'_>, _: u32) -> objects::Result<Next> {
        Ok(Next::Ret { pop: 4 })
    }

    static PRE_SITES: [u32; 2] = [0xb6f6e, 0xb6f7f];
    static TEST_BODIES: [Body; 2] = [
        Body { point: &points::OBJECTS_PRE_PHYSICS, sites: &PRE_SITES, run: ret },
        Body { point: &points::OBJECT_FUNCTION_VALUE, sites: &[], run: ret },
    ];

    fn test_table() -> &'static Table {
        static T: std::sync::LazyLock<Table> =
            std::sync::LazyLock::new(|| Table::new(TEST_BODIES.iter()).unwrap());
        &T
    }

    #[test]
    fn shadow_checks_a_fused_call_and_resume_without_replaying_the_callee() {
        use engine::points::Sig;
        use lifted::code::Assembled;
        use runtime::guest::Permissions;
        use std::sync::atomic::{AtomicU32, Ordering};

        const VALUE: u32 = points::OBJECT_FUNCTION_VALUE.base;
        const SITE: u32 = VALUE + 5;
        const STACK: u32 = 0x8100;
        static START: Point = Point::new("START", VALUE + 32, Sig::Opaque, "");
        static EFFECT: Point = Point::new("EFFECT", VALUE + 16, Sig::Opaque, "");
        static CALLS: AtomicU32 = AtomicU32::new(0);
        static BODIES: [Body; 3] = [
            Body { point: &START, sites: &[], run: |_, _| Ok(Next::Tail { target: VALUE }) },
            Body { point: &points::OBJECT_FUNCTION_VALUE, sites: &[SITE], run: |cx, pc| {
                if pc == VALUE {
                    Ok(Next::Call { target: EFFECT.base, site: SITE })
                } else {
                    cx.cpu.eax = 27;
                    Ok(Next::Ret { pop: 0 })
                }
            } },
            Body { point: &EFFECT, sites: &[], run: |_, _| {
                CALLS.fetch_add(1, Ordering::Relaxed);
                Ok(Next::Ret { pop: 0 })
            } },
        ];
        let table = Table::new(BODIES.iter()).unwrap();
        let shadow = crate::shadow::Shadow::new();
        let value = &points::OBJECT_FUNCTION_VALUE;
        shadow.register(value.name, OBJECTS, value.name, VALUE, true);

        for answer in [27, 28] {
            // Synthetic original: call EFFECT; mov eax,answer; ret.
            let bytes = [0xe8, 11, 0, 0, 0, 0xb8, answer, 0, 0, 0, 0xc3];
            shadow.set_code(Box::new(Assembled::new(VALUE, &bytes)));
            shadow.enable(&[OBJECTS.into()], None, None).unwrap();
            let mut g = Guest::new();
            g.map_zeroed(0x8000, 0x1000, Permissions::RW).unwrap();
            g.set_u32(STACK, 0xabcd).unwrap();
            let mut cpu = CpuAbi { esp: STACK, ..Default::default() };
            let before = CALLS.load(Ordering::Relaxed);
            let transfer = drive(&mut cpu, &mut g, &table, START.base, &shadow).unwrap();
            assert_eq!(CALLS.load(Ordering::Relaxed) - before, 1);
            assert_eq!(transfer, TransferAbi::ret(0xabcd));
            assert_eq!((cpu.esp, cpu.eax), (STACK + 4, 27));
            let status = shadow.status();
            assert_eq!((status.calls, status.checked, status.skipped), (2, 2, 0));
            if answer == 27 {
                assert_eq!(status.matched, 2);
            } else {
                assert_eq!((status.matched, status.mismatched), (1, 1));
                assert_eq!(status.mismatches[0].pc, format!("{SITE:#010x}"));
            }
        }
    }

    #[test]
    fn the_switch_is_off_by_default_and_installs_every_body_when_on() {
        assert!(!on(&Switches::defaults()));
        assert!(on(&Switches::defaults().with(&format!("impl.{OBJECTS}"), true)));
        // `-impl.sim.native` turns every subsystem off.
        assert!(!on(&Switches::defaults().with("impl.sim.native", false)));
        assert!(!objects::TABLE.is_empty());
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        assert!(plan.chains.is_empty(), "off: nothing installs");
        assert!(plan.save_sections().is_empty());

        let mut registry = Registry::new(Switches::defaults().with(&format!("impl.{OBJECTS}"), true));
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for body in objects::BODIES {
            let implement = plan.chains[&body.point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, OBJECTS, "{}", body.point.name);
            assert_eq!(implement.continuation_sites().0, body.sites);
        }
        // The bodies hold no state: no snapshot section.
        assert!(plan.save_sections().is_empty());
    }

    #[test]
    fn every_body_entry_is_installed_with_its_sites() {
        let mut registry = Registry::new(Switches::defaults());
        let feature = Installs(test_table());
        registry.add(&feature).unwrap();
        let plan = registry.build(&Base).unwrap();
        assert_eq!(plan.chains.len(), TEST_BODIES.len());
        for body in &TEST_BODIES {
            let implement = plan.chains[&body.point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, OBJECTS);
            assert_eq!(implement.continuation_sites().0, body.sites);
        }
    }

    #[test]
    fn ports_at_owned_points_stand_down_only_while_the_switch_is_on() {
        let t = test_table();
        let owned: [&Point; 2] = [&points::OBJECTS_PRE_PHYSICS, &points::OBJECT_FUNCTION_VALUE];
        for point in owned {
            assert!(owns_in(true, t, point.base));
            assert!(!owns_in(false, t, point.base));
        }
        assert!(!owns_in(true, t, points::OBJECTS_POST_PHYSICS.base));
        // A site is not an entry: the port there is not displaced by it.
        assert!(!owns_in(true, t, PRE_SITES[0]));
        // The slice 1 bodies own the walks while the switch is on.
        let on = Switches::defaults().with(&format!("impl.{OBJECTS}"), true);
        assert!(owns(&on, points::OBJECTS_PRE_PHYSICS.base));
        assert!(!owns(&Switches::defaults(), points::OBJECTS_PRE_PHYSICS.base));
    }

    #[test]
    fn the_switch_records_events_at_the_tick_the_damage_report_and_the_kill() {
        let mut registry = Registry::new(Switches::defaults().with(&format!("impl.{OBJECTS}"), true));
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for (point, label) in [
            (&points::GAME_TICK, "events: tick"),
            (&points::DAMAGE_REPORT, "events: damage"),
            (&points::PLAYER_KILLED, "events: kill"),
        ] {
            let chain = &plan.chains[&point.base];
            assert!(chain.observe_before.iter().any(|h| h.label == label), "{}", point.name);
        }
    }

    static DAMAGE_BODIES: [Body; 2] = [
        Body { point: &points::OBJECTS_PRE_PHYSICS, sites: &PRE_SITES, run: ret },
        Body { point: &points::OBJECT_DAMAGE_UPDATE, sites: &[], run: ret },
    ];

    /// A feature with a `before` at the damage update, as the Halo 1
    /// health model has.
    struct HooksDamage;

    impl hooks::Feature for HooksDamage {
        fn manifest(&self) -> hooks::Manifest {
            hooks::Manifest::new("damage-hook-test", "a before at the damage update")
        }
        fn build(&self, b: &mut Builder) -> Result<(), hooks::BuildError> {
            b.snapshot(hooks::SnapshotPolicy::Stateless);
            b.raw().before(&points::OBJECT_DAMAGE_UPDATE, "test", |_| Ok(hooks::Flow::Continue));
            Ok(())
        }
    }

    #[test]
    fn a_body_another_feature_hooks_is_guarded_and_the_others_are_not() {
        static T: std::sync::LazyLock<Table> =
            std::sync::LazyLock::new(|| Table::new(DAMAGE_BODIES.iter()).unwrap());
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&Installs(&T)).unwrap();
        registry.add(&HooksDamage).unwrap();
        let plan = registry.build(&Base).unwrap();
        hooks::install::guard_shared_bodies(plan.chains.values());
        assert!(T.guarded(points::OBJECT_DAMAGE_UPDATE.base));
        assert!(!T.guarded(points::OBJECTS_PRE_PHYSICS.base));

        // Alone, nothing is guarded.
        static U: std::sync::LazyLock<Table> =
            std::sync::LazyLock::new(|| Table::new(DAMAGE_BODIES.iter()).unwrap());
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&Installs(&U)).unwrap();
        let plan = registry.build(&Base).unwrap();
        hooks::install::guard_shared_bodies(plan.chains.values());
        assert!(!U.guarded(points::OBJECT_DAMAGE_UPDATE.base));
    }

    // Slice 6a: the resume machinery on a real body, the unit update's
    // shell (`c6b30`) alone, so its nine calls are true callouts.

    fn unit_table() -> &'static Table {
        static T: std::sync::LazyLock<Table> = std::sync::LazyLock::new(|| {
            Table::new(objects::BODIES.iter().filter(|b| b.point.base == points::UNIT_UPDATE.base)).unwrap()
        });
        &T
    }

    fn unit_body() -> &'static Body {
        unit_table().bodies()[0]
    }

    const UNIT_STACK: u32 = 0x703000;

    fn unit_guest(arg: u32) -> runtime::Guest {
        use runtime::guest::Permissions;
        let mut mem = runtime::Guest::new();
        mem.map_zeroed(0xb0000, 0x80000, Permissions::RX).unwrap();
        mem.map_zeroed(UNIT_STACK - 0x1000, 0x2000, Permissions::RW).unwrap();
        mem.set_u32(UNIT_STACK, runtime::exec::HARNESS_EXIT).unwrap();
        mem.set_u32(UNIT_STACK + 4, arg).unwrap();
        mem
    }

    #[test]
    fn a_body_resumes_at_its_sites_on_the_real_guest_stack() {
        use runtime::{Dispatch, Event, Runner, Runtime};
        use std::sync::{Arc, Mutex};
        let body = unit_body();
        let mut dispatch = Dispatch::default();
        dispatch
            .register_generated_rust(body.point.base, |_, _, _| panic!("unit original reached with the body installed"))
            .unwrap();
        for &site in body.sites {
            dispatch.register_resume(site, body.point.base, site).unwrap();
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        for (target, cleanup) in [
            (0xc49b0, 4),
            (0xc58f0, 4),
            (0xc60c0, 4),
            (0xc5eb0, 0),
            (0xc6740, 0),
            (0x114c60, 0),
            (0xc6990, 0),
            (0xc6810, 0),
            (0xc50a0, 0),
        ] {
            let calls = Arc::clone(&calls);
            dispatch.register_override(target, "unit fixture", move |cpu, ctx, _| {
                let mut calls = calls.lock().unwrap();
                let mem = ctx.memory();
                if cleanup != 0 {
                    assert_eq!(mem.u32(cpu.esp + 4)?, 0x9000_0032);
                } else {
                    assert_eq!(cpu.eax, 0x9000_0032);
                }
                calls.push(mem.u32(cpu.esp)?);
                cpu.eax = 0x4400_0000 | (1 << ((calls.len() - 1) % 8));
                mem.ret(cpu, cleanup)
            });
        }
        let rt = Runtime::new(unit_guest(0x9000_0032), dispatch);
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&Installs(unit_table())).unwrap();
        registry.build(&Base).unwrap().install(&rt).unwrap();
        let mut cpu = CpuAbi { esp: UNIT_STACK, ebx: 17, esi: 25, ..Default::default() };
        let mut ctx = ExecCtx::new(rt, 1);
        let done = Runner::default().run(&mut cpu, &mut ctx, body.point.base);
        assert!(matches!(ctx.event(done.event_id), Some(Event::HarnessExit)));
        assert_eq!(&*calls.lock().unwrap(), body.sites);
        // The first five answers ORed: 1 | 2 | 4 | 8 | 10.
        assert_eq!((cpu.eax, cpu.ebx, cpu.esi, cpu.esp), (0x4400_001f, 17, 25, UNIT_STACK + 8));
    }

    #[test]
    fn a_disabled_body_returns_to_the_generated_owner_with_its_selector() {
        use runtime::{Dispatch, Event, Runner, Runtime};
        let body = unit_body();
        let point = body.point.base;
        let first = body.sites[0];
        let mut dispatch = Dispatch::default();
        // A module may assign a shared return site to another canonical owner.
        let owner = 0xc6800;
        let selector = owner + 12;
        dispatch.register_generated_rust(point, |_, _, _| panic!("unexpected entry")).unwrap();
        dispatch
            .register_generated_rust(owner, move |cpu, ctx, pc| {
                assert_eq!(pc, selector);
                let memory = ctx.memory();
                cpu.esi = memory.u32(cpu.esp)?;
                cpu.ebx = memory.u32(cpu.esp + 4)?;
                cpu.esp += 8;
                cpu.eax = 91;
                memory.ret(cpu, 4)
            })
            .unwrap();
        for &pc in body.sites {
            dispatch.register_resume(pc, owner, if pc == first { selector } else { pc }).unwrap();
        }
        dispatch.register_override(0xc49b0, "disable the unit body", move |cpu, ctx, _| {
            assert!(ctx.runtime().edit_dispatch(|d| d.set_enabled(point, false)));
            ctx.memory().ret(cpu, 4)
        });
        let rt = Runtime::new(unit_guest(17), dispatch);
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&Installs(unit_table())).unwrap();
        registry.build(&Base).unwrap().install(&rt).unwrap();
        let mut ctx = ExecCtx::new(rt, 1);
        let mut cpu = CpuAbi { esp: UNIT_STACK, esi: 19, ebx: 23, ..Default::default() };
        let done = Runner::default().run(&mut cpu, &mut ctx, point);
        assert!(matches!(ctx.event(done.event_id), Some(Event::HarnessExit)));
        assert_eq!((cpu.eax, cpu.esi, cpu.ebx, cpu.esp), (91, 19, 23, UNIT_STACK + 8));
    }

    #[test]
    fn a_missing_continuation_leaves_the_plan_uninstalled() {
        use runtime::{Dispatch, Runtime};
        let body = unit_body();
        let mut dispatch = Dispatch::default();
        dispatch.register_generated_rust(body.point.base, |_, _, _| panic!("installation only")).unwrap();
        for &site in &body.sites[1..] {
            dispatch.register_resume(site, body.point.base, site).unwrap();
        }
        let rt = Runtime::new(runtime::Guest::new(), dispatch);
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&Installs(unit_table())).unwrap();
        let result = registry.build(&Base).unwrap().install(&rt);
        assert!(
            matches!(result, Err(hooks::InstallError::MissingResume { site, .. }) if site == body.sites[0])
        );
        assert!(rt.edit_dispatch(|d| d.override_name(body.point.base).is_none()));
    }

    /// A feature that installs a given table, so the installer is tested
    /// on bodies of its own.
    struct Installs(&'static Table);

    impl hooks::Feature for Installs {
        fn manifest(&self) -> hooks::Manifest {
            hooks::Manifest::new("sim-objects-test", "test bodies")
        }
        fn build(&self, b: &mut Builder) -> Result<(), hooks::BuildError> {
            b.snapshot(hooks::SnapshotPolicy::Stateless);
            install_table(b, self.0);
            Ok(())
        }
    }
}
