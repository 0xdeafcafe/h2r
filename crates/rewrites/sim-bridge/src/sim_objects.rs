//! `impl.sim.native.objects`: the object update, phase 3 of the simulation
//! rewrite (docs/engine/object-update.md; design
//! docs/engine/sim-rewrite-objects.md). Default off. With it off, the
//! walks and the per-object updates still run in Rust
//! (`impl.sim.native.objects.walk`, on; [`install_walk`]).
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
//! The function value `bab40` has a shadow arm over the Rust evaluator
//! (`engine::sim_objects::functions::arm`), on with
//! `impl.sim.native.objects.functions` ([`FUNCTIONS`]): at each entry the
//! driver runs, the evaluator answers once proven and is compared until
//! then, and on one call in 64 after.
//!
//! With [`OBJECTS`] off and [`WALK`] on (the default), only the walks
//! `b6f10`, `b7060` and `b7150`, the per-object updates `bc470` and
//! `bc820` with `bd090`, the type dispatchers, the wake leaves, and the
//! unit (`c6b30`), biped (`dd360`, `dd990`), weapon (`fdfb0`), item
//! (`10b4d0`), vehicle (`efde0`), projectile (`f8de0`) and scenery
//! (`10a2f0`) callbacks with their trees
//! (`engine::sim_objects::native_walk::TABLE`) are installed, and the Rust
//! answers every segment. `bd090` or a type callback outside its float
//! environment is the one refusal: the guest runs that call, counted as
//! `float_env`. Every other body is the guest's: the other types'
//! callbacks, damage, functions and lights. Finalize records its
//! `ObjectDeleted` events, so the tick's entry empties the buffer here too;
//! the combat observers are the objects switch's. With the animation
//! subsystem on, the aim update `c7840` is its (`native_walk::WITHOUT_AIM`),
//! as in `table`.
//!
//! [`crate::sim_native::SimNative`] calls [`install`]: the bodies hold no
//! state, so the feature's snapshot policy does not change. The arms'
//! Rust-owned data and proofs are the map's, rebuilt after a map load.
use hooks::Builder;
use engine::points;
use engine::sim_objects::native_walk::{self, Refusal};
use engine::sim_objects::{self as objects, Table, damage};
use runtime::{CpuAbi, ExecCtx, Fallback, Guest, GuestFault, Inline, TransferAbi};

/// The switch, without its `impl.` prefix, and the label of the bodies'
/// implementations.
pub const OBJECTS: &str = "sim.native.objects";

pub fn on(switches: &hooks::Switches) -> bool {
    switches.implementation(OBJECTS, false)
}

/// The function value's shadow arm, looked up exactly: it is on only when
/// set, and only under [`OBJECTS`]. Off by default until the parity ladder
/// passes (docs/engine/sim-rewrite.md, "Objects and units update (phase
/// 3)").
pub const FUNCTIONS: &str = "impl.sim.native.objects.functions";

pub fn functions_on(switches: &hooks::Switches) -> bool {
    on(switches) && switches.feature(FUNCTIONS, false)
}

/// The walks in Rust (docs/engine/object-update.md, "The walks in
/// Rust"): on by default; it follows `impl.sim.native.objects` and
/// `impl.sim.native` when either is set, so `-impl.sim.native` runs the
/// guest's walks.
pub const WALK: &str = "impl.sim.native.objects.walk";

/// The label of the walk table's implementations.
pub const WALK_LABEL: &str = "sim.native.objects.walk";

/// Whether the walk table is installed on its own: [`WALK`] on and
/// [`OBJECTS`] off (on, every body is installed, these among them).
pub fn walk_on(switches: &hooks::Switches) -> bool {
    !on(switches) && switches.implementation("sim.native.objects.walk", true)
}

/// Whether the per-function port at `addr` stands down: a body installed
/// by either switch owns the point.
pub fn owns(switches: &hooks::Switches, addr: u32) -> bool {
    owns_in(on(switches), table(switches), addr) || (walk_on(switches) && walk_table(switches).owns(addr))
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

// The walk table makes the same hand-off: with the animation subsystem on,
// c7840 is its hook, and c60c0's call to it is a callout.
fn walk_table(switches: &hooks::Switches) -> &'static Table {
    if crate::sim_native::anim_on(switches) { &native_walk::WITHOUT_AIM } else { &native_walk::TABLE }
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
    if walk_on(b.switches()) {
        install_walk(b);
    }
    if installed {
        let arm = functions_on(b.switches());
        install_table_with(b, bodies, OBJECTS, |_| &[], arm);
        install_events(b);
        install_curve_arm(b);
        // The arm's proof is the map's: a new map earns it again.
        b.raw().observe_before(&points::CACHE_TAGS_LOAD, CURVE_ARM_RESET, |_| {
            engine::sim_objects::functions::parity::reset()
        });
        if arm {
            // So are the function arm's proof and its Rust-owned data.
            b.raw().observe_before(&points::CACHE_TAGS_LOAD, FUNCTION_ARM_RESET, |_| {
                engine::sim_objects::functions::arm::reset()
            });
        }
    }
}

