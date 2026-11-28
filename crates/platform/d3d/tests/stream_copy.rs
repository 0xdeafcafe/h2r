//! `GuestMemory::read_append` matches `read` and leaves the output alone on a
//! fault, for the sparse test memory and for the runtime's guest adapter.

use d3d::{GuestMemory, SparseMemory};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
    fn bytes(&mut self, n: u32) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

#[test]
fn read_append_matches_read_and_leaves_out_alone_on_a_fault() {
    let mut m = SparseMemory::new();
    m.map(0x1000, 0x3000);
    let bytes = Rng(1).bytes(0x3000);
    m.write(0x1000, &bytes).unwrap();
    // Across page boundaries, after bytes already there.
    let mut out = vec![7u8; 3];
    m.read_append(0x1ff0, 0x1020, &mut out).unwrap();
    assert_eq!(out[..3], [7, 7, 7]);
    assert_eq!(out[3..], bytes[0xff0..0x2010]);
    // Runs off the mapped pages: an error and `out` as it was.
    let before = out.clone();
    assert!(m.read_append(0x3ff0, 0x20, &mut out).is_err());
    assert_eq!(out, before);
    assert!(m.read_append(0xffff_fff0, 0x20, &mut out).is_err());
    assert_eq!(out, before);
    m.read_append(0x2000, 0, &mut out).unwrap();
    assert_eq!(out, before);
}

#[test]
fn guest_adapter_read_append_matches_read_and_keeps_the_fault() {
    use runtime::guest::{FaultReason, Permissions};
    let mut g = runtime::Guest::new();
    g.map_zeroed(0x10000, 0x2000, Permissions::RW).unwrap();
    let bytes = Rng(2).bytes(0x2000);
    g.write_bytes(0x10000, &bytes, 1).unwrap();
    let m = d3d::runtime_glue::GuestAdapter::new(&mut g);
    let mut want = vec![0; 0x30];
    m.read(0x10ff0, &mut want).unwrap();
    let mut out = vec![1u8];
    m.read_append(0x10ff0, 0x30, &mut out).unwrap();
    assert_eq!(out[1..], want);
    assert!(m.take_fault().is_none());
    let before = out.clone();
    assert!(m.read_append(0x11ff0, 0x20, &mut out).is_err());
    assert_eq!(out, before);
    assert_eq!(m.take_fault().unwrap().reason, FaultReason::Unmapped);
}
