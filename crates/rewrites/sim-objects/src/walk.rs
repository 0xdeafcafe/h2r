//! Live-slot selection shared by the object update walks and object views.

use std::ops::Range;

/// Find the first occupied slot in this range. `read_word(at)` supplies the
/// bitmap word containing slot `at`. Its first argument is the range start,
/// then the first slot in each following word. This also lets the engine
/// reproduce the guest cursor at a failing read.
///
/// Stop at the first hit. The next visit starts a fresh search with the
/// current high-water mark and bitmap, even if it resumes in the same word.
#[inline]
pub fn next_live<E>(
    slots: Range<u32>,
    mut read_word: impl FnMut(u32) -> Result<u32, E>,
) -> Result<Option<u32>, E> {
    let mut at = slots.start;
    while at < slots.end {
        let word = read_word(at)? & (u32::MAX << (at & 31));
        if word != 0 {
            let slot = (at & !31) | word.trailing_zeros();
            return Ok((slot < slots.end).then_some(slot));
        }
        let last_in_word = at | 31;
        if last_in_word >= slots.end - 1 {
            break;
        }
        at = last_in_word + 1;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(words: &[u32], slots: Range<u32>) -> Option<u32> {
        next_live(slots, |at| Ok::<_, ()>(words[(at >> 5) as usize])).unwrap()
    }

    #[test]
    fn agrees_with_a_slotwise_walk_for_each_subrange() {
        let mut words = [0u32; 3];
        for occupied in [
            vec![],
            vec![0, 31, 32, 63, 64, 95],
            vec![1, 58, 90],
            (0..96).collect(),
        ] {
            words.fill(0);
            for &slot in &occupied {
                words[(slot >> 5) as usize] |= 1 << (slot & 31);
            }
            for start in 0..=96 {
                for end in start..=96 {
                    let want = occupied
                        .iter()
                        .copied()
                        .find(|&slot| start <= slot && slot < end);
                    assert_eq!(scan(&words, start..end), want, "{start}..{end}");
                }
            }
        }
    }

    #[test]
    fn an_early_hit_avoids_unmapped_words_and_a_required_read_propagates_its_fault() {
        let mut probes = Vec::new();
        let mut read = |at| {
            probes.push(at);
            match at >> 5 {
                0 => Ok(0),
                1 => Ok(1 << 7),
                _ => Err(at),
            }
        };
        assert_eq!(next_live(17..96, &mut read), Ok(Some(39)));
        assert_eq!(next_live(40..96, &mut read), Err(64));
        assert_eq!(probes, [17, 32, 40, 64]);
    }

    #[test]
    fn the_next_visit_sees_same_word_changes_and_pool_growth() {
        let mut words = [1 << 2 | 1 << 4, 0];
        assert_eq!(scan(&words, 0..5), Some(2));
        // The first object's callback removes slot 4 and activates slot 7.
        words[0] = 1 << 2 | 1 << 7;
        assert_eq!(scan(&words, 3..8), Some(7));
        words[1] = 1 << 3;
        let last = scan(&words, 8..36).unwrap();
        assert_eq!(last, 35);
        assert_eq!(scan(&words, last + 1..5), None);
    }
}