/// The map-load observer that clears the curve arm's proof.
const CURVE_ARM_RESET: &str = "objects: reset the curve arm";
/// The map-load observer that clears the function arm and its data.
const FUNCTION_ARM_RESET: &str = "objects: reset the function arm";
/// The walk table's bodies ([`walk_table`]), each entry and site answered
/// by the Rust: [`objects::drive_with`] fuses every transfer between them
/// and counts each segment. A body outside the float
/// environment it needs (`bd090`, the type callbacks) is refused for that
/// call, counted, and the guest runs it; the caller's Rust frame resumes
/// at its site. The walks and the per-object updates never refuse, so
/// their hooks are marked [`hooks::HookRef::replaces_guest`] (`native_walk::REPLACES_GUEST`).
/// Finalize's body appends the tick's `ObjectDeleted` events, so the
/// buffer is emptied at the tick's entry ([`install_events_clear`]).
fn install_walk(b: &mut Builder) {
    install_events_clear(b);
    let table = walk_table(b.switches());
    for &body in table.bodies() {
        let point = body.point;
        let hook = b
            .raw()
            .implement(point, WALK_LABEL, move |cpu, ctx, pc| walk_segment(cpu, ctx, table, pc, body.sites))
            .resumes(body.sites)
            .context_only()
            .when_shared(move |entry| {
                table.guard(entry);
            });
        if native_walk::REPLACES_GUEST.contains(&point.base) {
            hook.replaces_guest();
        }
    }
}

/// One entry or site of a body of the walk table `table` at `pc`: the
/// Rust runs it and every body it reaches, and carries on after each true
/// callout the runtime makes in place ([`drive_in_place`]); a refused one
/// is the guest's. Each callout is counted ([`native_walk::callout`]).
fn walk_segment(
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    table: &'static Table,
    pc: u32,
    sites: &[u32],
) -> Result<TransferAbi, GuestFault> {
    drive_in_place(
        cpu,
        ctx,
        table,
        pc,
        sites.contains(&pc),
        |cpu, pc| {
            let ok = table.supports_at(pc, cpu);
            if !ok {
                native_walk::refused(pc, Refusal::FloatEnv);
            }
            ok
        },
        |cpu, g, pc| objects::drive_with(cpu, g, table, pc, native_walk::segment),
        native_walk::callout,
    )
}

/// Run `drive` from `pc` (an entry, or a site when `site`) under a memory
/// guard, then make the true callout it ends with in place
/// (`ExecCtx::call_inline`, docs/perf/sim-10x-callouts.md) and carry on
/// at its site, until a transfer leaves for the runner: a return or tail
/// jump out of the table, or a callout the runtime left to it (`note`
/// hears each callout's site and why). Where `admit` refuses the Rust at
/// `pc` the original runs, at its resume when `pc` is a site: as the
/// runner would have entered this hook at the site and been refused.
/// Only a call to a body site is made in place, so the runner's resume
/// registration is the one a fallback returns through.
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
            drive(cpu, &mut *mem, pc)?
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

/// The tag-function curve evaluator `13b390` (`FUNCTION_EVALUATE`): the
/// idiomatic kernel predicts every call, and
/// `engine::sim_objects::functions::parity::enter` decides who answers.
/// Until the arm has matched enough guest answers with no mismatch, the
/// guest answers and the kernel's prediction is compared at the return
/// (`ExecCtx::observe_guest_return`); after, the kernel answers and one
/// call in `AUDIT_EVERY` is still compared. A declined call (float
/// environment, curve kinds 9 and 10, a faulting read) is the guest's.
fn install_curve_arm(b: &mut Builder) {
    use engine::sim_objects::functions::parity::{self, Step};
    let point = &points::FUNCTION_EVALUATE;
    b.raw()
        .implement(point, OBJECTS, move |cpu, ctx, pc| {
            if pc == point.base {
                let step = {
                    let mem = ctx.memory();
                    parity::enter(cpu, &mem)
                };
                match step {
                    Step::Answered { target } => return Ok(TransferAbi::ret(target)),
                    Step::Audit(prediction) => {
                        // Compared at the guest's return; a full observer
                        // table leaves the call uncompared.
                        ctx.observe_guest_return(cpu, parity::ARGS, move |c, _| parity::compare(prediction, c));
                    }
                    Step::Guest => {}
                }
            }
            original(cpu, ctx, pc, false)
        })
        .context_only()
        .optional();
}

