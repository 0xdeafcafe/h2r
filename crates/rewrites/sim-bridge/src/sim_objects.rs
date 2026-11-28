//! The object update, phase 3 of the simulation rewrite
//! (docs/engine/object-update.md; design docs/engine/sim-rewrite-objects.md),
//! always installed, with no switch.
//!
//! Every body in `engine::sim_objects::BODIES` but the aim update `c7840`
//! (the animation subsystem's, `crate::sim_native`) gets its entry and its
//! return sites ([`table`]): the walks, the per-object updates with
//! `bd090`, the type dispatchers and callbacks with their trees, the wake
//! leaves, the pose and after-root dispatchers, function values with
//! their imports, change colours, the damage update, the post-update
//! shell, lights and the machine's slot `+48` query. At either, the Rust
//! runs the body and every body it reaches ([`objects::drive_with`]); a
//! true callout to code with no body is made in place where the runtime
//! can (`ExecCtx::call_inline`, docs/perf/sim-10x-callouts.md) and the
//! Rust frame carries on at its site. No body hands a call to its
//! original: one entered outside the float environment its arithmetic
//! assumes is counted (`float_env`, `native_walk::Refusal`) and still
//! answered. The per-function ports at a point a body owns
//! (`sim_physics::PORTS`, `sim_native::ENTRIES`) stay test oracles.
//!
//! A body whose point another feature hooks too (the Halo 1 health
//! model's `before` and `after` on the damage update) is guarded when the
//! plan is installed (`hooks::install::guard_shared_bodies`): calls into
//! it go through its chain, so those hooks run around the body.
//!
//! The tick's combat events (`sim_state::events`) are recorded by
//! observers at the damage report and the player kill credit, and the
//! buffer is emptied at the tick's entry (docs/engine/object-update.md,
//! "Slice 8 body"); finalize records its `ObjectDeleted` events into the
//! same buffer.
//!
//! The function value `bab40` runs the Rust evaluator
//! (`engine::sim_objects::functions::arm`) over its register-level body:
//! the evaluator answers once proven against the body, which answers
//! until then. The tag-function curve evaluator `13b390` is Rust's
//! ([`install_curve_arm`]).
//!
//! [`crate::sim_native::SimNative`] calls [`install`]: the bodies hold no
//! state, so the feature's snapshot policy does not change. The arms'
//! Rust-owned data and counters are the map's, cleared after a map load.
use hooks::Builder;
use guest_layout::points;
use engine::sim_objects::native_walk::{self, Refusal};
use engine::sim_objects::{self as objects, Table, damage};
use runtime::{CpuAbi, ExecCtx, Fallback, Guest, GuestFault, Inline, TransferAbi};
use std::sync::LazyLock;

/// The label of the bodies' implementations.
pub const OBJECTS: &str = "sim.native.objects";

/// The installed bodies: every body but the aim update `c7840`, with the
/// float-free ones marked (`native_walk::FLOAT_FREE`).
pub fn table() -> &'static Table {
    static TABLE: LazyLock<Table> = LazyLock::new(|| {
        Table::new(objects::BODIES.iter().filter(|b| b.point.base != points::UNIT_AIM_UPDATE.base))
            .expect("BODIES is consistent")
            .float_free(native_walk::FLOAT_FREE)
    });
    &TABLE
}

/// Whether a body owns `addr`, so the per-function port there stands down.
pub fn owns(addr: u32) -> bool {
    table().owns(addr)
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
    let bodies = table();
    for point in SHADOW_POINTS {
        crate::shadow::register(point.name, OBJECTS, point, bodies.owns(point.base));
    }
    install_bodies(b, bodies);
    install_events(b);
    install_curve_arm(b);
    // The arms' data and counters are the map's: a new map starts over.
    b.raw().observe_before(&points::CACHE_TAGS_LOAD, CURVE_ARM_RESET, |_| {
        engine::sim_objects::functions::parity::reset()
    });
    b.raw().observe_before(&points::CACHE_TAGS_LOAD, FUNCTION_ARM_RESET, |_| {
        engine::sim_objects::functions::arm::reset()
    });
}

