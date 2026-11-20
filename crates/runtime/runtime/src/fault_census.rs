//! The fault census: which guest memory faults need a precise exit.
//!
//! A generated handler that leaves with a memory FAULT spills exact
//! registers, EFLAGS and the defined-flags mask, because something might
//! go on from that state: an SEH handler that continues execution, a host
//! that retries the access. Keeping that state exact is what makes every
//! load and store an observation point for the IR passes
//! (`crates/recompiler/opt`). The census counts, per run, every memory
//! fault and what became of the state it left: whether the run ended with
//! it (the state is only reported) or whether guest execution went on from
//! it on the same context (the state was used). Only the second needs a
//! precise exit (docs/runtime/block-clock-imprecise-faults.md).
//!
//! Always on: faults are rare, and each costs one lock here. The runner's
//! hot path pays one predictable branch per transfer
//! ([`ExecCtx::census_continue`]). The agent API reads it as
//! `profile.faults`.

use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::{Event, ExecCtx, GuestFault, TransferAbi, abi};

/// Distinct fault PCs kept; later ones count in the totals only.
pub const MAX_SITES: usize = 4096;

/// Who raised a memory fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FaultSource {
    /// An accessor generated code called (`h2abi_v1_*`): the handler leaves
    /// with this fault, and its exit state is what the IR passes keep exact.
    Generated,
    /// The runtime or a Rust handler (an HLE override, the runner's execute
    /// check): its state is whatever the Rust code left, not the passes'.
    Runtime,
}

impl FaultSource {
    pub fn name(self) -> &'static str {
        match self {
            Self::Generated => "generated",
            Self::Runtime => "runtime",
        }
    }
}

/// One fault PC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultSite {
    pub pc: u32,
    pub source: FaultSource,
    /// Faults raised at this PC.
    pub count: u64,
    /// Of those, the ones guest execution went on from.
    pub continued: u64,
    /// The first fault's details, for the report.
    pub first: GuestFault,
}

/// What [`FaultCensus::snapshot`] reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FaultCounts {
    /// Memory faults raised by accessors generated code called.
    pub generated: u64,
    /// Memory faults raised by the runtime or a Rust handler.
    pub runtime: u64,
    /// Memory faults a run returned to its caller with (each counted once,
    /// however many nested runners hand it up).
    pub ended: u64,
    /// Times guest execution went on, on the same context, from the state a
    /// memory fault left: the next transfer the runner dispatched there.
    pub continued: u64,
    /// Of `continued`, faults raised in generated code. These, and only
    /// these, need a precise exit: zero over a campaign run and a match is
    /// what imprecise memory faults require.
    pub continued_generated: u64,
    /// Of `continued_generated`, faults from a module built with imprecise
    /// faults (`generated::FEATURE_IMPRECISE_FAULTS`): execution went on
    /// from best-effort EFLAGS. Any is a bug, reported on stderr once.
    pub continued_imprecise: u64,
    /// Fault PCs by count, at most [`MAX_SITES`].
    pub sites: Vec<FaultSite>,
    /// Some PCs were not kept ([`MAX_SITES`]).
    pub sites_full: bool,
}

impl FaultCounts {
    /// The census's answer: no memory fault from generated code was gone on
    /// from, so no generated access needs a precise exit.
    pub fn precise_exits_needed(&self) -> bool {
        self.continued_generated != 0
    }
}

/// The census, one per [`crate::Runtime`].
#[derive(Debug, Default)]
pub struct FaultCensus(Mutex<Inner>);

#[derive(Debug, Default)]
struct Inner {
    counts: FaultCounts,
    by_pc: BTreeMap<u32, FaultSite>,
}

