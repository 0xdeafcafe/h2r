//! A cluster's row of the potentially visible set as `0x0016de20` gives
//! it (docs/graphics/native-visibility.md, "Computed visible sets"): the
//! tag's row, or, for a cluster whose flags byte has bit 1, a row computed
//! at run time from the open portals.
//!
//! The computed row starts from the cluster's row of the second table
//! (`0x0016e150`). Each pass walks the clusters the row holds, ascending
//! (`0x00044690`), and each of their portals not yet looked at: a portal
//! with flag `0x4` that is open adds, masked by the cluster's own row of
//! the first table, the union of its two clusters' second-table rows. A
//! portal is looked at once. When a pass adds nothing the row is done.
//! The result does not depend on the walk's order: it is the smallest row
//! closed under that rule.

use super::Decline;
use super::flood::OpenPortals;
use super::graph::{PortalGraph, portal_flags};

/// Words a row may have: the guest's frame holds 16 (512 clusters).
const MAX_WORDS: usize = 16;

/// `from`'s row, the tag's or the computed one.
pub fn row(graph: &PortalGraph, open: &OpenPortals, from: u16) -> Result<Vec<u32>, Decline> {
    let cluster = graph
        .clusters
        .get(usize::from(from))
        .ok_or(Decline::Layout)?;
    if graph.pvs_words > MAX_WORDS {
        return Err(Decline::Layout);
    }
    let own = graph.pvs_row(from).ok_or(Decline::Layout)?;
    if !cluster.computed_pvs {
        return Ok(own.to_vec());
    }
    let alt = |c: u16| graph.pvs_alt_row(c).ok_or(Decline::Layout);
    let mut current = alt(from)?.to_vec();
    let mut next = current.clone();
    let mut looked = vec![false; graph.portals.len()];
    loop {
        for c in set_bits(&current) {
            let listed = graph.clusters.get(c).ok_or(Decline::Layout)?;
            for &p in &listed.portals {
                let index = usize::from(p);
                let (Some(seen), Some(portal)) = (looked.get_mut(index), graph.portals.get(index))
                else {
                    return Err(Decline::Layout);
                };
                if std::mem::replace(seen, true) {
                    continue;
                }
                if portal.flags & portal_flags::JOINS_COMPUTED_PVS == 0 || !open.is_open(p) {
                    continue;
                }
                let [a, b] = portal.clusters.map(alt);
                let (a, b) = (a?, b?);
                for (k, word) in next.iter_mut().enumerate() {
                    *word |= (a[k] | b[k]) & own[k];
                }
            }
        }
        if next == current {
            return Ok(current);
        }
        current.clone_from(&next);
    }
}

/// The set bits of a row, ascending.
fn set_bits(row: &[u32]) -> impl Iterator<Item = usize> + '_ {
    row.iter().enumerate().flat_map(|(w, &word)| {
        (0..32)
            .filter(move |b| word & (1u32 << b) != 0)
            .map(move |b| 32 * w + b)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visibility::flood::tests::corridor;
    use crate::visibility::graph::portal_flags::JOINS_COMPUTED_PVS;

    /// Four rooms in a row (portals 0, 1, 2 between them). The first table
    /// lets every room see every room; the second gives each room itself.
    fn rooms() -> PortalGraph {
        let mut g = corridor(&[0.0, 0.0, 0.0]);
        g.pvs_words = 1;
        g.pvs = vec![0b1111, 0b1111, 0b1111, 0b1111];
        g.pvs_alt = vec![0b0001, 0b0010, 0b0100, 0b1000];
        for p in &mut g.portals {
            p.flags = JOINS_COMPUTED_PVS;
        }
        g
    }

    #[test]
    fn a_tag_row_is_copied_and_a_computed_row_grows_through_open_portals() {
        let mut g = rooms();
        let open = OpenPortals::all(3);
        assert_eq!(row(&g, &open, 0).unwrap(), [0b1111], "the tag's row");
        g.clusters[0].computed_pvs = true;
        assert_eq!(row(&g, &open, 0).unwrap(), [0b1111], "through all three");
        // Room 0's own row masks what the walk adds.
        g.pvs[0] = 0b0111;
        assert_eq!(row(&g, &open, 0).unwrap(), [0b0111]);
    }

    #[test]
    fn a_shut_portal_or_one_without_the_flag_stops_the_walk() {
        let mut g = rooms();
        g.clusters[0].computed_pvs = true;
        let mut open = OpenPortals::all(3);
        open.0[0] &= !(1 << 1);
        assert_eq!(row(&g, &open, 0).unwrap(), [0b0011], "portal 1 is shut");
        let open = OpenPortals::all(3);
        g.portals[0].flags = 0;
        assert_eq!(
            row(&g, &open, 0).unwrap(),
            [0b0001],
            "the second table's row alone"
        );
    }

    #[test]
    fn a_deliberate_mismatch_with_the_tag_row_is_the_computed_one() {
        let mut g = rooms();
        let open = OpenPortals::all(3);
        g.pvs[3] = 0b1000;
        g.clusters[3].computed_pvs = true;
        g.pvs_alt[3] = 0b1100;
        // The starting row is not masked; what the portals add is.
        assert_eq!(row(&g, &open, 3).unwrap(), [0b1100], "not the tag's 0b1000");
        g.clusters.truncate(2);
        assert_eq!(row(&g, &open, 3), Err(Decline::Layout));
    }
}
