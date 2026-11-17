//! Can the machine be snapshotted right now (docs/runtime/snapshots.md,
//! "When a snapshot can be taken")? Asked between two `Machine::run`
//! slices, when no guest code is on the host stack. Every guest thread is
//! then a CPU state plus a transfer, which is plain data, unless Rust state
//! is waiting on guest code: a callback continuation, an interrupt callback
//! in service, a DPC, a periodic handler that is running. Those are the
//! blockers this reports, each one, so a probe can count them.
use crate::threads::{DPC_THREAD_ID, Machine};
use runtime::ContinuationInfo;

/// The continuation every created thread holds for its whole life: it
/// terminates the thread when the start routine returns. It captures only
/// the thread id, so a load can make it again (at the same token): it does
/// not block a snapshot.
pub const THREAD_RETURN: &str = "kernel: thread return";
/// The continuation of a DPC routine that has not returned.
pub const DPC_RETURN: &str = "kernel: DPC return";

/// One reason a snapshot cannot be taken now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// Rust code waits for a guest call to return on this thread.
    Continuation {
        thread: u32,
        origin: &'static str,
        target: u32,
    },
    /// An interrupt callback runs on this thread; the interrupted CPU state
    /// is held host-side until it returns.
    InterruptInService { thread: u32 },
    /// A fault waits to end this thread's next run.
    FaultPending { thread: u32 },
    /// Interrupts raised and not yet delivered (host `GuestInterrupt`s).
    QueuedInterrupts(usize),
    /// Completions queued by host producers, not yet applied.
    DeferredSignals(usize),
    /// A periodic source's handler is running.
    PeriodicRunning(&'static str),
    /// The machine stopped (fault, missing export, exit).
    Stopped,
    /// Not the machine's own: a feature or host subsystem reported it.
    Host { owner: String, why: String },
}

impl Blocker {
    /// The same blocker on any thread counts as one kind: what a probe
    /// totals by.
    pub fn key(&self) -> String {
        match self {
            Blocker::Continuation { origin, target, .. } => {
                format!("continuation {origin} -> {target:#010x}")
            }
            Blocker::InterruptInService { .. } => "interrupt in service".into(),
            Blocker::FaultPending { .. } => "fault pending".into(),
            Blocker::QueuedInterrupts(_) => "queued interrupts".into(),
            Blocker::DeferredSignals(_) => "deferred signals".into(),
            Blocker::PeriodicRunning(name) => format!("periodic {name} running"),
            Blocker::Stopped => "machine stopped".into(),
            Blocker::Host { owner, why } => format!("{owner}: {why}"),
        }
    }
    /// True for blockers the machine itself holds, as opposed to a
    /// feature's or device's report.
    pub fn is_machine(&self) -> bool {
        !matches!(self, Blocker::Host { .. })
    }
}

impl std::fmt::Display for Blocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Blocker::Continuation { thread, .. }
            | Blocker::InterruptInService { thread }
            | Blocker::FaultPending { thread } => write!(f, "thread {thread}: {}", self.key()),
            Blocker::QueuedInterrupts(n) | Blocker::DeferredSignals(n) => {
                write!(f, "{} ({n})", self.key())
            }
            _ => f.write_str(&self.key()),
        }
    }
}

/// What [`Machine::quiescent`] found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Quiescence {
    pub blockers: Vec<Blocker>,
    /// Continuations a load can rebuild ([`THREAD_RETURN`]), by thread.
    pub rebuildable: Vec<(u32, ContinuationInfo)>,
}

impl Quiescence {
    pub fn is_quiescent(&self) -> bool {
        self.blockers.is_empty()
    }
}

impl Machine {
    /// Every reason the machine could not be snapshotted now. Meaningful
    /// only between slices (not from inside a handler, where the running
    /// thread's host stack holds guest frames). Takes the kernel lock.
    pub fn quiescent(&self) -> Quiescence {
        let mut q = Quiescence::default();
        if self.is_stopped() {
            q.blockers.push(Blocker::Stopped);
        }
        let contexts = self
            .threads()
            .map(|t| (t.id, &t.ctx))
            .chain(self.dpc_thread().map(|t| (DPC_THREAD_ID, &t.ctx)));
        for (thread, ctx) in contexts {
            for c in ctx.live_continuations() {
                if c.origin == THREAD_RETURN && thread != DPC_THREAD_ID {
                    q.rebuildable.push((thread, c));
                } else {
                    q.blockers.push(Blocker::Continuation {
                        thread,
                        origin: c.origin,
                        target: c.target,
                    });
                }
            }
            if ctx.interrupt_in_service() {
                q.blockers.push(Blocker::InterruptInService { thread });
            }
            if ctx.fault_pending() {
                q.blockers.push(Blocker::FaultPending { thread });
            }
        }
        let queued = self.runtime.pending_interrupts();
        if queued != 0 {
            q.blockers.push(Blocker::QueuedInterrupts(queued));
        }
        let deferred = self
            .kernel
            .deferred
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .len();
        if deferred != 0 {
            q.blockers.push(Blocker::DeferredSignals(deferred));
        }
        for p in self.kernel.lock().periodic.values() {
            if p.handler.is_none() {
                q.blockers.push(Blocker::PeriodicRunning(p.name));
            }
        }
        q
    }
}

