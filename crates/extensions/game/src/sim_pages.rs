//! The page census of a sim run (docs/runtime/sim.md §6,
//! `wall.guest_pages_written_4k` and `_16k`; switch `impl.sim.page-census`):
//! how many guest RAM pages the counted match wrote, by comparing one hash
//! per 4 KiB page at the first counted tick with the same at the end. A page
//! that is written and ends with its old bytes is not counted, as with
//! `Guest::cow_census`. The 16 KiB figure counts aligned groups of four
//! pages with any difference, the number a 16 KiB-page host would own.

use runtime::Guest;

const PAGE: usize = 4096;
const GROUP: usize = 4;

/// One hash per 4 KiB page of guest physical RAM. A change to any one word
/// of a page changes its hash: each step is a bijection of the state given
/// the word.
pub fn hashes(guest: &Guest) -> Vec<u64> {
    guest.physical_ram().chunks(PAGE).map(page_hash).collect()
}

fn page_hash(page: &[u8]) -> u64 {
    const K: u64 = 0x9e37_79b9_7f4a_7c15;
    let (words, rest) = page.as_chunks::<8>();
    let mut h = page.len() as u64;
    for w in words {
        h = (h ^ u64::from_le_bytes(*w)).wrapping_mul(K | 1).rotate_left(23);
    }
    for &b in rest {
        h = (h ^ u64::from(b)).wrapping_mul(K | 1).rotate_left(23);
    }
    h
}

/// Pages (4 KiB) and aligned 16 KiB groups that differ between two
/// [`hashes`]. `None` when the two cover different amounts of RAM.
pub fn written(base: &[u64], end: &[u64]) -> Option<(u64, u64)> {
    if base.len() != end.len() {
        return None;
    }
    let pages = base.iter().zip(end).filter(|(a, b)| a != b).count() as u64;
    let groups = base
        .chunks(GROUP)
        .zip(end.chunks(GROUP))
        .filter(|(a, b)| a != b)
        .count() as u64;
    Some((pages, groups))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_changed_byte_changes_its_page_hash() {
        let mut page = vec![7u8; PAGE];
        let before = page_hash(&page);
        for at in [0, 1, 8, PAGE - 1] {
            let mut p = page.clone();
            p[at] ^= 1;
            assert_ne!(page_hash(&p), before, "byte {at}");
        }
        page[3] = 0;
        assert_ne!(page_hash(&page), before);
    }

    #[test]
    fn pages_and_groups_count_what_differs() {
        // Two groups of four pages and a short third.
        let base = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        let mut end = base;
        end[1] = 0; // group 0
        end[2] = 0; // group 0 again
        end[9] = 0; // the short last group
        assert_eq!(written(&base, &end), Some((3, 2)));
        assert_eq!(written(&base, &base), Some((0, 0)));
        assert_eq!(written(&base, &end[..9]), None);
    }
}
