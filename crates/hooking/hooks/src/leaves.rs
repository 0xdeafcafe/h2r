//! `engine-leaves`: the Rust implementations of 22 guest functions
//! (`engine::abi::OVERRIDES`: datum arrays, tag lookup, locale lookup)
//! placed through the plan as implementations. Each is switched by
//! `impl.engine.<group>.<name>`, falling back to `impl.engine.<group>` and
//! `impl.engine`; groups are `locale`, `tags` and `datums`, all on by
//! default.
//!
//! The Rust `locale_string_id_find` serves the plan's string sources
//! (`crate::locale`), so with it on our strings need no hook.

use std::sync::OnceLock;

use engine::points::{self, Point, Sig};

use crate::{BuildError, Builder, Feature, Manifest};

pub struct EngineLeaves;

/// The switch group of an adapter: `impl.engine.<group>[.<name>]`.
pub fn group(name: &str) -> &'static str {
    if name.starts_with("locale_") {
        "locale"
    } else if name.starts_with("tag_") {
        "tags"
    } else {
        "datums"
    }
}

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
        for (o, point) in engine::abi::OVERRIDES.iter().zip(leaf_points()) {
            let g = group(o.name);
            let key = format!("engine.{g}.{}", o.name);
            if !b.switches().implementation(&key, true) {
                continue;
            }
            if point.name == points::LOCALE_FIND.name {
                // The Rust lookup asks our string sources first.
                let sources = b.locale().serve();
                b.raw()
                    .implement(point, o.name, move |cpu, ctx, _| {
                        let mut guest = ctx.memory();
                        if let Some(a) = sources.find(cpu.edi) {
                            cpu.eax = a;
                            return guest.ret(cpu, 0);
                        }
                        engine::runtime_glue::invoke(o, cpu, &mut guest)
                    })
                    .context_only();
                continue;
            }
            b.raw()
                .implement(point, o.name, move |cpu, ctx, _| {
                    let mut guest = ctx.memory();
                    engine::runtime_glue::invoke(o, cpu, &mut guest)
                })
                .context_only();
        }
        // Every adapter is a function of its registers and the memory of
        // the context it runs on (the string sources answer from state
        // fixed at boot, or read only), so each runs on the pipelined
        // render thread too.
        // Pure functions of guest memory; the string sources are fixed at
        // boot.
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
