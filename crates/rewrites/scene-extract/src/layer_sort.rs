//! Draw order shared by render layers and the transparent queue.

/// The layer sort at `0x0013da70` moves the middle pivot to the first slot, then scans
/// until the boolean comparator fires on each side. The larger partition
/// goes on the stack. Short partitions move the first maximum to the end.
/// Exact swaps matter both for equal keys and for the plane comparator.
pub fn sort<T: Copy>(mut slots: &mut [T], after: &impl Fn(T, T) -> bool) {
    while slots.len() > 8 {
        slots.swap(0, slots.len() / 2);
        let pivot = slots[0];
        let (mut left, mut right) = (0, slots.len());
        loop {
            left += 1;
            while left < slots.len() && !after(slots[left], pivot) {
                left += 1;
            }
            right -= 1;
            while right > 0 && !after(pivot, slots[right]) {
                right -= 1;
            }
            if right < left {
                break;
            }
            slots.swap(left, right);
        }
        slots.swap(0, right);
        let (lo, tail) = slots.split_at_mut(right);
        let hi = &mut tail[left - right..];
        if lo.len() < hi.len() {
            sort(lo, after);
            slots = hi;
        } else {
            sort(hi, after);
            slots = lo;
        }
    }
    for end in (1..slots.len()).rev() {
        let mut largest = 0;
        for i in 1..=end {
            if after(slots[i], slots[largest]) {
                largest = i;
            }
        }
        slots.swap(largest, end);
    }
}

/// The dispatch order of world and object sort slots. Layer 5 compares the
/// inherited distance word, packed model/section and source index. The other
/// opaque layers compare the selected program, shader, group and vertex class.
pub fn geometry(slots: &mut [[u8; 24]], layer: u32) {
    let half = |s: &[u8; 24], at| u16::from_le_bytes([s[at], s[at + 1]]);
    let word = |s: &[u8; 24], at| u32::from_le_bytes(s[at..at + 4].try_into().unwrap());
    if layer == 5 {
        let key = |s: &[u8; 24]| (s[5], half(s, 6), word(s, 16), half(s, 2), word(s, 12), half(s, 0));
        sort(slots, &|a, b| key(&a) > key(&b));
    } else {
        let key = |s: &[u8; 24]| (word(s, 16), half(s, 2), s[5], half(s, 8));
        sort(slots, &|a, b| key(&a) > key(&b));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_order_uses_inherited_words_and_breaks_final_ties_by_source_slot() {
        let mut entries = [[0u8; 24]; 4];
        for (s, (id, distance, model_section)) in entries.iter_mut().zip([
            (9u16, 31u16, 1u32), (4, 7, 2), (5, 7, 1), (2, 7, 1),
        ]) {
            s[..2].copy_from_slice(&id.to_le_bytes());
            s[6..8].copy_from_slice(&distance.to_le_bytes());
            s[12..16].copy_from_slice(&model_section.to_le_bytes());
        }
        geometry(&mut entries, 5);
        assert_eq!(entries.map(|s| s[0]), [2, 5, 4, 9]);
    }

    #[test]
    fn opaque_program_order_preserves_the_partition_swaps_for_equal_keys() {
        let mut entries: [[u8; 24]; 9] = std::array::from_fn(|i| {
            let mut slot = [0u8; 24];
            slot[0] = i as u8;
            slot
        });
        geometry(&mut entries, 3);
        assert_eq!(entries.map(|s| s[0]), [4, 1, 2, 3, 0, 5, 6, 7, 8]);
    }
}