/// The map-load observer that clears the curve evaluator's counters.
const CURVE_ARM_RESET: &str = "objects: reset the curve arm";
/// The map-load observer that clears the function arm and its data.
const FUNCTION_ARM_RESET: &str = "objects: reset the function arm";

/// Every body of `table`, each entry and site answered by the Rust
/// ([`segment`]). The walks and the per-object updates are also marked
/// [`hooks::HookRef::replaces_guest`] (`native_walk::REPLACES_GUEST`).
fn install_bodies(b: &mut Builder, table: &'static Table) {
    for &body in table.bodies() {
        let point = body.point;
        let hook = b
            .raw()
            .implement(point, OBJECTS, move |cpu, ctx, pc| {
                let site = pc != point.base && body.sites.contains(&pc);
                if pc != point.base && !site {
                    return original(cpu, ctx, pc, site);
                }
                segment(cpu, ctx, table, pc, site)
            })
            .resumes(body.sites)
            .context_only()
            // Another feature hooks this point too: calls go through the
            // chain (`hooks::install::guard_shared_bodies`).
            .when_shared(move |entry| {
                table.guard(entry);
            });
        if native_walk::REPLACES_GUEST.contains(&point.base) {
            hook.replaces_guest();
        }
    }
}

/// One entry or site of a body of `table` at `pc`: the Rust runs it and
/// every body it reaches, and carries on after each true callout the
/// runtime makes in place ([`drive_in_place`]). A float environment off
/// the body's is counted (`float_env`) and the Rust still answers. Each
/// callout is counted ([`native_walk::callout`]).
fn segment(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    table: &'static Table,
    pc: u32,
    site: bool,
) -> Result<TransferAbi, GuestFault> {
    drive_in_place(
        cpu,
        ctx,
        table,
        pc,
        site,
        |cpu, pc| {
            if !table.supports_at(pc, cpu) {
                native_walk::refused(pc, Refusal::FloatEnv);
            }
            true
        },
        |cpu, g, pc| drive(cpu, g, table, pc, &crate::shadow::SHADOW),
        native_walk::callout,
    )
}

/// Run `drive` from `pc` (an entry, or a site when `site`) under a memory
/// guard, then make the true callout it ends with in place
/// (`ExecCtx::call_inline`, docs/perf/sim-10x-callouts.md) and carry on
/// at its site, until a transfer leaves for the runner: a return or tail
/// jump out of the table, or a callout the runtime left to it (`note`
/// hears each callout's site and why). Where `admit` refuses the Rust at
/// `pc` the original runs, at its resume when `pc` is a site (only the
/// sound table, `crate::sim_sound`, refuses; the object bodies' `admit`
/// counts and admits). Only a call to a body site is made in place, so
/// the runner's resume registration is the one a fallback returns
/// through.
fn drive_in_place(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    table: &Table,
    mut pc: u32,
    mut site: bool,
    mut admit: impl FnMut(&CpuAbi, u32) -> bool,
    mut drive: impl FnMut(&mut CpuAbi, &mut Guest, u32) -> objects::Result<TransferAbi>,
    mut note: impl FnMut(u32, Option<Fallback>),
) -> Result<TransferAbi, GuestFault> {
    loop {
        if !admit(cpu, pc) {
            return original(cpu, ctx, pc, site);
        }
        let transfer = {
            let mut mem = ctx.memory();
            drive(cpu, &mut mem, pc)?
        };
        let at = transfer.resume_pc;
        if transfer.kind != runtime::abi::CALL || !table.resumes(at) {
            return Ok(transfer);
        }
        match ctx.call_inline(cpu, transfer.target_pc, at) {
            Inline::Returned => {
                note(at, None);
                (pc, site) = (at, true);
            }
            Inline::Leave { transfer, why } => {
                note(at, Some(why));
                return Ok(transfer);
            }
        }
    }
}

