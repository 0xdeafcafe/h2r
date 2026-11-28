//! Live comparisons for the HaloScript VM. The guest produces the game result.
//! Each pair of observers holds its predictions only until the guest returns.
use engine::points::Point;
use hooks::Builder;
use runtime::{CpuAbi, Guest};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub matched: u64,
    pub mismatched: u64,
    pub declined: u64,
    pub first: Option<String>,
}

pub(super) struct Counter {
    name: &'static str,
    totals: Mutex<Stats>,
}

impl Counter {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            totals: Mutex::new(Stats {
                matched: 0,
                mismatched: 0,
                declined: 0,
                first: None,
            }),
        }
    }

    pub(super) fn matched(&self) {
        self.totals.lock().unwrap().matched += 1;
    }

    pub(super) fn declined(&self) {
        self.totals.lock().unwrap().declined += 1;
    }

    pub(super) fn mismatch(&self, detail: impl FnOnce() -> String) {
        let mut stats = self.totals.lock().unwrap();
        stats.mismatched += 1;
        stats.first.get_or_insert_with(detail);
    }
}

pub(super) static THREAD: Counter = Counter::new("thread");
pub(super) static EVALUATOR: Counter = Counter::new("evaluator");
pub(super) static GLOBALS: Counter = Counter::new("globals");
pub(super) static GLOBAL_STORE: Counter = Counter::new("global_store");
pub(super) static DISPATCH: Counter = Counter::new("dispatch");

/// Process totals. A declined call supplies no evidence of parity.
pub fn stats() -> [(&'static str, Stats); 5] {
    [
        &THREAD,
        &EVALUATOR,
        &GLOBALS,
        &GLOBAL_STORE,
        &DISPATCH,
    ]
    .map(|counter| (counter.name, counter.totals.lock().unwrap().clone()))
}

/// Keep nested predictions separate by runtime, guest thread and stack address.
/// Entering at the same or a higher stack address abandons an unreturned call.
/// A declined inner call never consumes its caller's prediction.
pub(super) fn observe<T: Send + 'static>(
    b: &mut Builder,
    point: &'static Point,
    counter: &'static Counter,
    cleanup: u32,
    plan: impl Fn(&CpuAbi, &Guest) -> Option<T> + Send + Sync + 'static,
    compare: fn(&T, &CpuAbi, &Guest) -> Option<String>,
) {
    let pending = Arc::new(Mutex::new(BTreeMap::<(usize, u32, u32), T>::new()));
    let before = Arc::clone(&pending);
    b.raw()
        .observe_before(point, counter.name, move |o| {
            let id = Arc::as_ptr(o.runtime) as usize;
            let key = (id, o.thread_id, o.cpu.esp);
            let predicted = {
                let mem = o.guest();
                if mem.has_trap() {
                    None
                } else {
                    plan(o.cpu, &mem)
                }
            };
            let mut calls = before.lock().unwrap();
            while let Some((&stale, _)) = calls.range((id, o.thread_id, 0)..=key).next() {
                calls.remove(&stale);
                counter.declined();
            }
            if let Some(value) = predicted {
                calls.insert(key, value);
            } else {
                counter.declined();
            }
        })
        .optional();

    b.raw()
        .observe_after(point, counter.name, move |o| {
            let entry_sp = o.cpu.esp.wrapping_sub(4 + cleanup);
            let key = (Arc::as_ptr(o.runtime) as usize, o.thread_id, entry_sp);
            let Some(value) = pending.lock().unwrap().remove(&key) else {
                return;
            };
            let mem = o.guest();
            if mem.has_trap() {
                counter.declined();
            } else if let Some(difference) = compare(&value, o.cpu, &mem) {
                counter.mismatch(|| {
                    format!(
                        "{} thread {} esp {entry_sp:08x}: {difference}",
                        point.name, o.thread_id
                    )
                });
            } else {
                counter.matched();
            }
        })
        .optional();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::points;
    use hooks::{Base, Obs, Registry, Run, Switches};

    #[test]
    fn nested_checks_retire_only_the_returning_threads_prediction() {
        static COUNTER: Counter = Counter::new("test");
        let point = &points::HS_EVALUATE;
        let mut registry = Registry::new(Switches::default());
        registry
            .add(&hooks::from_fn("test", move |b| {
                observe(
                    b,
                    point,
                    &COUNTER,
                    4,
                    |cpu, _| (cpu.eax != 0).then_some(cpu.eax),
                    |value, cpu, _| (*value != cpu.eax).then(|| format!("expected {value}")),
                );
                Ok(())
            }))
            .unwrap();
        let plan = registry.build(&Base).unwrap();
        let chain = &plan.chains[&point.base];
        let Run::ObserveBefore(before) = &chain.observe_before[0].run else {
            unreachable!()
        };
        let Run::ObserveAfter(after) = &chain.observe_after[0].run else {
            unreachable!()
        };
        let rt = runtime::Runtime::new(Guest::new(), runtime::Dispatch::default());
        let call = |enter, thread_id, esp, eax| {
            let cpu = CpuAbi {
                esp,
                eax,
                ..CpuAbi::default()
            };
            let obs = Obs {
                addr: point.base,
                point,
                thread_id,
                cpu: &cpu,
                runtime: &rt,
            };
            if enter { before(&obs) } else { after(&obs) }
        };
        call(true, 1, 0x1000, 1);
        call(true, 2, 0x1000, 2);
        call(true, 1, 0xff0, 0); // declined nested call
        call(false, 1, 0xff8, 5);
        call(true, 1, 0xff0, 3); // unreturned nested call
        call(true, 1, 0xff0, 4);
        call(false, 2, 0x1008, 2);
        call(false, 1, 0xff8, 9); // first difference
        call(false, 1, 0x1008, 1);
        let stats = COUNTER.totals.lock().unwrap();
        assert_eq!((stats.matched, stats.declined, stats.mismatched), (2, 2, 1));
        assert!(stats.first.as_ref().unwrap().contains("expected 4"));
    }
}