impl FaultCensus {
    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A memory fault was raised at `pc`.
    pub(crate) fn note_fault(&self, pc: u32, fault: &GuestFault, source: FaultSource) {
        let mut i = self.inner();
        match source {
            FaultSource::Generated => i.counts.generated += 1,
            FaultSource::Runtime => i.counts.runtime += 1,
        }
        if let Some(site) = i.by_pc.get_mut(&pc) {
            site.count += 1;
        } else if i.by_pc.len() < MAX_SITES {
            i.by_pc.insert(
                pc,
                FaultSite {
                    pc,
                    source,
                    count: 1,
                    continued: 0,
                    first: fault.clone(),
                },
            );
        } else {
            i.counts.sites_full = true;
        }
    }

    fn note_ended(&self) {
        self.inner().counts.ended += 1;
    }

    fn note_continued(&self, open: OpenFault, imprecise: bool) {
        let mut i = self.inner();
        i.counts.continued += 1;
        if open.source == FaultSource::Generated {
            i.counts.continued_generated += 1;
            if imprecise {
                i.counts.continued_imprecise += 1;
                if i.counts.continued_imprecise == 1 {
                    eprintln!(
                        "fault census: execution went on from the fault at {:#010x}, built with imprecise faults: \
                         its EFLAGS are best-effort (docs/runtime/block-clock-imprecise-faults.md)",
                        open.pc
                    );
                }
            }
        }
        if let Some(site) = i.by_pc.get_mut(&open.pc) {
            site.continued += 1;
        }
    }

    /// The totals and the sites, most frequent first.
    pub fn snapshot(&self) -> FaultCounts {
        let i = self.inner();
        let mut counts = i.counts.clone();
        counts.sites = i.by_pc.values().cloned().collect();
        counts
            .sites
            .sort_by(|a, b| b.count.cmp(&a.count).then(a.pc.cmp(&b.pc)));
        counts
    }

    /// Start again from nothing (a new census window).
    pub fn reset(&self) {
        *self.inner() = Inner::default();
    }
}

/// The memory fault a context's last run ended with, until the context
/// goes on or is restarted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OpenFault {
    pub event: u32,
    pub pc: u32,
    pub source: FaultSource,
}

impl ExecCtx {
    /// A run returned `result` to its caller. A memory fault becomes this
    /// context's open fault, counted as ended once.
    pub(crate) fn census_run_end(&mut self, result: &TransferAbi) {
        if result.kind != abi::FAULT || self.open_fault.is_some_and(|o| o.event == result.event_id) {
            return;
        }
        let Some(Event::MemoryFault { pc, .. }) = self.event(result.event_id) else {
            return;
        };
        let pc = *pc;
        let source = self
            .fault_sources
            .iter()
            .find(|(id, _)| *id == result.event_id)
            .map_or(FaultSource::Runtime, |(_, s)| *s);
        self.open_fault = Some(OpenFault {
            event: result.event_id,
            pc,
            source,
        });
        self.runtime().fault_census().note_ended();
    }

    /// The runner is about to dispatch a transfer on this context: if a
    /// memory fault is open, execution goes on from the state it left.
    #[inline]
    pub(crate) fn census_continue(&mut self) {
        if self.open_fault.is_some() {
            self.census_continue_cold();
        }
    }

    #[cold]
    #[inline(never)]
    fn census_continue_cold(&mut self) {
        if let Some(open) = self.open_fault.take() {
            let imprecise =
                self.runtime().module_features() & crate::generated::FEATURE_IMPRECISE_FAULTS != 0;
            self.runtime().fault_census().note_continued(open, imprecise);
        }
    }

    /// Record a memory fault's event and count it.
    pub(crate) fn emit_memory_fault(&mut self, pc: u32, fault: GuestFault, source: FaultSource) -> u32 {
        self.runtime().fault_census().note_fault(pc, &fault, source);
        let id = self.emit(Event::MemoryFault { pc, fault });
        // Remember who raised the last few, for census_run_end. Bounded: a
        // run ends with the first one it raised, long before this fills.
        if self.fault_sources.len() == FAULT_SOURCES {
            self.fault_sources.remove(0);
        }
        self.fault_sources.push((id, source));
        id
    }

    /// The memory fault this context's last run ended with and nothing has
    /// gone on from yet, if any: its PC.
    pub fn open_fault_pc(&self) -> Option<u32> {
        self.open_fault.map(|o| o.pc)
    }