fn drive(
    cpu: &mut CpuAbi,
    g: &mut Guest,
    table: &Table,
    pc: u32,
    shadow: &crate::shadow::Shadow,
    arm: bool,
) -> objects::Result<TransferAbi> {
    let shadowed = shadow.any_enabled();
    if !shadowed && !arm {
        return objects::drive(cpu, g, table, pc);
    }
    objects::drive_with(cpu, g, table, pc, |body, cpu, g, pc| {
        let value = arm && body.point.base == points::OBJECT_FUNCTION_VALUE.base;
        if value && pc == body.point.base {
            use engine::sim_objects::functions::arm::{self as function_arm, Step};
            if let Step::Answered { target } = function_arm::enter(cpu, g) {
                return Ok(TransferAbi::ret(target));
            }
        }
        let transfer = if shadowed && SHADOW_POINTS.iter().any(|point| point.base == body.point.base) {
            shadow.run(body.point.name, cpu, g, pc, crate::shadow::game_tick, &mut |cpu, g| {
                body.segment(cpu, g, pc)
            })?
        } else {
            body.segment(cpu, g, pc)?
        };
        if value && transfer.kind == runtime::abi::RETURN {
            engine::sim_objects::functions::arm::returned(cpu, g);
        }
        Ok(transfer)
    })
}