/// The prefix of every probe line, for grep.
pub const PROBE_PREFIX: &str = "snapshot-probe:";

/// Counts blockers over many checks, for the opt-in snapshot probe
/// (`H2_SNAPSHOT_PROBE`, docs/runtime/snapshots.md "Quiescence probe"):
/// how often the machine could have been saved, and what stopped it.
/// Lines it returns are rate-limited: one when the set of machine blockers
/// changes (at most `max_change_lines` of them), and a progress line every
/// `progress_every` checks.
#[derive(Debug, Clone)]
pub struct Tally {
    pub checks: u64,
    /// No machine blocker (features and host subsystems aside).
    pub machine_quiescent: u64,
    /// No blocker at all.
    pub fully_quiescent: u64,
    machine: std::collections::BTreeMap<String, u64>,
    host: std::collections::BTreeMap<String, u64>,
    last: Option<Vec<String>>,
    change_lines: u64,
    pub max_change_lines: u64,
    pub progress_every: u64,
}

impl Default for Tally {
    fn default() -> Self {
        Self {
            checks: 0,
            machine_quiescent: 0,
            fully_quiescent: 0,
            machine: Default::default(),
            host: Default::default(),
            last: None,
            change_lines: 0,
            max_change_lines: 200,
            progress_every: 1800,
        }
    }
}

fn percent(n: u64, of: u64) -> f64 {
    if of == 0 {
        0.0
    } else {
        n as f64 * 100.0 / of as f64
    }
}

impl Tally {
    /// A blocker's counting key; `describe` names a continuation's target
    /// (a feature chain's shadow entry, say).
    pub fn key_of(b: &Blocker, describe: &dyn Fn(u32) -> Option<String>) -> String {
        match b {
            Blocker::Continuation { target, .. } => match describe(*target) {
                Some(d) => format!("{} ({d})", b.key()),
                None => b.key(),
            },
            _ => b.key(),
        }
    }

    /// Record one check at `tick`; returns the lines to print.
    pub fn observe(
        &mut self,
        tick: u64,
        blockers: &[Blocker],
        describe: &dyn Fn(u32) -> Option<String>,
    ) -> Vec<String> {
        self.checks += 1;
        let mut machine: Vec<String> = Vec::new();
        let mut host = 0usize;
        for b in blockers {
            let key = Self::key_of(b, describe);
            if b.is_machine() {
                machine.push(match b {
                    Blocker::Continuation { thread, .. }
                    | Blocker::InterruptInService { thread }
                    | Blocker::FaultPending { thread } => format!("thread {thread}: {key}"),
                    _ => b.to_string(),
                });
                *self.machine.entry(key).or_default() += 1;
            } else {
                host += 1;
                *self.host.entry(key).or_default() += 1;
            }
        }
        machine.sort();
        machine.dedup();
        if machine.is_empty() {
            self.machine_quiescent += 1;
            if host == 0 {
                self.fully_quiescent += 1;
            }
        }
        let mut out = Vec::new();
        if self.last.as_ref() != Some(&machine) && self.change_lines < self.max_change_lines {
            self.change_lines += 1;
            let state = if machine.is_empty() {
                "machine=quiescent".to_string()
            } else {
                format!("machine=blocked [{}]", machine.join("; "))
            };
            out.push(format!(
                "{PROBE_PREFIX} tick={tick} {state} host_blockers={host}"
            ));
            if self.change_lines == self.max_change_lines {
                out.push(format!(
                    "{PROBE_PREFIX} change lines capped at {}; progress lines continue",
                    self.max_change_lines
                ));
            }
        }
        self.last = Some(machine);
        if self.progress_every != 0 && self.checks.is_multiple_of(self.progress_every) {
            out.push(format!(
                "{PROBE_PREFIX} progress tick={tick} {}",
                self.counts()
            ));
        }
        out
    }

    fn counts(&self) -> String {
        format!(
            "checked={} machine_quiescent={} ({:.1}%) fully_quiescent={} ({:.1}%)",
            self.checks,
            self.machine_quiescent,
            percent(self.machine_quiescent, self.checks),
            self.fully_quiescent,
            percent(self.fully_quiescent, self.checks),
        )
    }

