//! Complete action selection in the primary graph and an inherited graph.
//! The cache callback shortens the active resource list between requests.
use super::*;

const ENTRY: u32 = 0x1cc3f0;
const MANAGER: u32 = DATA + 0x1000;
const TAGS: u32 = DATA + 0x1300;
const GRAPH: u32 = DATA + 0x1500;
const PARENT: u32 = DATA + 0x1800;
const LINKS: u32 = DATA + 0x1a00;
const NAMES: u32 = DATA + 0x1b00;
const CHOSEN: u32 = DATA + 0x1c00;
const REFERENCE: u32 = DATA + 0x1d00;
const RETURNED: u32 = DATA + 0x1e00;
const MATCH: u32 = DATA + 0x1f00;
const INDICES: u32 = DATA + 0x2000;
const GROUPS: u32 = DATA + 0x2200;

pub(super) fn cases(h: &mut Harness<'_>) {
    // Select at each progressively less specific action/mode lookup and
    // once miss every lookup. The final successful mode lookup traverses
    // all resource sites with a live callback under both graphs.
    let selected_sites = [
        0x1cc471, 0x1cc4b3, 0x1cc505, 0x1cc528, 0x1cc552, 0x1cc57a, 0x1cc5a8, 0,
    ];
    for (i, selected_site) in selected_sites.into_iter().enumerate() {
        let full = selected_site == 0x1cc5a8;
        let seed = 0xce00 + i as u32;
        h.case(
            ENTRY,
            seed,
            move |c, m| {
                c.eax = MANAGER;
                c.ecx = CHOSEN;
                c.edx = NAMES;
                word(m, MANAGER + 0x68, 0x12340001);
                word(m, guest_layout::points::TAG_ENTRIES.base, TAGS);
                word(m, TAGS + 16 + 8, GRAPH);
                word(m, TAGS + 32 + 8, PARENT);
                word(m, GRAPH + 0x4c, u32::from(full));
                word(m, GRAPH + 0x50, LINKS);
                word(m, LINKS + 4, 2);
                for (idx, graph) in [GRAPH, PARENT].into_iter().enumerate() {
                    let groups = GROUPS + idx as u32 * 0x100;
                    word(m, graph + 0xb0, groups);
                    // Inactive, active, active. The callback removes the last
                    // group before the caller's loop retests its bound.
                    for index in 0..3 {
                        word(m, groups + index * 20 + 16, u32::from(index != 0));
                        word(m, INDICES + index * 4, index);
                    }
                }
                word(m, MATCH + 0x28, INDICES);
                for n in 0..4 {
                    word(m, NAMES + n * 4, seed.wrapping_mul(101) + n);
                }
                for n in 1..=4 {
                    word(m, STACK + 4 * n, seed + n);
                }
                word(m, STACK + 20, 6);
                word(m, STACK + 24, REFERENCE);
            },
            move |target, site, c, m| {
                match target {
                    0x1cc2f0 => 8,
                    0x1db170 => {
                        let index = if site == selected_site { seed } else { 0xffff };
                        word(m, RETURNED, (index << 16) | 1);
                        c.eax = RETURNED;
                        32
                    }
                    0x1db120 => {
                        c.eax = if site == selected_site { MATCH } else { 0 };
                        8
                    }
                    0x1dd560 => {
                        // Some selections have a missing prefix. One traverses
                        // the entire three-level match at every fallback.
                        c.eax = if full || (site + seed) % 4 == 0 {
                            MATCH
                        } else {
                            0
                        };
                        word(m, MATCH + 0x24, 3);
                        12
                    }
                    0x1236f0 => {
                        word(m, MATCH + 0x24, 2);
                        c.eax = seed.rotate_left(site % 32);
                        0
                    }
                    _ => panic!("unexpected action prediction call {target:#x}"),
                }
            },
        );
    }
}