/// The tag-function curve evaluator `13b390` (`FUNCTION_EVALUATE`): Rust
/// answers every call (`engine::sim_objects::functions::parity::enter`),
/// the idiomatic kernel or, for what it does not carry, the
/// register-level port.
fn install_curve_arm(b: &mut Builder) {
    use engine::sim_objects::functions::parity;
    let point = &points::FUNCTION_EVALUATE;
    b.raw()
        .implement(point, OBJECTS, move |cpu, ctx, pc| {
            if pc != point.base {
                unimplemented!("objects curve: entered at {pc:#x}, not the entry");
            }
            let target = {
                let mem = ctx.memory();
                parity::enter(cpu, &mem)
            };
            Ok(TransferAbi::ret(target))
        })
        .context_only()
        .optional();
}

/// Run the body at `pc` and every body it reaches in `table`, each
/// segment counted (`native_walk::segment`). At `bab40`'s entry the
/// function arm may answer first; a shadowed point runs through `shadow`.
fn drive(
    cpu: &mut CpuAbi,
    g: &mut Guest,
    table: &Table,
    pc: u32,
    shadow: &crate::shadow::Shadow,
) -> objects::Result<TransferAbi> {
    use engine::sim_objects::functions::arm::{self as function_arm, Step};
    let shadowed = shadow.any_enabled();
    objects::drive_with(cpu, g, table, pc, |body, cpu, g, pc| {
        let value = body.point.base == points::OBJECT_FUNCTION_VALUE.base;
        if value && pc == body.point.base {
            if let Step::Answered { target } = function_arm::enter(cpu, g) {
                return Ok(TransferAbi::ret(target));
            }
        }
        let transfer = if shadowed && SHADOW_POINTS.iter().any(|point| point.base == body.point.base) {
            shadow.run(body.point.name, cpu, g, pc, crate::shadow::game_tick, &mut |cpu, g| {
                body.segment(cpu, g, pc)
            })?
        } else {
            native_walk::segment(body, cpu, g, pc)?
        };
        if value && transfer.kind == runtime::abi::RETURN {
            function_arm::returned(cpu, g);
        }
        Ok(transfer)
    })
}