    /// The closing report: totals, then the `top` machine blockers and the
    /// `top` host ones by how many times they were seen.
    pub fn summary(&self, top: usize) -> Vec<String> {
        let mut out = vec![format!("{PROBE_PREFIX} summary {}", self.counts())];
        for (kind, map) in [("machine", &self.machine), ("host", &self.host)] {
            let mut v: Vec<_> = map.iter().collect();
            v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            for (key, n) in v.iter().take(top) {
                out.push(format!(
                    "{PROBE_PREFIX} top {kind} {n} ({:.1}%) {key}",
                    percent(**n, self.checks)
                ));
            }
            if v.len() > top {
                out.push(format!(
                    "{PROBE_PREFIX} top {kind} ... {} more kinds",
                    v.len() - top
                ));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Kernel};

    fn cont(thread: u32, target: u32) -> Blocker {
        Blocker::Continuation {
            thread,
            origin: "feature: after hooks",
            target,
        }
    }
    fn host(owner: &str) -> Blocker {
        Blocker::Host {
            owner: owner.into(),
            why: "not audited for snapshots".into(),
        }
    }

    #[test]
    fn the_tally_counts_checks_and_prints_only_changes() {
        let mut t = Tally {
            progress_every: 3,
            ..Tally::default()
        };
        let none = |_: u32| None;
        let named = |a: u32| (a == 0x10).then(|| "after hooks on main_loop".to_string());
        assert_eq!(
            t.observe(1, &[host("feature x")], &none),
            vec!["snapshot-probe: tick=1 machine=quiescent host_blockers=1"]
        );
        // Same machine state: nothing printed.
        assert!(t.observe(2, &[host("feature x")], &none).is_empty());
        assert_eq!(
            t.observe(3, &[cont(1, 0x10), cont(2, 0x10)], &named),
            vec![
                "snapshot-probe: tick=3 machine=blocked [thread 1: continuation feature: after hooks -> 0x00000010 (after hooks on main_loop); thread 2: continuation feature: after hooks -> 0x00000010 (after hooks on main_loop)] host_blockers=0",
                "snapshot-probe: progress tick=3 checked=3 machine_quiescent=2 (66.7%) fully_quiescent=0 (0.0%)",
            ]
        );
        t.observe(4, &[], &none);
        assert_eq!(
            (t.checks, t.machine_quiescent, t.fully_quiescent),
            (4, 3, 1)
        );
        let summary = t.summary(5);
        assert_eq!(
            summary[0],
            "snapshot-probe: summary checked=4 machine_quiescent=3 (75.0%) fully_quiescent=1 (25.0%)"
        );
        // Two threads blocked by the same kind count twice in one check.
        assert_eq!(
            summary[1],
            "snapshot-probe: top machine 2 (50.0%) continuation feature: after hooks -> 0x00000010 (after hooks on main_loop)"
        );
        assert_eq!(
            summary[2],
            "snapshot-probe: top host 2 (50.0%) feature x: not audited for snapshots"
        );
        assert!(summary.iter().all(|l| l.starts_with(PROBE_PREFIX)));
    }

    #[test]
    fn change_lines_are_capped() {
        let mut t = Tally {
            max_change_lines: 2,
            progress_every: 0,
            ..Tally::default()
        };
        let none = |_: u32| None;
        let mut printed = 0;
        for tick in 0..10 {
            let b = if tick % 2 == 0 {
                vec![]
            } else {
                vec![Blocker::DeferredSignals(1)]
            };
            printed += t.observe(tick, &b, &none).len();
        }
        // Two changes and the cap notice.
        assert_eq!(printed, 3);
        assert_eq!(t.checks, 10);
        assert_eq!(
            t.summary(1)[1],
            "snapshot-probe: top machine 5 (50.0%) deferred signals"
        );
    }
    use runtime::{CpuAbi, Dispatch, Guest, Runtime, TransferAbi, guest::Permissions};
    use std::sync::Arc;

    const STACK_BOTTOM: u32 = 0x6fff_0000;
    const STACK_TOP: u32 = 0x7000_0000;

    fn machine() -> Machine {
        let mut g = Guest::new();
        g.map_zeroed(0x40_0000, 0x1_0000, Permissions::RWX).unwrap();
        g.map_zeroed(STACK_BOTTOM, STACK_TOP - STACK_BOTTOM, Permissions::RW)
            .unwrap();
        let rt = Runtime::new(g, Dispatch::default());
        let kernel = Kernel::install(&rt, Config::default()).unwrap();
        let cpu = CpuAbi {
            eip: 0x40_0000,
            esp: STACK_TOP - 4,
            ..CpuAbi::xbox_thread()
        };
        rt.memory()
            .set_u32(cpu.esp, runtime::exec::HARNESS_EXIT)
            .unwrap();
        Machine::new(rt, kernel, cpu, STACK_BOTTOM, STACK_TOP).unwrap()
    }

    /// Push a continuation on thread 1, as an override calling the guest
    /// would.
    fn callback(m: &mut Machine, origin: Option<&'static str>) -> u32 {
        let rt = Arc::clone(&m.runtime);
        let t = m.thread_mut(1).unwrap();
        let mut cpu = t.cpu;
        let done = |_: &mut CpuAbi, _: &mut runtime::ExecCtx| Ok(TransferAbi::stop(0));
        let transfer = match origin {
            Some(o) => t
                .ctx
                .call_guest_named(&mut cpu, &mut rt.memory(), 0x40_0100, o, done),
            None => t
                .ctx
                .call_guest(&mut cpu, &mut rt.memory(), 0x40_0100, done),
        }
        .unwrap();
        t.cpu = cpu;
        assert_eq!(transfer.target_pc, 0x40_0100);
        rt.memory().u32(cpu.esp).unwrap()
    }

    #[test]
    fn a_fresh_machine_is_quiescent() {
        let m = machine();
        let q = m.quiescent();
        assert!(q.is_quiescent(), "{q:?}");
        assert!(q.rebuildable.is_empty());
    }

    #[test]
    fn a_callback_continuation_blocks_until_it_returns() {
        let mut m = machine();
        let token = callback(&mut m, Some("test: callback"));
        let q = m.quiescent();
        assert_eq!(
            q.blockers,
            vec![Blocker::Continuation {
                thread: 1,
                origin: "test: callback",
                target: 0x40_0100
            }]
        );
        assert_eq!(
            q.blockers[0].to_string(),
            "thread 1: continuation test: callback -> 0x00400100"
        );
        assert!(m.thread_mut(1).unwrap().ctx.cancel_continuation(token));
        assert!(m.quiescent().is_quiescent());
    }

    #[test]
    fn an_unnamed_continuation_is_named_by_its_closure_type() {
        let mut m = machine();
        callback(&mut m, None);
        let q = m.quiescent();
        let [Blocker::Continuation { origin, .. }] = q.blockers.as_slice() else {
            panic!("{q:?}");
        };
        assert!(origin.contains("quiesce"), "{origin}");
    }

    #[test]
    fn thread_return_continuations_are_rebuildable_not_blockers() {
        let mut m = machine();
        callback(&mut m, Some(THREAD_RETURN));
        let q = m.quiescent();
        assert!(q.is_quiescent(), "{q:?}");
        assert_eq!(q.rebuildable.len(), 1);
        assert_eq!(q.rebuildable[0].1.origin, THREAD_RETURN);
    }

    #[test]
    fn queued_interrupts_and_deferred_signals_block() {
        let m = machine();
        m.runtime.raise_interrupt(runtime::GuestInterrupt {
            target: 0x40_0200,
            args: vec![],
            convention: runtime::CallConvention::Stdcall,
            thread: None,
            coalesce: false,
            name: "test interrupt",
        });
        m.kernel.deferred.lock().unwrap().push(0x1234);
        let q = m.quiescent();
        assert_eq!(
            q.blockers,
            vec![Blocker::QueuedInterrupts(1), Blocker::DeferredSignals(1)]
        );
        assert_eq!(q.blockers[0].to_string(), "queued interrupts (1)");
        assert_eq!(m.runtime.clear_interrupts(), 1);
        m.kernel.deferred.lock().unwrap().clear();
        assert!(m.quiescent().is_quiescent());
    }

    #[test]
    fn a_running_periodic_handler_blocks() {
        let m = machine();
        let id = m
            .kernel
            .add_periodic(crate::periodic::Source {
                name: "test vblank",
                rate: crate::periodic::Rate::HZ_60,
                max_catch_up: 1,
                handler: Box::new(|_, _, _| Ok(None)),
            })
            .unwrap();
        assert!(m.quiescent().is_quiescent());
        let handler = m
            .kernel
            .lock()
            .periodic
            .get_mut(&id)
            .unwrap()
            .handler
            .take();
        assert_eq!(
            m.quiescent().blockers,
            vec![Blocker::PeriodicRunning("test vblank")]
        );
        m.kernel.lock().periodic.get_mut(&id).unwrap().handler = handler;
        assert!(m.quiescent().is_quiescent());
    }

    #[test]
    fn keys_group_threads_together() {
        let a = Blocker::Continuation {
            thread: 1,
            origin: "x",
            target: 4,
        };
        let b = Blocker::Continuation {
            thread: 2,
            origin: "x",
            target: 4,
        };
        assert_eq!(a.key(), b.key());
        assert_ne!(a.to_string(), b.to_string());
        let host = Blocker::Host {
            owner: "feature agent".into(),
            why: "not audited".into(),
        };
        assert!(!host.is_machine());
        assert_eq!(host.key(), "feature agent: not audited");
    }
}
