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

pub(crate) type Continuation =
    Box<dyn FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result<TransferAbi, GuestFault> + Send>;

#[derive(Default)]
struct Slot {
    generation: u32,
    continuation: Option<Continuation>,
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
    pub(crate) fn insert(&mut self, continuation: Continuation) -> Option<u32> {
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
    fn generation_exhaustion_retires_the_slot_instead_of_wrapping() {
        let mut pool = Continuations::default();
        pool.slots.resize_with(MAX_SLOTS, Slot::default);
        pool.slots[MAX_SLOTS - 1].generation = MAX_GENERATION - 1;
        pool.reusable.push_back(MAX_SLOTS - 1);
        let last = pool.insert(callback()).unwrap();
        assert_eq!(last, END - 1);
        assert!(pool.take(last).is_some());
        assert!(pool.take(last).is_none());
        assert!(pool.insert(callback()).is_none());
        assert_eq!(pool.clear(), 0);
        assert!(pool.insert(callback()).is_none());
        // An unused generation on another slot remains usable, but can never
        // make the retired slot's token resolve to that new closure.
        pool.reusable.push_back(0);
        let other = pool.insert(callback()).unwrap();
        assert!(pool.take(last).is_none());
        assert!(pool.take(other).is_some());
    }
    #[test]
    fn available_slots_are_reused_in_fifo_order_with_new_generations() {
        let mut pool = Continuations::default();
        let tokens: Vec<_> = (0..MAX_SLOTS)
            .map(|_| pool.insert(callback()).unwrap())
            .collect();
        for &index in &[7, 2, 11] {
            assert!(pool.take(tokens[index]).is_some());
        }
        for &index in &[7, 2, 11] {
            let next = pool.insert(callback()).unwrap();
            assert_ne!(next, tokens[index]);
            assert_eq!((next - BASE) & SLOT_MASK, index as u32);
            assert_eq!((next - BASE) >> SLOT_BITS, 2);
            assert!(pool.take(tokens[index]).is_none());
        }
    }
}
