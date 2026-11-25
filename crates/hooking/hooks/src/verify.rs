//! Per-point verification at install: each chain's point is checked
//! against the code at its address in the loaded image
//! (`guest_layout::point_shapes`). A point that is not what it says turns its
//! features off with the point's name, the way a missing original does,
//! instead of hooking the middle of another function.
//!
//! A site on a call's continuation is refused the same way: the image's
//! resume table holds the address and a call ends there. A return is routed
//! to the resume table first (`runtime::Runtime::resolve_in`), which
//! resumes the caller's generated code without consulting an override, so
//! the hook would never run.

use std::collections::{BTreeMap, BTreeSet};

use guest_layout::point_shapes::{self, Verdict};
use guest_layout::points::Sig;
use runtime::Runtime;

use crate::plan::Chain;

/// The XBE header's base. Its magic is there when a real image is loaded;
/// synthetic test runtimes have none, and nothing is checked on them.
const IMAGE_HEADER: u32 = 0x0001_0000;
const MAGIC: [u8; 4] = *b"XBEH";

/// The chains whose point does not have its shape, or is a site on a
/// call's continuation, by address, with why.
pub(crate) fn mismatches<'a>(
    rt: &Runtime,
    chains: impl IntoIterator<Item = &'a Chain>,
) -> BTreeMap<u32, String> {
    let chains: Vec<&Chain> = chains.into_iter().collect();
    // The resume table first, with the memory lock not held.
    let resumed: BTreeSet<u32> = rt.read_dispatch(|d| {
        chains
            .iter()
            .filter(|c| matches!(c.point.sig, Sig::Site) && d.has_resume(c.addr))
            .map(|c| c.addr)
            .collect()
    });
    let mem = rt.memory();
    let guest: &runtime::Guest = &mem;
    let read = |addr: u32, buf: &mut [u8]| guest.read_bytes(addr, buf, 1).is_ok();
    let mut magic = [0u8; 4];
    if !read(IMAGE_HEADER, &mut magic) || magic != MAGIC {
        return BTreeMap::new();
    }
    chains
        .into_iter()
        .filter_map(|c| match point_shapes::verify(c.point, c.addr, &read) {
            Verdict::Mismatch(why) => Some((c.addr, why)),
            Verdict::Holds | Verdict::Unchecked | Verdict::Unreadable => resumed
                .contains(&c.addr)
                .then(|| point_shapes::continuation(c.point, c.addr, &read))
                .flatten()
                .map(|why| (c.addr, why)),
        })
        .collect()
}
