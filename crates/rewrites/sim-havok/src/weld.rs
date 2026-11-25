//! Selection of the next contact from the unsorted suffix. Child order
//! matters to the solver. The first equal distance wins each pass.
use x87::abi::{self, X87Abi};

fn ordinary(word: u32) -> bool {
    let magnitude = word & 0x7fff_ffff;
    magnitude == 0 || (0x0080_0000..=0x7f80_0000).contains(&magnitude)
}

/// Select one candidate from a captured suffix and reproduce the x87 state
/// visible to the weld callback. The adapter re-reads distances after a
/// callback because it may have moved, replaced or invalidated candidates.
///
/// Widening normal binary32 numbers, zeros and infinities to x87 is exact.
/// All iterations use the same free stack slot. Only the last comparison
/// survives in C0/C2/C3, and only the winner survives in the popped slot.
/// NaNs, subnormals and a full x87 stack go to the ordered oracle.
pub fn select(distances: &[u32], fpu: &mut X87Abi) -> Option<usize> {
    let (&last, prefix) = distances.split_last()?;
    if !sim_core::det::x87_env(fpu.control, fpu.status, fpu.tag, 1)
        || !distances.iter().copied().all(ordinary)
    {
        return None;
    }
    let mut best = 0;
    for (i, &next) in prefix.iter().enumerate().skip(1) {
        if f32::from_bits(distances[best]) > f32::from_bits(next) {
            best = i;
        }
    }
    // This replays the tail instructions, including FCOM's defined bits.
    // When the last value wins, FLD puts its bits in the popped slot.
    abi::ld_f32(fpu, distances[best]);
    abi::cmp(fpu, 0, abi::src_kind::F32, u64::from(last), 0, false);
    if fpu.status & 0x4100 == 0 {
        abi::st_st(fpu, 0, true);
        abi::ld_f32(fpu, last);
        best = prefix.len();
    }
    abi::st_st(fpu, 0, true);
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oracle(keys: &[u32], fpu: &mut X87Abi) -> usize {
        let mut best = 0;
        abi::ld_f32(fpu, keys[0]);
        for (i, &key) in keys.iter().enumerate() {
            abi::cmp(fpu, 0, abi::src_kind::F32, u64::from(key), 0, false);
            if fpu.status & 0x4100 == 0 {
                abi::st_st(fpu, 0, true);
                abi::ld_f32(fpu, key);
                best = i;
            }
        }
        abi::st_st(fpu, 0, true);
        best
    }

    #[test]
    fn every_suffix_matches_x87_including_stale_stack_and_ties() {
        let distances = [2.0, f32::INFINITY, 4.0, -0.0, 0.0, -2.5, -1.0, -2.5,
            -3.0, f32::NEG_INFINITY].map(f32::to_bits);
        for first in 0..distances.len() {
            for end in first + 1..=distances.len() {
                let mut actual = X87Abi { control: 0x027f, status: 0x4520, ..Default::default() };
                abi::ld_f32(&mut actual, 1.25f32.to_bits());
                let mut expected = actual;
                let keys = &distances[first..end];
                assert_eq!(select(keys, &mut actual), Some(oracle(keys, &mut expected)));
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn exceptional_distance_leaves_input_stack_intact() {
        for exceptional in [1, 0x7f80_1234, f32::NAN.to_bits()] {
            let mut actual = X87Abi::default();
            let expected = actual;
            assert_eq!(select(&[0, exceptional], &mut actual), None);
            assert_eq!(actual, expected);
        }
    }
}
