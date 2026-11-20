//! Generation-checked callback identities. Slots can be reused, token values
//! cannot: a slot is retired before its generation would wrap.
use crate::{CpuAbi, ExecCtx, GuestFault, TransferAbi};
use std::collections::VecDeque;

// This control-only namespace follows the current kernel system allocator's
// upper bound, and does not consume physical RAM. Guest PCs need no alignment.
pub(crate) const BASE: u32 = 0xb000_0000;
pub(crate) const END: u32 = 0xf000_0000;
const MAX_SLOTS: usize = 1024;
const SLOT_BITS: u32 = 10;
const SLOT_MASK: u32 = MAX_SLOTS as u32 - 1;
const MAX_GENERATION: u32 = (END - BASE - 1) >> SLOT_BITS;
const _: () = assert!(MAX_SLOTS == 1 << SLOT_BITS);

/// A boxed callback continuation: what [`crate::ExecCtx::call_guest`]
/// stores, and what a snapshot load rebuilds
/// ([`crate::ExecCtx::restore_continuations`]).
pub type Continuation =
    Box<dyn FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result<TransferAbi, GuestFault> + Send>;

/// One slot of a context's continuation pool, for machine snapshots
/// (docs/runtime/snapshots.md). Every slot's generation is kept, live or
/// not, so tokens made after a load are the tokens the original would have
/// made: they land on the guest stack, so they are guest-visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotRecord {
    pub generation: u32,
    /// The live continuation in the slot, rebuildable ones only.
    pub live: Option<LiveRecord>,
}

/// A live rebuildable continuation: its maker's name, call target and state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveRecord {
    pub origin: String,
    pub target: u32,
    pub state: [u32; 4],
}

/// A context's whole continuation pool as plain data.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContinuationLayout {
    pub slots: Vec<SlotRecord>,
    /// Free slots in the order they will be reused.
    pub reusable: Vec<u32>,
}

/// Why a continuation pool cannot be saved or restored.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("continuation {origin} -> {target:#x} is a closure and cannot be saved")]
    NotRebuildable { origin: &'static str, target: u32 },
    #[error("no rebuilder for continuation {origin} -> {target:#x}")]
    NoRebuilder { origin: String, target: u32 },
    #[error("continuation layout is inconsistent: {0}")]
    Invalid(&'static str),
}

/// A live continuation, for snapshot quiescence checks
/// (docs/runtime/snapshots.md): what made it and which guest call it waits
/// on. A continuation is a Rust closure, so a machine holding one cannot be
/// saved unless its maker knows how to rebuild it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContinuationInfo {
    /// The return token on the guest stack.
    pub token: u32,
    /// The maker's name for it, or the closure's type name.
    pub origin: &'static str,
    /// The guest address the call went to.
    pub target: u32,
    /// For a rebuildable continuation, everything its closure captured
    /// beyond the context it runs on: with `origin` and `target` it can be
    /// made again after a snapshot load ([`crate::ExecCtx::call_guest_rebuildable`]).
    /// `None`: an ordinary closure, which blocks snapshots while live.
    pub state: Option<[u32; 4]>,
}

#[derive(Default)]
struct Slot {
    generation: u32,
    continuation: Option<Continuation>,
    origin: &'static str,
    target: u32,
    state: Option<[u32; 4]>,
}

#[derive(Default)]
pub(crate) struct Continuations {
    slots: Vec<Slot>,
    reusable: VecDeque<usize>,
}

pub(crate) fn is_token_address(pc: u32) -> bool {
    (BASE..END).contains(&pc)
}

impl Continuations {
    pub(crate) fn insert(
        &mut self,
        continuation: Continuation,
        origin: &'static str,
        target: u32,
        state: Option<[u32; 4]>,
    ) -> Option<u32> {
        // Spread identities across fresh slots first, then reuse in FIFO order.
        let index = if self.slots.len() < MAX_SLOTS {
            self.slots.push(Slot::default());
            self.slots.len() - 1
        } else {
            self.reusable.pop_front()?
        };
        let slot = &mut self.slots[index];
        debug_assert!(slot.continuation.is_none() && slot.generation < MAX_GENERATION);
        slot.generation += 1;
        slot.continuation = Some(continuation);
        slot.origin = origin;
        slot.target = target;
        slot.state = state;
        Some(BASE + (slot.generation << SLOT_BITS) + index as u32)
    }

