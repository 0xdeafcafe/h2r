//! Validate a mixed material section at the end of the guest's part loop.
//! An admission covers exactly the ordered bytes, headers and destinations
//! observed for this section. Camera motion and material edits re-arm it.

use super::*;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(PartialEq)]
struct Proposal(Vec<(u32, Vec<u8>)>);

impl Proposal {
    fn equals(&self, other: &[(u32, &[u8])]) -> bool {
        self.0.len() == other.len()
            && self
                .0
                .iter()
                .zip(other)
                .all(|((dest, data), (addr, bytes))| dest == addr && data.as_slice() == *bytes)
    }
}

#[derive(Default)]
struct Parity {
    generation: Option<u64>,
    checked: HashMap<(u32, u32), Proposal>,
    pending: Option<Visit>,
}

struct Visit {
    stack: u32,
    record: u32,
    key: (u32, u32),
    proposal: Proposal,
}

impl Parity {
    fn admit(
        &mut self,
        generation: u64,
        section: u32,
        owner: u32,
        stack: u32,
        record: u32,
        writes: &[(u32, &[u8])],
    ) -> bool {
        if self.generation != Some(generation) {
            self.checked.clear();
            self.generation = Some(generation);
        }
        let key = (section, owner);
        if self
            .checked
            .get(&key)
            .is_some_and(|prior| prior.equals(writes))
        {
            return true;
        }
        self.checked.remove(&key);
        self.pending = Some(Visit {
            stack,
            record,
            key,
            proposal: Proposal(writes.iter().map(|&(addr, b)| (addr, b.to_vec())).collect()),
        });
        false
    }

    fn finish(&mut self, m: &dyn GuestMemory, stack: u32, record: u32) -> Option<bool> {
        let p = self.pending.as_ref()?;
        if p.stack != stack || p.record != record {
            return None;
        }
        let p = self.pending.take()?;
        let exact = p.proposal.0.iter().all(|(addr, bytes)| {
            m.bytes(*addr, bytes.len())
                .is_ok_and(|guest| guest == *bytes)
        });
        if exact {
            // Bound retained section images in scenes with many placements.
            if self.checked.len() >= 128 {
                self.checked.clear();
            }
            self.checked.insert(p.key, p.proposal);
        }
        Some(exact)
    }
}

thread_local! { static PARITY: RefCell<Parity> = RefCell::default(); }

pub(crate) fn admit(
    generation: u64,
    section: u32,
    owner: u32,
    stack: u32,
    record: u32,
    writes: &[(u32, &[u8])],
) -> bool {
    PARITY.with_borrow_mut(|p| p.admit(generation, section, owner, stack, record, writes))
}

pub(crate) fn reset_visit() {
    PARITY.with_borrow_mut(|p| {
        p.pending = None;
    });
}

pub(crate) fn checking() -> bool {
    PARITY.with_borrow(|p| p.pending.is_some())
}

pub(crate) fn install(b: &mut hooks::Builder) {
    b.raw()
        .observe_before(
            &points::scene_effects::WORLD_SECTION_END,
            "check BSP material lists",
            |c| {
                if checking() {
                    let result =
                        PARITY.with_borrow_mut(|p| p.finish(&*c.guest(), c.cpu.esp, c.cpu.ebp));
                    if let Some(exact) = result {
                        SceneList::TransparentGeometry.count(SceneListResult::Compared);
                        if !exact {
                            SceneList::TransparentGeometry.count(SceneListResult::Mismatch);
                        }
                    }
                }
            },
        )
        .context_only()
        .optional();
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn admission_requires_the_guest_output_for_this_section_and_both_lists() {
        let mut p = Parity::default();
        let mut mem = FakeGuest::new();
        let edits: &[(u32, &[u8])] = &[(0x4000, &[1, 2]), (0x5000, &[5, 6])];
        assert!(!p.admit(8, 7, 1, 99, 2, edits));
        mem.put(0x4000, &[1, 2]).put(0x5000, &[6, 5]);
        assert_eq!(p.finish(&mem, 99, 9), None);
        assert_eq!(p.finish(&mem, 99, 2), Some(false));
        assert!(!p.admit(8, 7, 1, 99, 2, edits));
        mem.put(0x5000, &[5, 6]);
        assert_eq!(p.finish(&mem, 99, 2), Some(true));
        assert!(p.admit(8, 7, 1, 99, 2, edits));
        assert!(!p.admit(8, 7, 1, 99, 2, &[(0x4000, &[2, 1]), (0x5000, &[5, 6])]));
        assert!(!p.admit(9, 7, 1, 99, 2, edits));
    }
}
