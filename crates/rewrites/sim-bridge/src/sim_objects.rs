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
//! The tick's `Damage` and `Kill` events (`sim_state::events`) are
//! recorded by observers at the damage report and the kill credit, and
//! the buffer is emptied at the tick's entry (docs/engine/object-update.md,
//! "Slice 8 body").
//!
//! [`crate::sim_native::SimNative`] calls [`install`]: the bodies hold no
//! state, so the feature's snapshot policy does not change.
use hooks::Builder;
use engine::points;
use engine::sim_objects::{self as objects, Table, damage};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

/// The switch, without its `impl.` prefix.
pub const OBJECTS: &str = hooks::install::SIM_OBJECTS;

pub fn on(switches: &hooks::Switches) -> bool {
    switches.implementation(OBJECTS, false)
}

/// Whether the per-function port at `addr` stands down: the switch is on
/// and a body owns the point.
pub fn owns(switches: &hooks::Switches, addr: u32) -> bool {
    owns_in(on(switches), &objects::TABLE, addr)
}

fn owns_in(on: bool, table: &Table, addr: u32) -> bool {
    on && table.owns(addr)
}

pub(crate) fn install(b: &mut Builder) {
    install_table(b, &objects::TABLE);
    install_events(b);
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
                    return objects::drive(cpu, &mut mem, table, pc);
                }
                original(cpu, ctx, pc, site)
            })
            .resumes(body.sites)
            .context_only();
    }
}

/// The original at an entry, or its resume at a site.
fn original(cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32, site: bool) -> Result<TransferAbi, GuestFault> {
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
            assert_eq!(implement.resume_sites, body.sites);
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
            assert_eq!(implement.resume_sites, body.sites);
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
        hooks::install::guard_shared_bodies(&T, plan.chains.values());
        assert!(T.guarded(points::OBJECT_DAMAGE_UPDATE.base));
        assert!(!T.guarded(points::OBJECTS_PRE_PHYSICS.base));

        // Alone, nothing is guarded.
        static U: std::sync::LazyLock<Table> =
            std::sync::LazyLock::new(|| Table::new(DAMAGE_BODIES.iter()).unwrap());
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&Installs(&U)).unwrap();
        let plan = registry.build(&Base).unwrap();
        hooks::install::guard_shared_bodies(&U, plan.chains.values());
        assert!(!U.guarded(points::OBJECT_DAMAGE_UPDATE.base));
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
