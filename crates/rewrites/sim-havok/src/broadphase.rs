//! Incremental three-axis endpoint sweep used by the island batch update.
//! Nodes store endpoint *positions*. Overlap tests on untouched axes use
//! those positions, including the order of endpoints with equal coordinates.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub value: u16,
    pub node: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Node {
    pub lower: [u16; 3],
    pub upper: [u16; 3],
    pub handle: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crossing {
    Pair { added: bool, handles: [u32; 2] },
    Marker { added: bool, offset: u32, node: u16 },
}

pub struct Sweep {
    pub axes: [Vec<Endpoint>; 3],
    pub nodes: Vec<Node>,
}

impl Node {
    fn overlaps_other_axes(self, other: Self, axis: usize) -> bool {
        if axis == 0 {
            // The X sweep subtracts Y and Z as one word. Keep the borrow
            // into Z, including at the signed halfword boundary.
            let pack = |p: [u16; 3]| u32::from(p[1]) | (u32::from(p[2]) << 16);
            let a = pack(self.upper).wrapping_sub(pack(other.lower));
            let b = pack(other.upper).wrapping_sub(pack(self.lower));
            return (a | b) & 0x8000_8000 == 0;
        }
        (0..3).filter(|&other_axis| other_axis != axis).all(|i| {
            let a = self.upper[i].wrapping_sub(other.lower[i]);
            let b = other.upper[i].wrapping_sub(self.lower[i]);
            (a | b) & 0x8000 == 0
        })
    }

    fn index_mut(&mut self, axis: usize, upper: bool) -> &mut u16 {
        if upper { &mut self.upper[axis] } else { &mut self.lower[axis] }
    }
}

impl Sweep {
    /// Move one box in the batch's handle order. On a malformed index the
    /// caller discards this transient sweep and its crossings.
    pub fn update(
        &mut self,
        node: u16,
        lower: [u16; 3],
        upper: [u16; 3],
        mut emit: impl FnMut(Crossing) -> Option<()>,
    ) -> Option<()> {
        for axis in 0..3 {
            if lower[axis] & 1 != 0 || upper[axis] & 1 != 1 || lower[axis] > upper[axis] {
                return None;
            }
            // Expand first, then contract. This also fixes the order of
            // transient additions/removals when a box jumps over a body.
            for (high, right, value) in [
                (false, false, lower[axis]),
                (true, true, upper[axis]),
                (true, false, upper[axis]),
                (false, true, lower[axis]),
            ] {
                let ends = &mut self.axes[axis];
                let mut slot = usize::from(*self.nodes.get_mut(usize::from(node))?.index_mut(axis, high));
                if ends.get(slot)?.node != node {
                    return None;
                }
                loop {
                    let next = if right { slot.checked_add(1)? } else { slot.checked_sub(1)? };
                    let neighbor = *ends.get(next)?;
                    if (right && neighbor.value >= value) || (!right && neighbor.value <= value) {
                        break;
                    }
                    let other_id = usize::from(neighbor.node);
                    if neighbor.node == node {
                        return None;
                    }
                    let other_high = neighbor.value & 1 != 0;
                    *self.nodes.get_mut(other_id)?.index_mut(axis, other_high) = u16::try_from(slot).ok()?;
                    ends[slot] = neighbor;
                    slot = next;
                    *self.nodes[usize::from(node)].index_mut(axis, high) = u16::try_from(slot).ok()?;
                    let moving = self.nodes[usize::from(node)];
                    let other = self.nodes[other_id];
                    if high != other_high && moving.overlaps_other_axes(other, axis) {
                        let added = high == right;
                        if axis == 0 && other.handle & 1 != 0 {
                            emit(Crossing::Marker { added, offset: other.handle & !1, node })?;
                        } else {
                            emit(Crossing::Pair { added, handles: [moving.handle, other.handle] })?;
                        }
                    }
                }
                ends[slot] = Endpoint { value, node };
            }
        }
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Sweep {
        let ends: Vec<_> = [(0,0), (2,1), (5,1), (6,2), (9,2), (10,3), (13,3), (99,0)]
            .into_iter().map(|(value,node)| Endpoint { value, node }).collect();
        let flat: Vec<_> = [(0,0), (2,1), (2,2), (2,3), (5,1), (5,2), (5,3), (99,0)]
            .into_iter().map(|(value,node)| Endpoint { value, node }).collect();
        let mut nodes = vec![Node { lower: [0;3], upper: [0;3], handle: 0 }; 4];
        let axes = [ends, flat.clone(), flat];
        for (axis, endpoints) in axes.iter().enumerate() {
            for (i, end) in endpoints.iter().enumerate() {
                let node = &mut nodes[end.node as usize];
                *node.index_mut(axis,end.value & 1 != 0) = i as u16;
                node.handle = u32::from(end.node)*8;
            }
        }
        Sweep { axes, nodes }
    }

    #[test]
    fn crossing_two_bodies_preserves_intermediate_adds_and_removes() {
        let mut world = fixture();
        let mut events = vec![];
        world.update(1, [14,2,2], [17,5,5], |e| { events.push(e); Some(()) }).unwrap();
        let pair = |added, other| Crossing::Pair { added, handles: [8,other] };
        assert_eq!(events, [pair(true,16), pair(true,24), pair(false,16), pair(false,24)]);
        for (axis, ends) in world.axes.iter().enumerate() {
            assert!(ends.windows(2).all(|w| w[0].value <= w[1].value));
            for (slot, end) in ends.iter().enumerate() {
                let n = world.nodes[usize::from(end.node)];
                assert_eq!(slot,usize::from(if end.value & 1 != 0 { n.upper[axis] } else { n.lower[axis] }));
            }
        }

        events.clear();
        world.update(1,[2,2,2],[5,5,5],|e| { events.push(e); Some(()) }).unwrap();
        assert_eq!(events, [pair(true,24),pair(true,16),pair(false,24),pair(false,16)]);
    }

    #[test]
    fn marker_crossings_are_ordered_separately_from_body_pairs() {
        let mut world = fixture();
        world.nodes[2].handle = 17;
        let mut events = vec![];
        world.update(1,[6,2,2],[13,5,5], |e| { events.push(e); Some(()) }).unwrap();
        assert_eq!(events, [
            Crossing::Marker { added:true, offset:16, node:1 },
            Crossing::Pair { added:true, handles:[8,24] }
        ]);
        // Equal keys retain positions.
        let initial = world.axes.clone();
        world.update(1,[6,2,2],[13,5,5], |_| panic!("unexpected crossing")).unwrap();
        assert_eq!(initial,world.axes);
    }
}
