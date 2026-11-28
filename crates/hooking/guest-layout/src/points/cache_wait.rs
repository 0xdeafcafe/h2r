//! The blocking cache allocations' wait (docs/graphics/d3d-nv2a.md,
//! "NV2A register window").
//!
//! Retail's allocation retries `0x12d2f0`, pumps the cache with `0x12c600`
//! up to a count, then until a timeout kicks the GPU (`mov edx,[0x407488]`,
//! `CDevice_KickOff` `0x3fac30`), peeks the device fence `[[edx+0x934]+..]`
//! and yields with `SwitchToThread` (`0x2d2084`) before the next try. No
//! D3D device exists, so a site is the `mov edx,[0x407488]` that opens the
//! kick, and its resume the `jmp` after the yield's call: a `before` hook
//! yields the thread and resumes there, leaving the retries, the pump and
//! the timeout to retail. A site follows a `jbe`, never a call, and a
//! resume is the instruction after a call, which the generated resume
//! table routes.
use super::{Point, Sig};

const DOC: &str = "docs/graphics/d3d-nv2a.md";

/// One wait: the kick's first instruction and the instruction after the
/// yield's call.
#[derive(Debug)]
pub struct Wait {
    pub site: Point,
    pub resume: u32,
}

const fn wait(name: &'static str, site: u32, resume: u32) -> Wait {
    Wait { site: Point::new(name, site, Sig::Site, DOC), resume }
}

/// The generic allocation `0x12d400`, the inlined copies in `0x6a8a0`,
/// `0x80e10`, `0x1241b0`, `0x1995a0`, `0x199740`, `0x862e0` (two) and the
/// movie start `0x156090`.
pub static WAITS: [Wait; 9] = [
    wait("CACHE_WAIT_ALLOC", 0x0012_d4e3, 0x0012_d50d),
    wait("CACHE_WAIT_6A8A0", 0x0006_a927, 0x0006_a951),
    wait("CACHE_WAIT_80E10", 0x0008_0ebf, 0x0008_0ee9),
    wait("CACHE_WAIT_1241B0", 0x0012_4292, 0x0012_42bc),
    wait("CACHE_WAIT_1995A0", 0x0019_963b, 0x0019_9665),
    wait("CACHE_WAIT_199740", 0x0019_97c7, 0x0019_97f1),
    wait("CACHE_WAIT_862E0_A", 0x0008_6381, 0x0008_63ab),
    wait("CACHE_WAIT_862E0_B", 0x0008_643d, 0x0008_6467),
    wait("CACHE_WAIT_MOVIE", 0x0015_61eb, 0x0015_6215),
];

/// Every site and resume, each once.
pub fn instruction_entries() -> impl Iterator<Item = u32> {
    WAITS.iter().flat_map(|w| [w.site.base, w.resume])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for a in instruction_entries() {
            assert!(seen.insert(a), "{a:#010x}");
        }
    }
}
