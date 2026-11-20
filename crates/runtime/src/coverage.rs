//! Function coverage from the generated module registry. Diagnostic runs
//! route direct native calls back through the runner. Resumed entries are
//! recorded as selectors, alongside normal function entries.

use crate::{GuestFn, H2GenEntry, H2GenResume};
use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

pub(crate) struct Coverage {
    owners: HashMap<(usize, u32), BTreeSet<u32>>,
    data: Mutex<GuestCoverage>,
}

/// Coverage uses guest virtual addresses, never host function pointers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuestCoverage {
    pub registered: BTreeSet<u32>,
    pub functions: BTreeSet<u32>,
    /// Entry selectors of native bodies, including resume labels. Inline
    /// basic blocks that never return to dispatch have no selector here.
    pub selectors: BTreeSet<(u32, u32)>,
    pub unmapped: u64,
}

impl Coverage {
    pub(crate) fn new(entries: &[H2GenEntry], resumes: &[H2GenResume]) -> Self {
        let mut by_handler: HashMap<usize, Vec<u32>> = HashMap::new();
        let mut by_entry = HashMap::new();
        for e in entries.iter().filter(|e| e.kind == 0) {
            if let Some(handler) = e.handler {
                by_handler.entry(handler as usize).or_default().push(e.addr);
                by_entry.insert(e.addr, handler as usize);
            }
        }
        let mut owners: HashMap<_, BTreeSet<_>> = HashMap::new();
        for e in entries {
            let Some(handler) = e.handler else { continue };
            let handler = handler as usize;
            let set = owners.entry((handler, e.addr)).or_default();
            if e.kind == 0 {
                set.insert(e.addr);
            } else if let Some(candidates) = by_handler.get(&handler) {
                set.extend(candidates);
            }
        }
        for r in resumes {
            if let Some(&handler) = by_entry.get(&r.owner) {
                owners
                    .entry((handler, r.start))
                    .or_default()
                    .insert(r.owner);
            }
        }
        Self {
            owners,
            data: Mutex::new(GuestCoverage {
                registered: by_entry.into_keys().collect(),
                ..Default::default()
            }),
        }
    }

    pub(crate) fn record(&self, handler: GuestFn, pc: u32) {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(owners) = self
            .owners
            .get(&(handler as usize, pc))
            .filter(|set| !set.is_empty())
        {
            for &owner in owners {
                data.functions.insert(owner);
                data.selectors.insert((owner, pc));
            }
        } else {
            data.unmapped = data.unmapped.saturating_add(1);
        }
    }

    pub(crate) fn snapshot(&self) -> GuestCoverage {
        self.data.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CpuAbi, ExecCtxOpaque, TransferAbi};

    #[allow(unsafe_code)]
    unsafe extern "C" fn body(_: *mut CpuAbi, _: *mut ExecCtxOpaque, _: u32) -> TransferAbi {
        TransferAbi::stop(0)
    }

    #[test]
    fn selectors_are_attributed_to_the_registry_owner() {
        let coverage = Coverage::new(
            &[
                H2GenEntry {
                    addr: 0x1000,
                    kind: 0,
                    handler: Some(body),
                    abi_version: None,
                },
                H2GenEntry {
                    addr: 0x1010,
                    kind: 1,
                    handler: Some(body),
                    abi_version: None,
                },
            ],
            &[H2GenResume {
                pc: 0x1020,
                owner: 0x1000,
                start: 0x1020,
            }],
        );
        coverage.record(body, 0x1010);
        coverage.record(body, 0x1020);
        let report = coverage.snapshot();
        assert_eq!(report.registered, BTreeSet::from([0x1000]));
        assert_eq!(report.functions, BTreeSet::from([0x1000]));
        assert_eq!(
            report.selectors,
            BTreeSet::from([(0x1000, 0x1010), (0x1000, 0x1020)])
        );
        assert_eq!(report.unmapped, 0);
    }
}
