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
//! [`crate::sim_native::SimNative`] calls [`install`]: the bodies hold no
//! state, so the feature's snapshot policy does not change.
use hooks::Builder;
use engine::sim_objects::{self as objects, Table};
use runtime::{CpuAbi, ExecCtx, GuestFault, TransferAbi};

/// The switch, without its `impl.` prefix.
pub const OBJECTS: &str = "sim.native.objects";

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
    fn the_switch_is_off_by_default_and_installs_nothing_with_zero_bodies() {
        assert!(!on(&Switches::defaults()));
        assert!(on(&Switches::defaults().with(&format!("impl.{OBJECTS}"), true)));
        // `-impl.sim.native` turns every subsystem off.
        assert!(!on(&Switches::defaults().with("impl.sim.native", false)));
        assert!(objects::TABLE.is_empty());
        for switches in [
            Switches::defaults(),
            Switches::defaults().with(&format!("impl.{OBJECTS}"), true),
        ] {
            let mut registry = Registry::new(switches);
            registry.add(&SimNative::for_tests()).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert!(plan.chains.is_empty());
            assert!(plan.save_sections().is_empty());
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
        // With zero bodies nothing stands down.
        let on = Switches::defaults().with(&format!("impl.{OBJECTS}"), true);
        assert!(!owns(&on, points::OBJECTS_PRE_PHYSICS.base));
    }

    /// A feature that installs a given table, so the installer is tested
    /// before slice 1 adds a body to `BODIES`.
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
