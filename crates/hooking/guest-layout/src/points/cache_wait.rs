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
/// `0x80e10`, `0x1241b0`, `0x1995a0`, `0x199740`, `0x862e0` (two),
/// `0x8c790`, `0x146e30`, `0x18f260`, `0x194160`, `0x194260` and the movie
/// start `0x156090`.
pub static WAITS: [Wait; 14] = [
    wait("CACHE_WAIT_ALLOC", 0x0012_d4e3, 0x0012_d50d),
    wait("CACHE_WAIT_6A8A0", 0x0006_a927, 0x0006_a951),
    wait("CACHE_WAIT_80E10", 0x0008_0ebf, 0x0008_0ee9),
    wait("CACHE_WAIT_1241B0", 0x0012_4292, 0x0012_42bc),
    wait("CACHE_WAIT_1995A0", 0x0019_963b, 0x0019_9665),
    wait("CACHE_WAIT_199740", 0x0019_97c7, 0x0019_97f1),
    wait("CACHE_WAIT_862E0_A", 0x0008_6381, 0x0008_63ab),
    wait("CACHE_WAIT_862E0_B", 0x0008_643d, 0x0008_6467),
    wait("CACHE_WAIT_8C790", 0x0008_c887, 0x0008_c8b1),
    wait("CACHE_WAIT_146E30", 0x0014_6f0e, 0x0014_6f38),
    wait("CACHE_WAIT_18F260", 0x0018_f30f, 0x0018_f339),
    wait("CACHE_WAIT_194160", 0x0019_41e7, 0x0019_4211),
    wait("CACHE_WAIT_194260", 0x0019_4357, 0x0019_4381),
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

    /// Every wait has retail's one shape: `mov edx,[0x407488]` (6 bytes),
    /// `call 0x3fac30` (5), the peek `mov eax,[edx+0x934]` (6), the two
    /// register reads and the `jne` (14), the PGRAPH read (6), and the
    /// yield's `call 0x2d2084` (5): the resume is 0x2a past the site.
    #[test]
    fn each_wait_skips_one_kick_peek_and_yield() {
        for w in &WAITS {
            assert_eq!(w.resume - w.site.base, 0x2a, "{}", w.site.name);
        }
    }

    /// The 14 game peeks of `[[0x407488]+0x934]` outside the three cache
    /// flushes (`crate::points::CACHE_FLUSH` and its two neighbours, which
    /// are Rust) are each inside one wait (docs/graphics/d3d-nv2a.md,
    /// "NV2A register window").
    #[test]
    fn every_game_fence_peek_is_inside_a_wait() {
        let peeks = [
            0x0006_a932, 0x0008_0eca, 0x0008_638c, 0x0008_6448, 0x0008_c892, 0x0012_429d, 0x0012_d4ee,
            0x0014_6f19, 0x0015_61f6, 0x0018_f31a, 0x0019_41f2, 0x0019_4362, 0x0019_9646, 0x0019_97d2,
        ];
        for pc in peeks {
            let n = WAITS.iter().filter(|w| (w.site.base..w.resume).contains(&pc)).count();
            assert_eq!(n, 1, "{pc:#x}");
        }
    }
}