    pub(crate) fn live(&self, token: u32) -> bool {
        is_token_address(token) && {
            let identity = token - BASE;
            self.slots
                .get((identity & SLOT_MASK) as usize)
                .is_some_and(|s| s.continuation.is_some() && s.generation == identity >> SLOT_BITS)
        }
    }

    pub(crate) fn take(&mut self, token: u32) -> Option<Continuation> {
        if !is_token_address(token) {
            return None;
        }
        let identity = token - BASE;
        let index = (identity & SLOT_MASK) as usize;
        let generation = identity >> SLOT_BITS;
        let slot = self.slots.get_mut(index)?;
        if generation == 0 || generation != slot.generation {
            return None;
        }
        let continuation = slot.continuation.take()?;
        if generation < MAX_GENERATION {
            self.reusable.push_back(index);
        }
        Some(continuation)
    }

    /// Every live continuation, in slot order.
    pub(crate) fn live_list(&self) -> Vec<ContinuationInfo> {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.continuation.is_some())
            .map(|(index, s)| ContinuationInfo {
                token: BASE + (s.generation << SLOT_BITS) + index as u32,
                origin: s.origin,
                target: s.target,
                state: s.state,
            })
            .collect()
    }

    /// The pool as plain data. Refuses while a closure that is not
    /// rebuildable is live.
    pub(crate) fn layout(&self) -> Result<ContinuationLayout, LayoutError> {
        let slots = self
            .slots
            .iter()
            .map(|s| {
                let live = match (&s.continuation, s.state) {
                    (None, _) => None,
                    (Some(_), None) => {
                        return Err(LayoutError::NotRebuildable {
                            origin: s.origin,
                            target: s.target,
                        });
                    }
                    (Some(_), Some(state)) => Some(LiveRecord {
                        origin: s.origin.to_owned(),
                        target: s.target,
                        state,
                    }),
                };
                Ok(SlotRecord {
                    generation: s.generation,
                    live,
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(ContinuationLayout {
            slots,
            reusable: self.reusable.iter().map(|&i| i as u32).collect(),
        })
    }

    /// Replace the pool with `layout`, making each live continuation again
    /// with `rebuild(origin, target, state)`, which answers the origin's
    /// static name and the closure. Nothing changes on an error.
    pub(crate) fn restore(
        &mut self,
        layout: &ContinuationLayout,
        mut rebuild: impl FnMut(&str, u32, [u32; 4]) -> Option<(&'static str, Continuation)>,
    ) -> Result<(), LayoutError> {
        if layout.slots.len() > MAX_SLOTS {
            return Err(LayoutError::Invalid("more slots than a pool holds"));
        }
        let mut seen = vec![false; layout.slots.len()];
        for &i in &layout.reusable {
            let free = layout
                .slots
                .get(i as usize)
                .is_some_and(|s| s.live.is_none());
            if !free || std::mem::replace(&mut seen[i as usize], true) {
                return Err(LayoutError::Invalid(
                    "reusable slot is live, repeated or absent",
                ));
            }
        }
        let mut slots = Vec::with_capacity(layout.slots.len());
        for record in &layout.slots {
            if record.generation > MAX_GENERATION {
                return Err(LayoutError::Invalid("generation out of range"));
            }
            let mut slot = Slot {
                generation: record.generation,
                ..Slot::default()
            };
            if let Some(live) = &record.live {
                if record.generation == 0 {
                    return Err(LayoutError::Invalid("live slot without a generation"));
                }
                let (origin, continuation) = rebuild(&live.origin, live.target, live.state)
                    .ok_or_else(|| LayoutError::NoRebuilder {
                        origin: live.origin.clone(),
                        target: live.target,
                    })?;
                slot.continuation = Some(continuation);
                slot.origin = origin;
                slot.target = live.target;
                slot.state = Some(live.state);
            }
            slots.push(slot);
        }
        self.slots = slots;
        self.reusable = layout.reusable.iter().map(|&i| i as usize).collect();
        Ok(())
    }

    pub(crate) fn clear(&mut self) -> usize {
        let mut count = 0;
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.continuation.take().is_some() {
                count += 1;
                if slot.generation < MAX_GENERATION {
                    self.reusable.push_back(index);
                }
            }
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn callback() -> Continuation {
        Box::new(|_, _| Ok(TransferAbi::stop(0)))
    }
    #[test]
    fn live_continuations_are_listed_with_their_origin() {
        let mut pool = Continuations::default();
        let a = pool.insert(callback(), "a", 0x1000, None).unwrap();
        let b = pool
            .insert(callback(), "b", 0x2000, Some([1, 2, 3, 4]))
            .unwrap();
        assert!(pool.take(a).is_some());
        assert_eq!(
            pool.live_list(),
            vec![ContinuationInfo {
                token: b,
                origin: "b",
                target: 0x2000,
                state: Some([1, 2, 3, 4]),
            }]
        );
        pool.clear();
        assert!(pool.live_list().is_empty());
    }
    #[test]
    fn a_layout_restores_the_same_future_tokens_and_rebuilds_live_slots() {
        let mut pool = Continuations::default();
        let a = pool.insert(callback(), "a", 0x1000, None).unwrap();
        let b = pool
            .insert(callback(), "b", 0x2000, Some([9, 0, 0, 0]))
            .unwrap();
        let c = pool.insert(callback(), "c", 0x3000, None).unwrap();
        assert!(pool.take(c).is_some());
        assert!(pool.take(a).is_some());
        let layout = pool.layout().unwrap();
        let mut copy = Continuations::default();
        copy.restore(&layout, |origin, target, state| {
            assert_eq!((origin, target, state), ("b", 0x2000, [9, 0, 0, 0]));
            Some(("b", callback()))
        })
        .unwrap();
        assert_eq!(copy.live_list(), pool.live_list());
        assert!(copy.live(b));
        assert_eq!(copy.layout().unwrap(), layout);
        for _ in 0..3 {
            assert_eq!(
                copy.insert(callback(), "n", 0, None),
                pool.insert(callback(), "n", 0, None)
            );
        }
        // A closure blocks a save; a missing rebuilder blocks a load and
        // leaves the pool alone.
        assert!(matches!(
            pool.layout(),
            Err(LayoutError::NotRebuildable { origin: "n", .. })
        ));
        let mut untouched = Continuations::default();
        assert!(matches!(
            untouched.restore(&layout, |_, _, _| None),
            Err(LayoutError::NoRebuilder { .. })
        ));
        assert!(untouched.slots.is_empty());
        let mut bad = layout.clone();
        bad.reusable.push(1); // b's slot, which is live
        assert!(matches!(
            Continuations::default().restore(&bad, |_, _, _| Some(("b", callback()))),
            Err(LayoutError::Invalid(_))
        ));
    }
    #[test]
    fn generation_exhaustion_retires_the_slot_instead_of_wrapping() {
        let mut pool = Continuations::default();
        pool.slots.resize_with(MAX_SLOTS, Slot::default);
        pool.slots[MAX_SLOTS - 1].generation = MAX_GENERATION - 1;
        pool.reusable.push_back(MAX_SLOTS - 1);
        let last = pool.insert(callback(), "test", 0, None).unwrap();
        assert_eq!(last, END - 1);
        assert!(pool.take(last).is_some());
        assert!(pool.take(last).is_none());
        assert!(pool.insert(callback(), "test", 0, None).is_none());
        assert_eq!(pool.clear(), 0);
        assert!(pool.insert(callback(), "test", 0, None).is_none());
        // An unused generation on another slot remains usable, but can never
        // make the retired slot's token resolve to that new closure.
        pool.reusable.push_back(0);
        let other = pool.insert(callback(), "test", 0, None).unwrap();
        assert!(pool.take(last).is_none());
        assert!(pool.take(other).is_some());
    }
    #[test]
    fn available_slots_are_reused_in_fifo_order_with_new_generations() {
        let mut pool = Continuations::default();
        let tokens: Vec<_> = (0..MAX_SLOTS)
            .map(|_| pool.insert(callback(), "test", 0, None).unwrap())
            .collect();
        for &index in &[7, 2, 11] {
            assert!(pool.take(tokens[index]).is_some());
        }
        for &index in &[7, 2, 11] {
            let next = pool.insert(callback(), "test", 0, None).unwrap();
            assert_ne!(next, tokens[index]);
            assert_eq!((next - BASE) & SLOT_MASK, index as u32);
            assert_eq!((next - BASE) >> SLOT_BITS, 2);
            assert!(pool.take(tokens[index]).is_none());
        }
    }
}