    /// Drop the open fault without counting it as continued: for a host
    /// that discards the faulting state and re-enters the guest from state
    /// of its own (a fresh call with its own registers).
    pub fn discard_fault(&mut self) {
        self.open_fault = None;
    }
}

/// Sources remembered per context ([`ExecCtx::emit_memory_fault`]).
pub(crate) const FAULT_SOURCES: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::guest::{Access, FaultReason};

    fn fault(addr: u32) -> GuestFault {
        GuestFault {
            addr,
            len: 4,
            access: Access::Read,
            reason: FaultReason::Unmapped,
        }
    }

    #[test]
    fn sites_count_by_pc_and_sort_by_count() {
        let c = FaultCensus::default();
        c.note_fault(0x10, &fault(0), FaultSource::Generated);
        c.note_fault(0x20, &fault(4), FaultSource::Runtime);
        c.note_fault(0x20, &fault(8), FaultSource::Runtime);
        let s = c.snapshot();
        assert_eq!((s.generated, s.runtime), (1, 2));
        assert_eq!(s.sites.iter().map(|s| (s.pc, s.count)).collect::<Vec<_>>(), [(0x20, 2), (0x10, 1)]);
        assert_eq!(s.sites[0].first.addr, 4, "the first fault's details stay");
        assert!(!s.precise_exits_needed());
        c.reset();
        assert_eq!(c.snapshot(), FaultCounts::default());
    }

    #[test]
    fn only_continued_generated_faults_need_precise_exits() {
        let c = FaultCensus::default();
        c.note_fault(0x10, &fault(0), FaultSource::Runtime);
        c.note_continued(OpenFault { event: 1, pc: 0x10, source: FaultSource::Runtime }, true);
        assert!(!c.snapshot().precise_exits_needed());
        assert_eq!(c.snapshot().continued_imprecise, 0, "a Rust handler's fault is not the passes'");
        c.note_fault(0x30, &fault(0), FaultSource::Generated);
        c.note_continued(OpenFault { event: 2, pc: 0x30, source: FaultSource::Generated }, false);
        let s = c.snapshot();
        assert_eq!((s.continued, s.continued_generated, s.continued_imprecise), (2, 1, 0));
        assert!(s.precise_exits_needed());
        assert_eq!(s.sites.iter().find(|s| s.pc == 0x30).unwrap().continued, 1);
    }

    const CODE: u32 = 0x0001_0000;
    const LOAD_PC: u32 = CODE + 4;
    const AFTER: u32 = CODE + 0x10;
    const UNMAPPED: u32 = 0xdead_0000;

    /// A runtime whose `CODE` leaves with a fault the way a generated
    /// handler does (an `h2abi_v1_read` of an unmapped address, then FAULT
    /// with its event), and whose `AFTER` just stops.
    #[allow(unsafe_code)]
    fn faulting() -> std::sync::Arc<crate::Runtime> {
        use crate::guest::Permissions;
        let mut d = crate::Dispatch::default();
        let _ = d.register_override(CODE, "load", |cpu, ctx, _| {
            let mut out = [0u8; 4];
            // SAFETY: the live context of this handler call, and a 4-byte buffer.
            let s = unsafe {
                crate::ffi::h2abi_v1_read((ctx as *mut ExecCtx).cast(), LOAD_PC, UNMAPPED, 4, 1, out.as_mut_ptr())
            };
            assert_ne!(s, 0);
            cpu.eip = LOAD_PC;
            Ok(TransferAbi::fault(s))
        });
        let _ = d.register_override(AFTER, "after", |_, _, _| Ok(TransferAbi::stop(0)));
        let rt = crate::Runtime::new(crate::Guest::new(), d);
        rt.memory().map_zeroed(CODE, 0x1000, Permissions::RX).unwrap();
        rt
    }

    #[test]
    fn a_fault_a_run_ends_with_is_counted_once_and_not_as_continued() {
        let rt = faulting();
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        let mut cpu = crate::CpuAbi::default();
        let r = crate::Runner::default().run(&mut cpu, &mut ctx, CODE);
        assert_eq!(r.kind, abi::FAULT);
        assert_eq!(ctx.open_fault_pc(), Some(LOAD_PC));
        // Handing the same result up again (a nested runner's caller) does
        // not count it twice.
        ctx.census_run_end(&r);
        let s = rt.fault_census().snapshot();
        assert_eq!((s.generated, s.runtime, s.ended, s.continued), (1, 0, 1, 0));
        assert_eq!(s.sites[0].pc, LOAD_PC);
        assert_eq!((s.sites[0].source, s.sites[0].first.addr), (FaultSource::Generated, UNMAPPED));
        assert!(!s.precise_exits_needed());
    }

    #[test]
    fn going_on_from_a_generated_fault_needs_a_precise_exit() {
        let rt = faulting();
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        let mut cpu = crate::CpuAbi::default();
        assert_eq!(crate::Runner::default().run(&mut cpu, &mut ctx, CODE).kind, abi::FAULT);
        // A host that carries on from the faulting state (an SEH continue).
        assert_eq!(crate::Runner::default().run(&mut cpu, &mut ctx, AFTER).kind, abi::STOP);
        let s = rt.fault_census().snapshot();
        assert_eq!((s.ended, s.continued, s.continued_generated), (1, 1, 1));
        assert_eq!(s.sites[0].continued, 1);
        assert!(s.precise_exits_needed());
        assert_eq!(ctx.open_fault_pc(), None, "counted once");
    }

    #[test]
    fn a_discarded_or_restarted_fault_is_not_continued() {
        let rt = faulting();
        let mut cpu = crate::CpuAbi::default();
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        assert_eq!(crate::Runner::default().run(&mut cpu, &mut ctx, CODE).kind, abi::FAULT);
        ctx.discard_fault();
        assert_eq!(crate::Runner::default().run(&mut cpu, &mut ctx, AFTER).kind, abi::STOP);
        assert_eq!(crate::Runner::default().run(&mut cpu, &mut ctx, CODE).kind, abi::FAULT);
        let mut ctx = ctx.restart();
        assert_eq!(crate::Runner::default().run(&mut cpu, &mut ctx, AFTER).kind, abi::STOP);
        let s = rt.fault_census().snapshot();
        assert_eq!((s.generated, s.ended, s.continued), (2, 2, 0));
    }

    #[test]
    fn a_rust_handlers_fault_is_counted_as_the_runtimes() {
        let mut d = crate::Dispatch::default();
        let _ = d.register_override(CODE, "rust", |_, ctx, _| {
            ctx.memory().u32(UNMAPPED)?;
            Ok(TransferAbi::stop(0))
        });
        let _ = d.register_override(AFTER, "after", |_, _, _| Ok(TransferAbi::stop(0)));
        let rt = crate::Runtime::new(crate::Guest::new(), d);
        rt.memory().map_zeroed(CODE, 0x1000, crate::guest::Permissions::RX).unwrap();
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        let mut cpu = crate::CpuAbi::default();
        assert_eq!(crate::Runner::default().run(&mut cpu, &mut ctx, CODE).kind, abi::FAULT);
        assert_eq!(crate::Runner::default().run(&mut cpu, &mut ctx, AFTER).kind, abi::STOP);
        let s = rt.fault_census().snapshot();
        assert_eq!((s.generated, s.runtime, s.continued, s.continued_generated), (0, 1, 1, 0));
        assert!(!s.precise_exits_needed());
    }

    #[test]
    fn the_site_table_is_bounded() {
        let c = FaultCensus::default();
        for pc in 0..MAX_SITES as u32 + 3 {
            c.note_fault(pc, &fault(0), FaultSource::Generated);
        }
        let s = c.snapshot();
        assert_eq!((s.sites.len(), s.sites_full, s.generated), (MAX_SITES, true, MAX_SITES as u64 + 3));
    }
}
