//! Which calls can stay in the current manager invocation. Missing image
//! selectors, shared hooks and disabled or replaced overrides all require
//! a transfer through the runtime.
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use runtime::Runtime;

use super::{AnimBody, AnimRun};

struct Row {
    name: String,
    shared: AtomicBool,
}

struct Snapshot {
    runtime: Weak<Runtime>,
    generation: u64,
    entries: Arc<[u32]>,
}

pub(super) struct Fusion {
    rows: BTreeMap<u32, Row>,
    cache: Mutex<Option<Snapshot>>,
}

impl Fusion {
    pub(super) fn new(exports: &[AnimBody]) -> Self {
        let rows = exports
            .iter()
            .filter(|b| matches!(b.run, AnimRun::Manager))
            .map(|b| {
                // The sole implementation's override name (hooks::install::place).
                let name = format!("plan:{}[sim-native]", b.point.name);
                (
                    b.point.base,
                    Row {
                        name,
                        shared: AtomicBool::new(false),
                    },
                )
            })
            .collect();
        Self {
            rows,
            cache: Mutex::new(None),
        }
    }

    pub(super) fn guard(&self, entry: u32) {
        if let Some(row) = self.rows.get(&entry) {
            row.shared.store(true, Ordering::Relaxed);
        }
    }

    /// Snapshot the installed handlers before taking the memory guard.
    /// A fork can share this plan but has its own set of active overrides.
    pub(super) fn available(&self, rt: &Arc<Runtime>) -> impl Fn(u32) -> bool + use<> {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        let current = cache.as_ref().is_some_and(|cached| {
            cached.runtime.as_ptr() == Arc::as_ptr(rt)
                && cached.generation == rt.dispatch_generation()
        });
        if !current {
            *cache = Some(rt.read_dispatch(|dispatch| {
                let entries = self
                    .rows
                    .iter()
                    .filter_map(|(&entry, row)| {
                        (!row.shared.load(Ordering::Relaxed)
                            && dispatch.override_name(entry) == Some(row.name.as_str())
                            && dispatch.override_handler(entry).is_some())
                        .then_some(entry)
                    })
                    .collect();
                Snapshot {
                    runtime: Arc::downgrade(rt),
                    generation: rt.dispatch_generation(),
                    entries,
                }
            }));
        }
        let snapshot = cache.as_ref().expect("current dispatch snapshot");
        let generation = snapshot.generation;
        let entries = Arc::clone(&snapshot.entries);
        let rt = Arc::clone(rt);
        move |entry| rt.dispatch_generation() == generation && entries.binary_search(&entry).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::super::{ANIM_INVALIDATE, SimNative};
    use engine::{points::Point, sim_anim::manager};
    use hooks::{Base, BuildError, Builder, Feature, Manifest, Registry, SnapshotPolicy, Switches};
    use runtime::{CpuAbi, Dispatch, ExecCtx, Guest, Runtime, TransferAbi, guest::Permissions};

    // Handle lookup calls selector setup, which calls channel selection.
    const LOOKUP: u32 = 0x1cb490;
    const SETUP: u32 = 0x1cb410;
    const SELECT: u32 = 0x1c6470;

    struct Observe(&'static Point);
    impl Feature for Observe {
        fn manifest(&self) -> Manifest {
            Manifest::new("manager-observer-test", "observe a manager call")
        }
        fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
            b.snapshot(SnapshotPolicy::Stateless);
            b.raw().observe_before(self.0, "selector setup", |_| {});
            Ok(())
        }
    }

    #[test]
    fn lookup_fuses_only_installed_unshared_enabled_setup() {
        for shared in [true, false] {
            let mut dispatch = Dispatch::default();
            for (point, _) in ANIM_INVALIDATE {
                dispatch
                    .register_generated_rust(point.base, |_, _, _| panic!("observer only"))
                    .unwrap();
            }
            for body in manager::bodies().filter(|b| matches!(b.entry, LOOKUP | SETUP)) {
                dispatch
                    .register_generated_rust(body.entry, |_, _, _| panic!("unexpected decline"))
                    .unwrap();
                for &site in body.sites {
                    dispatch.register_resume(site, body.entry, site).unwrap();
                }
            }
            let mut g = Guest::new();
            g.map_zeroed(0x1cb000, 0x1000, Permissions::RX).unwrap();
            g.map_zeroed(0x30000, 0x1000, Permissions::RW).unwrap();
            let rt = Runtime::new(g, dispatch);
            let mut r = Registry::new(Switches::defaults());
            r.add(&SimNative::new(&rt)).unwrap();
            if shared {
                let setup = manager::bodies().find(|b| b.entry == SETUP).unwrap();
                r.add(&Observe(&setup.point)).unwrap();
            }
            let mut plan = r.build(&Base).unwrap();
            super::only_anim(&mut plan);
            plan.install(&rt).unwrap();
            let mut ctx = ExecCtx::new(rt.clone(), 1);
            let lookup = rt.read_dispatch(|d| d.override_handler(LOOKUP)).unwrap();
            for enabled in [true, false, true] {
                assert!(rt.edit_dispatch(|d| d.set_enabled(SETUP, enabled)));
                let mut cpu = CpuAbi {
                    esp: 0x30400,
                    eax: 0x30800,
                    ..CpuAbi::default()
                };
                // The synthetic manager's graph is 0, a present handle.
                let transfer = lookup.invoke(&mut cpu, &mut ctx, LOOKUP);
                let expected = if enabled && !shared {
                    TransferAbi::call(SELECT, 0x1cb45a)
                } else {
                    TransferAbi::call(SETUP, 0x1cb4b1)
                };
                assert_eq!(transfer, expected, "shared={shared} enabled={enabled}");
                assert_eq!(ctx.memory().u32(cpu.esp).unwrap(), expected.resume_pc);
            }
        }
    }
}
