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
