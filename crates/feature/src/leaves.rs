//! `engine-leaves`: the Rust implementations of 22 guest functions
//! (`engine::abi::OVERRIDES`: datum arrays, tag lookup, locale lookup)
//! placed through the plan as implementations, behind `impl.engine`
//! (off by default: nothing installed them before).
//!
//! Through the plan they share points with other features' hooks instead
//! of displacing them: the locale lookup is implemented here while the
//! button layouts serve their own strings from a `before` on the same
//! point.

use std::sync::{Arc, OnceLock};

use engine::points::{self, Point, Sig};

use crate::{BuildError, Builder, Feature, Manifest};

pub struct EngineLeaves;

/// A point for each adapter: the table's own where it names the address,
/// else one made from the adapter's name (the abi table is the record for
/// those).
fn leaf_points() -> &'static [&'static Point] {
    static POINTS: OnceLock<Vec<&'static Point>> = OnceLock::new();
    POINTS.get_or_init(|| {
        engine::abi::OVERRIDES
            .iter()
            .map(|o| {
                points::by_base(o.addr).unwrap_or_else(|| {
                    Box::leak(Box::new(Point::new(
                        o.name,
                        o.addr,
                        Sig::Opaque,
                        "docs/engine/README.md",
                    )))
                })
            })
            .collect()
    })
}

impl Feature for EngineLeaves {
    fn manifest(&self) -> Manifest {
        Manifest::new("engine-leaves", "Rust datum, tag and locale functions")
            .doc("docs/engine/README.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if !b.switches().implementation("engine", false) {
            return Ok(());
        }
        for (o, point) in engine::abi::OVERRIDES.iter().zip(leaf_points()) {
            let o = Arc::new(o);
            b.raw().implement(point, o.name, move |cpu, ctx, _| {
                let mut guest = ctx.memory();
                engine::runtime_glue::invoke(&o, cpu, &mut guest)
            });
        }
        Ok(())
    }
}