/// The event observers. They write only the game thread's event buffer.
fn install_events(b: &mut Builder) {
    b.raw().observe_before(&points::GAME_TICK, EVENTS_CLEAR, |_| damage::tick_starts());
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

/// The tick-entry observer that empties the event buffer.
const EVENTS_CLEAR: &str = "events: tick";

/// Install `table`'s bodies under `label`, each call admitted only inside
/// the table's float environment (else the original runs), for a table
/// that is not the object update's (`crate::sim_sound`).
pub(crate) fn install_table(
    b: &mut Builder,
    table: &'static Table,
    label: &'static str,
    generated_sites: fn(u32) -> &'static [u32],
) {
    for &body in table.bodies() {
        let point = body.point;
        b.raw()
            .implement(point, label, move |cpu, ctx, pc| {
                let site = pc != point.base && body.sites.contains(&pc);
                if pc != point.base && !site {
                    return original(cpu, ctx, pc, site);
                }
                drive_in_place(
                    cpu,
                    ctx,
                    table,
                    pc,
                    site,
                    |cpu, _| table.supports(cpu),
                    |cpu, g, pc| drive(cpu, g, table, pc, &crate::shadow::SHADOW),
                    |_, _| {},
                )
            })
            .resumes(body.sites)
            .generated_resumes(generated_sites(point.base))
            .context_only()
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
    use guest_layout::points::{self, Point};
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
        use guest_layout::points::Sig;
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
    fn every_body_but_the_aim_update_installs_with_its_sites() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for body in objects::BODIES {
            let implement = plan.chains[&body.point.base].implement.as_ref().unwrap();
            if body.point.base == points::UNIT_AIM_UPDATE.base {
                assert_eq!(implement.label, crate::sim_native::ANIM, "the animation subsystem's");
                continue;
            }
            assert_eq!(implement.label, OBJECTS, "{}", body.point.name);
            assert_eq!(implement.continuation_sites().0, body.sites);
            assert_eq!(
                implement.replaces_guest(),
                native_walk::REPLACES_GUEST.contains(&body.point.base),
                "{}",
                body.point.name
            );
        }
        // The bodies hold no state; the manager keeps the "sim-native"
        // section, with no bytes.
        assert!(plan.save_sections().iter().all(|(_, bytes)| bytes.is_empty()));
        // Every walk entry has resume sites, so the lift keeps its body.
        let entries: std::collections::BTreeSet<u32> = native_walk::TABLE.bodies().iter().map(|b| b.point.base).collect();
        assert!(plan.replaced(&entries).is_empty());
        // The switches are gone.
        for gone in ["impl.sim.native.objects", "impl.sim.native.objects.functions"] {
            assert!(Switches::parse(gone).is_err(), "{gone}");
        }
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
    fn ports_at_owned_points_stand_down() {
        for point in [
            &points::OBJECTS_PRE_PHYSICS,
            &points::OBJECT_FUNCTION_VALUE,
            &points::OBJECT_DAMAGE_UPDATE,
            &points::LIGHTS_UPDATE,
            &points::LIGHT_RECORD_UPDATE,
            &points::OBJECT_TYPE_POSE,
            &points::OBJECT_TYPE_AFTER_ROOT,
            &points::MACHINE_SLOT_48,
            &points::UNIT_UPDATE,
            &points::ITEM_UPDATE,
        ] {
            assert!(owns(point.base), "{}", point.name);
        }
        // The aim update is the animation subsystem's.
        assert!(!owns(points::UNIT_AIM_UPDATE.base));
        // A site is not an entry: the port there is not displaced by it.
        let site = objects::TABLE.at(points::OBJECTS_PRE_PHYSICS.base).unwrap().0.sites[0];
        assert!(!owns(site));
    }

    #[test]
    fn events_are_recorded_at_the_tick_the_damage_report_and_the_kill() {
        let mut registry = Registry::new(Switches::defaults());
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

    #[test]
    fn a_map_load_resets_both_arms() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        let load = &plan.chains[&points::CACHE_TAGS_LOAD.base];
        for label in [CURVE_ARM_RESET, FUNCTION_ARM_RESET] {
            assert!(load.observe_before.iter().any(|h| h.label == label), "{label}");
        }
    }

    /// The real `bab40` body under the driver with the arm on: the
    /// evaluator's prediction at the entry matches the body's return.
    #[test]
    fn the_function_arm_compares_the_value_body_at_its_return() {
        use engine::sim_objects::functions::arm;
        use runtime::guest::Permissions;
        const STACK: u32 = 0x007f_0800;
        const OUT: u32 = 0x007f_0f00;
        const POOL: u32 = 0x0050_0000;
        arm::reset();
        let mut g = Guest::new();
        g.map_zeroed(0x000b_0000, 0x0010_0000, Permissions::RW).unwrap();
        g.map_zeroed(0x0041_0000, 0x0020_0000, Permissions::RW).unwrap();
        g.map_zeroed(0x007f_0000, 0x1000, Permissions::RW).unwrap();
        g.set_u32(engine::function_eval::constant::ONE, 1.0f32.to_bits()).unwrap();
        g.set_u32(points::OBJECTS.base, POOL).unwrap();
        g.set_u32(POOL + 0x44, POOL + 0x1000).unwrap();
        g.set_u32(POOL + 0x1000 + 12 + 8, POOL + 0x2000).unwrap();
        // `bab40(object 1, name 0, OUT)`, returning outside the table.
        for (off, v) in [(0, 0x0001_2345), (4, 0x1234_0001), (8, 0), (0xc, OUT)] {
            g.set_u32(STACK + off, v).unwrap();
        }
        let mut cpu = CpuAbi { esp: STACK, ecx: 0xaabb_ccdd, mxcsr: 0x1fa0, ..Default::default() };
        cpu.x87.control = 0x027f;
        let shadow = crate::shadow::Shadow::new();
        let value = points::OBJECT_FUNCTION_VALUE.base;
        let transfer = drive(&mut cpu, &mut g, &objects::TABLE, value, &shadow).unwrap();
        assert_eq!(transfer, TransferAbi::ret(0x0001_2345));
        assert_eq!(g.u32(OUT).unwrap(), 1.0f32.to_bits());
        let s = arm::status();
        assert_eq!((s.checked, s.matched, s.mismatched, s.answered, s.dropped), (1, 1, 0, 0, 0));
        arm::reset();
    }

    #[test]
    fn the_curve_evaluator_is_the_kernels() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        let chain = &plan.chains[&points::FUNCTION_EVALUATE.base];
        let implement = chain.implement.as_ref().unwrap();
        assert_eq!(implement.label, OBJECTS);
        assert!(implement.optional);
        assert!(chain.observe_before.is_empty() && chain.observe_after.is_empty());
        assert!(implement.continuation_sites().0.is_empty());
    }

    #[test]
    fn damage_functions_lights_and_the_pose_dispatchers_are_rust_bodies() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for point in [
            &points::OBJECT_DAMAGE_UPDATE,
            &points::OBJECT_FUNCTION_VALUE,
            &points::LIGHTS_UPDATE,
            &points::LIGHT_RECORD_UPDATE,
            &points::OBJECT_TYPE_POSE,
            &points::OBJECT_TYPE_AFTER_ROOT,
            &points::MACHINE_SLOT_48,
        ] {
            let implement = plan.chains[&point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, OBJECTS, "{}", point.name);
        }
        // Finalize's deletion events and the combat events are emptied at
        // the tick's entry.
        let tick = &plan.chains[&points::GAME_TICK.base];
        assert!(tick.observe_before.iter().any(|h| h.label == EVENTS_CLEAR));
    }

    /// Outside the float environment, the root leaf `baf80` (float-free)
    /// is the Rust's and counted as a segment, and `bd090` is the Rust's
    /// too, counted as `float_env`; the guest runs neither.
    #[test]
    fn a_body_answers_outside_the_float_environment_and_counts_it() {
        use runtime::guest::Permissions;
        use runtime::{Dispatch, Runtime};
        const STACK: u32 = 0x0057_0800;
        const CALLER: u32 = 0x0001_2345;
        let pose = points::OBJECT_NODE_MATRICES.base;
        let root = points::OBJECT_ROOT.base;
        let mut d = Dispatch::default();
        d.register_generated_rust(pose, |_, _, _| panic!("bd090 is never the guest's")).unwrap();
        d.register_generated_rust(root, |_, _, _| panic!("the root leaf is never the guest's")).unwrap();
        let mut g = Guest::new();
        g.map_zeroed(0x0057_0000, 0x1000, Permissions::RW).unwrap();
        g.set_u32(STACK, CALLER).unwrap();
        let mut ctx = ExecCtx::new(Runtime::new(g, d), 1);
        let count = |entry: u32| native_walk::stats().into_iter().find(|s| s.entry == entry).unwrap();
        // Flush to zero.
        let ftz = || {
            let mut cpu = CpuAbi { esp: STACK, ecx: u32::MAX, mxcsr: 0x9f80, ..Default::default() };
            cpu.x87.control = 0x027f;
            cpu
        };
        let before = (count(root), count(pose));
        let mut cpu = ftz();
        let t = segment(&mut cpu, &mut ctx, table(), root, false).unwrap();
        assert_eq!(t, TransferAbi::ret(CALLER));
        assert_eq!((cpu.eax, cpu.esp), (u32::MAX, STACK + 4));
        assert_eq!(count(root).segments, before.0.segments + 1);
        assert_eq!(count(root).refused, before.0.refused, "float-free");
        // bd090 on an object pool the guest does not map: the Rust body
        // runs, and faults or calls out, but never enters its original.
        let mut cpu = ftz();
        let _ = segment(&mut cpu, &mut ctx, table(), pose, false);
        assert_eq!(count(pose).refused[0], before.1.refused[0] + 1);
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
            install_table(b, self.0, OBJECTS, |_| &[]);
            Ok(())
        }
    }
}
