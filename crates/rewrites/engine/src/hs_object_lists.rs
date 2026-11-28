//! The object-list sweep `0x001deed0`, the HaloScript update's tail call
//! (crate::hs_update, step 6), in Rust. Every tick it frees the object
//! lists nothing references any more, and each list's chain of nodes.
//!
//! The guest body, whole (`0x1deed0..0x1defaa` and its callee
//! `0x2963b0..0x2963e0`, read from `h2 disasm`):
//!
//! | Step | Guest | Here |
//! |---|---|---|
//! | 1 | the lists pool `L = [0x4f55d8]`, read once | [`sweep`]'s `lists` |
//! | 2 | the first live slot (`0x16bc00` from 0), its salted handle | [`DatumArray::next_index`], [`DatumArray::handle_of`] |
//! | 3 | the list `data + index * 12` (the guest's stride, not `L+0x24`): word `+4` 0 (no references) | the same read |
//! | 3a | the nodes from `+8` (`0x2963b0`, nodes pool `[0x4f55d4]`): each node's next is its `+8`, read before the node is freed | [`free_chain`] |
//! | 3b | the list itself freed (`0x16ba40`) | [`DatumArray::delete`] |
//! | 4 | the next live slot after this one, the bound `L+0x38` read again | the same reads |
//!
//! A read that faults is the guest's own: the lifted body faults on the
//! same read.

use guest_layout::datum::DatumArray;
use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};

/// The sweep's guest entry.
pub const ENTRY: u32 = 0x001d_eed0;
/// The node-chain free the sweep calls, ported as [`free_chain`].
pub const FREE_CHAIN: u32 = 0x0029_63b0;
/// The object lists pool pointer.
pub const LISTS: u32 = 0x004f_55d8;
/// The list nodes pool pointer.
pub const NODES: u32 = 0x004f_55d4;
/// A list's size as the sweep addresses it.
const LIST: u32 = 12;
/// `u16` `+4`: the list's reference count.
const REFERENCES: u32 = 4;
/// `u32` `+8`: the list's first node; a node's next node.
const NEXT: u32 = 8;

/// `0x2963b0`: free the chain of nodes from `first` (`eax`) in `nodes`
/// (`edx`); `u32::MAX` ends it. Returns the nodes freed.
pub fn free_chain(m: &mut dyn GuestMemory, nodes: DatumArray, first: u32) -> Result<u32, MemFault> {
    let mut at = first;
    let mut freed = 0;
    while at != u32::MAX {
        let elem = nodes.element_addr(m, (at & 0xffff) as i32)?;
        let next = m.u32(elem.wrapping_add(NEXT))?;
        nodes.delete(m, DatumHandle(at))?;
        freed += 1;
        at = next;
    }
    Ok(freed)
}