/// The event observers. They write only the game thread's event buffer.
fn install_events(b: &mut Builder) {
    install_events_clear(b);
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

/// The tick's event buffer is emptied at `GAME_TICK`'s entry, so at tick
/// end it holds that tick's events.
fn install_events_clear(b: &mut Builder) {
    b.raw().observe_before(&points::GAME_TICK, EVENTS_CLEAR, |_| damage::tick_starts());
}

/// The observer [`install_events_clear`] adds.
const EVENTS_CLEAR: &str = "events: tick";

/// The chains [`install_walk`] adds to a plan: one per walk body and the
/// event clear at `GAME_TICK`.
#[cfg(test)]
pub(crate) fn walk_chains() -> usize {
    native_walk::TABLE.bodies().len() + 1
}

/// [`walk_chains`] with the animation subsystem on: its table lacks the
/// aim update `c7840`.
#[cfg(test)]
pub(crate) fn walk_chains_without_aim() -> usize {
    native_walk::WITHOUT_AIM.bodies().len() + 1
}

pub(crate) fn install_table(
    b: &mut Builder,
    table: &'static Table,
    label: &'static str,
    generated_sites: fn(u32) -> &'static [u32],
) {
    install_table_with(b, table, label, generated_sites, false);
}

/// [`install_table`], with the function value's arm when `arm`.
fn install_table_with(
    b: &mut Builder,
    table: &'static Table,
    label: &'static str,
    generated_sites: fn(u32) -> &'static [u32],
    arm: bool,
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
                    |cpu, g, pc| drive(cpu, g, table, pc, &crate::shadow::SHADOW, arm),
                    |_, _| {},
                )
            })
            .resumes(body.sites)
            .generated_resumes(generated_sites(point.base))
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
            let transfer = drive(&mut cpu, &mut g, &table, START.base, &shadow, false).unwrap();
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
        // The default-on animation manager arms install their own entries;
        // none of them is an object body. Only the walk table's are.
        for body in objects::BODIES {
            let implement = plan.chains.get(&body.point.base).and_then(|c| c.implement.as_ref());
            let walk = native_walk::TABLE.owns(body.point.base);
            assert_eq!(implement.map(|i| i.label), walk.then_some(WALK_LABEL), "off: {}", body.point.name);
        }
        // The manager keeps the "sim-native" section; it holds no bytes.
        assert!(plan.save_sections().iter().all(|(_, bytes)| bytes.is_empty()));

        let mut registry = Registry::new(Switches::defaults().with(&format!("impl.{OBJECTS}"), true));
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for body in objects::BODIES {
            let implement = plan.chains[&body.point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, OBJECTS, "{}", body.point.name);
            assert_eq!(implement.continuation_sites().0, body.sites);
        }
        // The bodies hold no state: no snapshot section.
        // The manager keeps the "sim-native" section; it holds no bytes.
        assert!(plan.save_sections().iter().all(|(_, bytes)| bytes.is_empty()));
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
        // The slice 1 bodies own the walks while the switch is on, and the
        // walk table owns them by default.
        let on = Switches::defaults().with(&format!("impl.{OBJECTS}"), true);
        assert!(owns(&on, points::OBJECTS_PRE_PHYSICS.base));
        assert!(owns(&Switches::defaults(), points::OBJECTS_PRE_PHYSICS.base));
        assert!(owns(&Switches::defaults(), points::UNIT_UPDATE.base));
        assert!(!owns(&Switches::defaults(), points::ITEM_UPDATE.base));
        assert!(!owns(&Switches::defaults().with("impl.sim.native", false), points::OBJECTS_PRE_PHYSICS.base));
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

    #[test]
    fn the_function_arm_is_off_unless_set_under_the_objects_switch() {
        let objects_on = || Switches::defaults().with(&format!("impl.{OBJECTS}"), true);
        assert!(!functions_on(&objects_on()), "the objects switch alone leaves it off");
        assert!(!functions_on(&Switches::defaults().with(FUNCTIONS, true)), "it needs the objects switch");
        assert!(functions_on(&objects_on().with(FUNCTIONS, true)));
        for (switches, armed) in [(objects_on(), false), (objects_on().with(FUNCTIONS, true), true)] {
            let mut registry = Registry::new(switches);
            registry.add(&SimNative::for_tests()).unwrap();
            let plan = registry.build(&Base).unwrap();
            let load = &plan.chains[&points::CACHE_TAGS_LOAD.base];
            assert_eq!(load.observe_before.iter().any(|h| h.label == FUNCTION_ARM_RESET), armed);
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
        let transfer = drive(&mut cpu, &mut g, &objects::TABLE, value, &shadow, true).unwrap();
        assert_eq!(transfer, TransferAbi::ret(0x0001_2345));
        assert_eq!(g.u32(OUT).unwrap(), 1.0f32.to_bits());
        let s = arm::status();
        assert_eq!((s.checked, s.matched, s.mismatched, s.answered, s.dropped), (1, 1, 0, 0, 0));
        arm::reset();
    }

    #[test]
    fn the_switch_installs_the_curve_evaluators_capture_arm() {
        // On: the curve evaluator's point is implemented by the arm, which
        // answers or hands the call to the original.
        let mut registry = Registry::new(Switches::defaults().with(&format!("impl.{OBJECTS}"), true));
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        let chain = &plan.chains[&points::FUNCTION_EVALUATE.base];
        let implement = chain.implement.as_ref().unwrap();
        assert_eq!(implement.label, OBJECTS);
        assert!(implement.optional);
        assert!(chain.observe_before.is_empty() && chain.observe_after.is_empty());
        assert!(implement.continuation_sites().0.is_empty());
        // The map load clears the arm's proof.
        let load = &plan.chains[&points::CACHE_TAGS_LOAD.base];
        assert!(load.observe_before.iter().any(|h| h.label == CURVE_ARM_RESET));
        // Off: nothing installs.
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        assert!(!plan.chains.contains_key(&points::FUNCTION_EVALUATE.base));
    }

    #[test]
    fn the_walk_table_is_on_by_default_and_gives_way_to_the_objects_switch() {
        let objects_on = || Switches::defaults().with(&format!("impl.{OBJECTS}"), true);
        assert!(walk_on(&Switches::defaults()));
        assert!(!walk_on(&objects_on()), "the objects switch installs every body instead");
        for off in [WALK, "impl.sim.native.objects", "impl.sim.native", "impl.sim"] {
            assert!(!walk_on(&Switches::defaults().with(off, false)), "-{off}");
        }
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for body in native_walk::TABLE.bodies() {
            let implement = plan.chains[&body.point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, WALK_LABEL, "{}", body.point.name);
            assert_eq!(implement.continuation_sites().0, body.sites);
            assert_eq!(
                implement.replaces_guest(),
                native_walk::REPLACES_GUEST.contains(&body.point.base),
                "{}",
                body.point.name
            );
            assert!(owns(&Switches::defaults(), body.point.base));
        }
        // Finalize never refuses; the multiplayer types' callbacks are the
        // walk table's and refuse outside their float environment.
        let finalize = plan.chains[&points::OBJECTS_FINALIZE.base].implement.as_ref().unwrap();
        assert!(finalize.label == WALK_LABEL && finalize.replaces_guest());
        for point in [
            &points::UNIT_UPDATE,
            &points::BIPED_UPDATE,
            &points::BIPED_POST_UPDATE,
            &points::WEAPON_UPDATE,
            &points::ITEM_UPDATE,
            &points::VEHICLE_UPDATE,
            &points::PROJECTILE_UPDATE,
            &points::SCENERY_UPDATE,
            &points::UNIT_CLAMP_DIRECTION,
            &points::UNIT_ANIMATION_STATE,
        ] {
            let implement = plan.chains[&point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, WALK_LABEL, "{}", point.name);
            assert!(!implement.replaces_guest(), "{} refuses outside its float environment", point.name);
        }
        // Finalize's deletion events are emptied at the tick's entry; the
        // combat observers are the objects switch's.
        let tick = &plan.chains[&points::GAME_TICK.base];
        assert!(tick.observe_before.iter().any(|h| h.label == EVENTS_CLEAR));
        let damage = plan.chains.get(&points::DAMAGE_REPORT.base);
        assert!(damage.is_none_or(|c| !c.observe_before.iter().any(|h| h.label == "events: damage")));
        // The other types, damage, functions and lights are the guest's.
        for point in [
            &points::DEVICE_UPDATE,
            &points::MACHINE_UPDATE,
            &points::OBJECT_DAMAGE_UPDATE,
            &points::OBJECT_FUNCTION_VALUE,
            &points::FUNCTION_EVALUATE,
        ] {
            let implement = plan.chains.get(&point.base).and_then(|c| c.implement.as_ref());
            assert!(implement.is_none(), "{}", point.name);
            assert!(!owns(&Switches::defaults(), point.base), "{}", point.name);
        }
        // No state.
        assert!(plan.save_sections().iter().all(|(_, bytes)| bytes.is_empty()));
        // Every walk entry has resume sites, so the lift keeps its body.
        let entries: std::collections::BTreeSet<u32> = native_walk::TABLE.bodies().iter().map(|b| b.point.base).collect();
        assert!(plan.replaced(&entries).is_empty());
        // With the objects switch on, every body is the objects switch's.
        let mut registry = Registry::new(objects_on());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for body in native_walk::TABLE.bodies() {
            let implement = plan.chains[&body.point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, OBJECTS, "{}", body.point.name);
        }
    }

    /// Outside the float environment, the root leaf `baf80` (float-free)
    /// is still the Rust's and counted, and `bd090` is refused to the
    /// guest, counted as `float_env`.
    #[test]
    fn a_walk_body_answers_outside_the_float_environment_and_bd090_is_refused() {
        use runtime::guest::Permissions;
        use runtime::{Dispatch, Runtime};
        use std::sync::atomic::{AtomicU64, Ordering};
        const STACK: u32 = 0x0057_0800;
        const CALLER: u32 = 0x0001_2345;
        static GUEST: AtomicU64 = AtomicU64::new(0);
        let pose = points::OBJECT_NODE_MATRICES.base;
        let root = points::OBJECT_ROOT.base;
        let mut d = Dispatch::default();
        d.register_generated_rust(pose, move |cpu, ctx, pc| {
            assert_eq!(pc, pose);
            GUEST.fetch_add(1, Ordering::Relaxed);
            ctx.memory().ret(cpu, 4)
        })
        .unwrap();
        d.register_generated_rust(root, |_, _, _| panic!("the root leaf never refuses")).unwrap();
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
        let before = (count(root), count(pose), GUEST.load(Ordering::Relaxed));
        let mut cpu = ftz();
        let t = walk_segment(&mut cpu, &mut ctx, &native_walk::TABLE, root, &[]).unwrap();
        assert_eq!(t, TransferAbi::ret(CALLER));
        assert_eq!((cpu.eax, cpu.esp), (u32::MAX, STACK + 4));
        assert_eq!(count(root).segments, before.0.segments + 1);
        let mut cpu = ftz();
        walk_segment(&mut cpu, &mut ctx, &native_walk::TABLE, pose, &[]).unwrap();
        assert_eq!(GUEST.load(Ordering::Relaxed), before.2 + 1);
        let after = count(pose);
        assert_eq!(after.refused[0], before.1.refused[0] + 1);
        assert_eq!(after.segments, before.1.segments);
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
