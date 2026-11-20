//! Rust accesses that fit in one page go through the fast page table
//! (`Guest::prepare_fast_pages`) instead of the mapping walk. Both ways must
//! agree on everything: the bytes read, the bytes written, the fault and
//! its reason for unmapped, partly mapped, read-only, no-access, misaligned
//! and device-memory accesses, and the write-watch dirty pages. Two guests
//! with the same mappings, one with the table and one without, run the same
//! random accesses and are compared after each.

use runtime::Guest;
use runtime::guest::{PAGE_SIZE, Permissions, RAM_SIZE};
use runtime::watch::{CONTIGUOUS_BASE, WatchId};

/// Mappings covering every case the fast path must turn down or take.
fn guest(fast: bool) -> (Guest, WatchId) {
    let mut g = Guest::new();
    // Whole RW pages.
    g.map(0x0001_0000, 4 * PAGE_SIZE, 0x0010_0000, Permissions::RW)
        .unwrap();
    // An alias of the first two of them (same physical pages).
    g.map(0x0005_0000, 2 * PAGE_SIZE, 0x0010_0000, Permissions::RW)
        .unwrap();
    // A mapping that ends mid-page: 0x21800.. is unmapped.
    g.map(0x0002_0000, 0x1800, 0x0011_0000, Permissions::RW)
        .unwrap();
    // Read-only, and no-access.
    g.map(0x0003_0000, 2 * PAGE_SIZE, 0x0012_0000, Permissions::READ)
        .unwrap();
    g.map(0x0004_0000, PAGE_SIZE, 0x0013_0000, Permissions::NONE)
        .unwrap();
    // Page-sized but shifted backing (physical not page-aligned).
    g.map(0x0006_0000, 2 * PAGE_SIZE, 0x0014_0800, Permissions::RW)
        .unwrap();
    // Two adjacent mappings with distant physical pages.
    g.map(0x0007_0000, PAGE_SIZE, 0x0015_0000, Permissions::RW)
        .unwrap();
    g.map(0x0007_1000, PAGE_SIZE, 0x0016_0000, Permissions::RW)
        .unwrap();
    // Right below device memory (0xFD000000).
    g.map(0xFCFF_F000, PAGE_SIZE, 0x0017_0000, Permissions::RW)
        .unwrap();
    let id = g.subscribe_writes("test");
    g.watch_writes(id, CONTIGUOUS_BASE, RAM_SIZE as u32)
        .unwrap();
    if fast {
        g.prepare_fast_pages();
    }
    (g, id)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.next() as usize % xs.len()]
    }
}

const BASES: [u32; 11] = [
    0x0001_0000,
    0x0005_0000,
    0x0002_0000,
    0x0002_1000,
    0x0003_0000,
    0x0004_0000,
    0x0006_0000,
    0x0007_0000,
    0x0007_1000,
    0xFCFF_F000,
    0x0009_0000,
];
const LENS: [usize; 14] = [0, 1, 2, 3, 4, 7, 8, 20, 64, 2048, 4095, 4096, 4097, 8192];
const ALIGNS: [u32; 6] = [1, 1, 1, 2, 4, 3];

fn compare(seed: u64, steps: usize) {
    let (mut slow, sid) = guest(false);
    let (mut fast, fid) = guest(true);
    let mut r = Rng(seed);
    for step in 0..steps {
        let addr = r
            .pick(&BASES)
            .wrapping_add(r.next() % 0x2400)
            .wrapping_sub(0x200);
        // Near the end of a page more often, to cross it.
        let addr = if r.next().is_multiple_of(3) {
            (addr | 0xFFF) - r.next() % 8
        } else {
            addr
        };
        let len = r.pick(&LENS);
        let align = r.pick(&ALIGNS);
        let what = format!("seed {seed} step {step}: {addr:#x} +{len} align {align}");
        match r.next() % 4 {
            0 => {
                let (mut a, mut b) = (vec![0xAA; len], vec![0xAA; len]);
                let ra = slow.read_bytes(addr, &mut a, align);
                let rb = fast.read_bytes(addr, &mut b, align);
                assert_eq!(ra, rb, "read {what}");
                assert_eq!(a, b, "read bytes {what}");
            }
            1 => {
                let (mut a, mut b) = (vec![1, 2, 3], vec![1, 2, 3]);
                let ra = slow.read_append(addr, len, &mut a, align);
                let rb = fast.read_append(addr, len, &mut b, align);
                assert_eq!(ra, rb, "append {what}");
                assert_eq!(a, b, "append bytes {what}");
            }
            _ => {
                let data: Vec<u8> = (0..len).map(|_| r.next() as u8).collect();
                let ra = slow.write_bytes(addr, &data, align);
                let rb = fast.write_bytes(addr, &data, align);
                assert_eq!(ra, rb, "write {what}");
                if r.next().is_multiple_of(4) {
                    assert_eq!(slow.take_dirty(sid), fast.take_dirty(fid), "dirty {what}");
                }
            }
        }
        // Permissions change under the table now and then.
        if step % 997 == 500 {
            let p = r.pick(&[Permissions::READ, Permissions::RW, Permissions::NONE]);
            slow.protect_range(0x0001_0000, PAGE_SIZE, p).unwrap();
            fast.protect_range(0x0001_0000, PAGE_SIZE, p).unwrap();
        }
    }
    assert_eq!(slow.take_dirty(sid), fast.take_dirty(fid));
    assert!(
        slow.physical_ram() == fast.physical_ram(),
        "seed {seed}: RAM differs"
    );
}

#[test]
fn fast_page_accesses_match_the_mapping_walk() {
    for seed in 1..=8u64 {
        compare(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15), 20_000);
    }
}