/// The sweep. Returns the lists freed.
pub fn sweep(m: &mut dyn GuestMemory) -> Result<u32, MemFault> {
    let lists = DatumArray::at(m.u32(LISTS)?);
    let mut freed = 0;
    let mut at = lists.next_index(m, 0)?;
    while let Some(index) = at {
        let Some(handle) = lists.handle_of(m, index)? else { break };
        let slot = handle.raw() & 0xffff;
        let data = m.u32(lists.addr.wrapping_add(guest_layout::datum::layout::DATA))?;
        let list = data.wrapping_add(slot.wrapping_mul(LIST));
        if m.u16(list.wrapping_add(REFERENCES))? == 0 {
            let first = m.u32(list.wrapping_add(NEXT))?;
            let nodes = DatumArray::at(m.u32(NODES)?);
            free_chain(m, nodes, first)?;
            lists.delete(m, handle)?;
            freed += 1;
        }
        at = lists.next_index(m, slot as i32 + 1)?;
    }
    Ok(freed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::datum::layout;
    use guest_layout::testing::FakeGuest;

    const L: u32 = 0x0060_0000;
    const L_BITS: u32 = 0x0060_0800;
    const L_DATA: u32 = 0x0060_1000;
    const N: u32 = 0x0061_0000;
    const N_BITS: u32 = 0x0061_0800;
    const N_DATA: u32 = 0x0061_1000;
    const NODE: u32 = 0x10;

    fn pool(g: &mut FakeGuest, at: u32, bits: u32, data: u32, size: u32, slots: u32) {
        g.map(at, 0x1000).map(data, 0x1000);
        g.put_u32(at + layout::MAXIMUM_COUNT, 32)
            .put_u32(at + layout::ELEMENT_SIZE, size)
            .put_u32(at + layout::HIGH_WATER, slots)
            .put_u32(at + layout::ACTIVE_COUNT, slots)
            .put_u32(at + layout::DATA, data)
            .put_u32(at + layout::BITMAP, bits)
            .put_u32(bits, (1u32 << slots) - 1);
        for i in 0..slots {
            g.put_u16(data + size * i, 0x8000 + i as u16);
        }
    }

    fn handle(slot: u32) -> u32 {
        (0x8000 + slot) << 16 | slot
    }

    /// Lists 0..3 with (references, first node); nodes 0..5 chained as
    /// list 1: 0 -> 2, list 2: 1 -> 3 -> 4.
    fn guest(refs: [u16; 3]) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x004f_5000, 0x1000);
        g.put_u32(LISTS, L).put_u32(NODES, N);
        pool(&mut g, L, L_BITS, L_DATA, LIST, 3);
        pool(&mut g, N, N_BITS, N_DATA, NODE, 5);
        let firsts = [u32::MAX, handle(0), handle(1)];
        for (i, (r, f)) in refs.iter().zip(firsts).enumerate() {
            g.put_u16(L_DATA + LIST * i as u32 + REFERENCES, *r).put_u32(L_DATA + LIST * i as u32 + NEXT, f);
        }
        for (node, next) in [(0, handle(2)), (2, u32::MAX), (1, handle(3)), (3, handle(4)), (4, u32::MAX)] {
            g.put_u32(N_DATA + NODE * node + NEXT, next);
        }
        g
    }

    fn live(g: &FakeGuest, data: u32, size: u32, slots: u32) -> Vec<u32> {
        (0..slots).filter(|&i| g.u16(data + size * i).unwrap() != 0).collect()
    }

    #[test]
    fn unreferenced_lists_and_their_nodes_are_freed() {
        let mut g = guest([0, 2, 0]);
        assert_eq!(sweep(&mut g), Ok(2));
        assert_eq!(live(&g, L_DATA, LIST, 3), [1]);
        assert_eq!(live(&g, N_DATA, NODE, 5), [0, 2], "list 1's nodes stay");
        assert_eq!(g.u32(L + layout::ACTIVE_COUNT).unwrap(), 1);
        assert_eq!(g.u32(N + layout::ACTIVE_COUNT).unwrap(), 2);
        assert_eq!(g.u32(L_BITS).unwrap(), 0b010);
    }

    #[test]
    fn referenced_lists_stay() {
        let mut g = guest([1, 1, 1]);
        assert_eq!(sweep(&mut g), Ok(0));
        assert_eq!(live(&g, L_DATA, LIST, 3), [0, 1, 2]);
        assert_eq!(live(&g, N_DATA, NODE, 5), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn the_top_list_freed_lowers_the_high_water() {
        let mut g = guest([1, 1, 0]);
        assert_eq!(sweep(&mut g), Ok(1));
        assert_eq!(g.u32(L + layout::HIGH_WATER).unwrap(), 2);
        assert_eq!(live(&g, N_DATA, NODE, 5), [0, 2]);
    }

    #[test]
    fn an_empty_chain_frees_no_node() {
        let mut g = guest([0, 1, 1]);
        let nodes = DatumArray::at(N);
        assert_eq!(free_chain(&mut g, nodes, u32::MAX), Ok(0));
        assert_eq!(sweep(&mut g), Ok(1));
        assert_eq!(live(&g, N_DATA, NODE, 5), [0, 1, 2, 3, 4]);
    }
}
