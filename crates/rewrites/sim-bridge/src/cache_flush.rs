//! The resource caches' flushes and an owner's unload in Rust
//! (`engine::native_cache_flush`), with no push-buffer kick. Always
//! installed: there is no switch and no guest arm. Each body owns its
//! entry and its return sites; a transfer anywhere else is the
//! original's by construction (a site the body does not own).

use engine::native_cache_flush::TABLE;
use guest_layout::points::cache_wait::WAITS;
use hooks::{Builder, Flow};
use runtime::TransferAbi;

/// The label the bodies install under.
pub const LABEL: &str = "cache flush";
/// The label the allocations' wait installs under.
pub const WAIT_LABEL: &str = "cache wait";

pub(crate) fn install(b: &mut Builder) {
    for &body in TABLE.bodies() {
        let point = body.point;
        b.raw()
            .implement(point, LABEL, move |cpu, ctx, pc| {
                let site = pc != point.base && body.sites.contains(&pc);
                if pc != point.base && !site {
                    return crate::sim_objects::original(cpu, ctx, pc, false);
                }
                let mut mem = ctx.memory();
                body.segment(cpu, &mut mem, pc)
            })
            .resumes(body.sites)
            .context_only();
    }
    // The blocking allocations' wait: the GPU kick and the fence peek are
    // gone; the yield stays (`guest_layout::points::cache_wait`).
    for wait in &WAITS {
        b.raw()
            .before(&wait.site, WAIT_LABEL, move |_| {
                std::thread::yield_now();
                Ok(Flow::Return(TransferAbi::resume(wait.resume)))
            })
            .context_only();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_native::SimNative;
    use hooks::{Base, Registry, Switches};

    #[test]
    fn every_flush_installs_with_its_sites() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for body in TABLE.bodies() {
            let implement = plan.chains[&body.point.base].implement.as_ref().unwrap();
            assert_eq!(implement.label, LABEL, "{}", body.point.name);
            assert_eq!(implement.continuation_sites().0, body.sites, "{}", body.point.name);
        }
    }
}
