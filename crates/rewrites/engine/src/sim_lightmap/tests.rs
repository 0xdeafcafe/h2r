//! The store's fixup and the view's seams, on hand-built blocks.
use super::*;
use crate::sim_native::Memory;

/// Guest memory: one flat span at [`BASE`].
struct Flat(Vec<u8>);

const BASE: u32 = 0x0010_0000;

impl Flat {
    fn new() -> Self {
        Self(vec![0; 0x1000])
    }
}

impl Memory for Flat {
    fn read_bytes(&self, addr: u32, dst: &mut [u8], _: u32) -> Result<(), GuestFault> {
        let o = addr.wrapping_sub(BASE) as usize;
        match self.0.get(o..o + dst.len()) {
            Some(b) => {
                dst.copy_from_slice(b);
                Ok(())
            }
            None => Err(GuestFault {
                addr,
                len: dst.len(),
                access: runtime::guest::Access::Read,
                reason: runtime::guest::FaultReason::Unmapped,
            }),
        }
    }

    fn write_bytes(&mut self, addr: u32, src: &[u8], _: u32) -> Result<(), GuestFault> {
        let o = addr.wrapping_sub(BASE) as usize;
        self.0[o..o + src.len()].copy_from_slice(src);
        Ok(())
    }
}

/// A block: `blkh`, the resource area's size, a 0x40-byte section, the
/// area, `blkf`. Resources: a tag block of 8-byte elements at section
/// `+0x00`, data at `+0x08`, a descriptor array (one 0x20-byte entry) at
/// `+0x10` and vertex data for its entry 0.
fn block() -> (Info, Vec<Resource>, Vec<u8>) {
    let section = 0x40usize;
    let area = 0x80usize;
    let mut b = Vec::new();
    b.extend_from_slice(b"hklb");
    b.extend_from_slice(&(area as u32).to_le_bytes());
    b.resize(8 + section + area, 0);
    b.extend_from_slice(b"fklb");
    let s = 8;
    let a = s + section;
    // The descriptor's own fields, as the disc holds them: +1 the stride
    // (4), +4 a base of 0, +8 a header of 0.
    b[a + 0x24 + 1] = 4;
    // Element data, for the reads.
    b[a + 4..a + 8].copy_from_slice(&0x1111_2222u32.to_le_bytes());
    let resources = vec![
        Resource { kind: 0, field: 0x00, secondary: 8, size: 0x10, offset: 4 },
        Resource { kind: 1, field: 0x08, secondary: 0xffff, size: 0xc, offset: 0x18 },
        Resource { kind: 0, field: 0x10, secondary: 0x20, size: 0x20, offset: 0x24 },
        Resource { kind: 2, field: 0x10, secondary: 0, size: 0x10, offset: 0x48 },
        Resource { kind: 0, field: 0x18, secondary: 4, size: 4, offset: u32::MAX },
    ];
    let info = Info { raw: 0x40, size: b.len() as u32, section: section as u32 };
    (info, resources, b)
}

fn word(store: &Store, token: u32) -> u32 {
    let mut b = [0u8; 4];
    store.read(token, &mut b).unwrap();
    u32::from_le_bytes(b)
}

#[test]
fn the_fixup_points_each_resource_at_its_copy_in_the_store() {
    let (info, resources, bytes) = block();
    let mut b = Store::builder();
    b.add_block(BASE, info, &resources, &bytes).unwrap();
    let store = b.finish();
    let s = store.section(BASE).unwrap();
    assert!(is_token(s));
    let data = s + 0x40;
    // Kind 0: the count and the data.
    assert_eq!((word(&store, s), word(&store, s + 4)), (2, data + 4));
    assert_eq!(word(&store, word(&store, s + 4)), 0x1111_2222);
    // Kind 1: the size and the data.
    assert_eq!((word(&store, s + 8), word(&store, s + 0xc)), (0xc, data + 0x18));
    // Kind 2: entry 0 of the array at +0x10.
    let d = word(&store, s + 0x14);
    assert_eq!(d, data + 0x24);
    assert_eq!(word(&store, d) >> 16, 0x10 / 4);
    assert_eq!(word(&store, d + 0xc), data + 0x48);
    assert_eq!(word(&store, d + 0x10), d + 0x14);
    assert_eq!(word(&store, d + 0x14), 1);
    assert_eq!(word(&store, d + 0x18), (data + 0x48) & 0x0fff_ffff);
    assert_eq!(word(&store, d + 0x1c), 0);
    // A skipped resource leaves its field as the disc has it.
    assert_eq!(word(&store, s + 0x18), 0);
}

#[test]
fn a_block_of_the_wrong_size_is_refused() {
    let (info, resources, mut bytes) = block();
    bytes.pop();
    let mut b = Store::builder();
    assert!(matches!(b.add_block(BASE, info, &resources, &bytes), Err(BuildError::Framing { .. })));
}

#[test]
fn the_view_reads_tokens_from_the_store_and_the_rest_from_the_guest() {
    let (info, resources, bytes) = block();
    let mut b = Store::builder();
    b.add_block(BASE, info, &resources, &bytes).unwrap();
    b.add_pixels(BASE + 0x100, &[1, 2, 3, 4]).unwrap();
    let store = b.finish();
    let mut g = Flat::new();
    g.set_u32(BASE + 0x200, 0xdead_beef).unwrap();
    let mut v = View::new(&g, &store);
    assert_eq!(v.u32(BASE + 0x200).unwrap(), 0xdead_beef);
    let p = v.pixels(BASE + 0x100).unwrap().unwrap();
    assert_eq!(v.u32(p).unwrap(), 0x0403_0201);
    assert_eq!(v.prt_entries(), 0);
    assert!(v.set_u32(BASE + 0x200, 0).is_err());
}

#[test]
fn an_in_tag_section_is_read_where_the_tag_points() {
    let store = Store::empty();
    let mut g = Flat::new();
    let info = BASE + 0x300;
    g.set_u32(info, u32::MAX).unwrap();
    g.set_u16(info + 0x1c, 0x24).unwrap();
    g.set_u8(info + 0x1e, 1).unwrap();
    g.set_u32(info + 0x24, 1).unwrap();
    g.set_u32(info + 0x28, BASE + 0x400).unwrap();
    let v = View::new(&g, &store);
    assert_eq!(v.section(info).unwrap(), Some(BASE + 0x400));
    // Not loaded and not held: the guest's false.
    let none = BASE + 0x340;
    g.set_u32(none, 0x40).unwrap();
    let v = View::new(&g, &store);
    assert_eq!(v.section(none).unwrap(), None);
}

#[test]
fn the_tick_record_takes_the_sample_and_its_mark() {
    let mut g = Flat::new();
    let records = BASE + 0x800;
    let fill = Fill { object: 0xe0_0001, record: records };
    assert_eq!(fill.args(), [records + 0x30, 1]);
    for i in 0..5u32 {
        g.set_u32(records + 0x30 + 4 * i, 0x100 + i).unwrap();
    }
    fill.finish(&mut g, false).unwrap();
    assert_eq!(g.u8(records + 2).unwrap(), 0);
    fill.finish(&mut g, true).unwrap();
    assert_eq!(g.u8(records + 2).unwrap(), 1);
    assert_eq!(g.u32(records + 0x28).unwrap(), 0x103);
    assert_eq!(g.u32(records + 0x2c).unwrap(), 0x104);
}

#[test]
fn only_the_two_tick_reads_are_filled() {
    assert_eq!(Site::of(0x0017_96e2), Some(Site::Update));
    assert_eq!(Site::of(0x0017_b61f), Some(Site::New));
    assert_eq!(Site::of(0x0004_ba21), None);
}

/// The record get's world: object `OBJECT` (index 2) with body `BODY`,
/// and a two-record cache.
mod record {
    use super::*;
    use crate::{FlatMemory, GuestMemory};
    use crate::datum::layout;

    const OBJECT: u32 = 0x0001_0002;
    const HEADERS: u32 = 0x2000;
    const BODY: u32 = 0x3000;
    const ARRAY: u32 = 0x1_0000;
    const BITMAP: u32 = 0x1_0100;
    const DATA: u32 = 0x1_1000;
    const STACK: u32 = 0x8_0000;

    fn world() -> FlatMemory {
        let mut m = FlatMemory::new(0, 0x60_0000);
        m.set_u32(0x004e_0300, 0x1000).unwrap();
        m.set_u32(0x1000 + 0x44, HEADERS).unwrap();
        m.set_u32(HEADERS + 2 * 12 + 8, BODY).unwrap();
        m.set_u32(BODY + 0xcc, u32::MAX).unwrap();
        m.set_u32(RECORDS, ARRAY).unwrap();
        m.set_u32(ARRAY + layout::MAXIMUM_COUNT, 2).unwrap();
        m.set_u32(ARRAY + layout::ELEMENT_SIZE, 0x100).unwrap();
        m.set_u32(ARRAY + layout::DATA, DATA).unwrap();
        m.set_u32(ARRAY + layout::BITMAP, BITMAP).unwrap();
        m.set_u32(STACK, 0x0017_96e2).unwrap();
        m.set_u32(STACK + 4, OBJECT).unwrap();
        m
    }

    fn cpu() -> CpuAbi {
        let mut c = CpuAbi::default();
        c.esp = STACK;
        c.ebx = 0x11;
        c.esi = 0x22;
        c.edi = 0x33;
        c.ebp = 0x44;
        c.mxcsr = 0x1f80;
        c.x87.control = 0x027f;
        c
    }

    #[test]
    fn the_objects_own_record_comes_back_as_ret_4_leaves_it() {
        let mut m = world();
        let handle = 0x8001_0001;
        m.set_u32(BODY + 0xcc, handle).unwrap();
        m.set_u32(DATA + 0x100 + 4, OBJECT).unwrap();
        let mut c = cpu();
        let ret = record_call(&mut c, &mut m).unwrap();
        assert_eq!(ret, 0x0017_96e2);
        assert_eq!(c.eax, handle);
        assert_eq!(c.esp, STACK + 8);
        assert_eq!((c.ebx, c.esi, c.edi, c.ebp), (0x11, 0x22, 0x33, 0x44));
    }

    #[test]
    fn a_full_cache_with_nothing_old_enough_gives_none() {
        let mut m = world();
        m.set_u32(ARRAY + layout::HIGH_WATER, 2).unwrap();
        m.set_u32(ARRAY + layout::ACTIVE_COUNT, 2).unwrap();
        m.set_u32(BITMAP, 0b11).unwrap();
        for i in 0..2u32 {
            let entry = DATA + 0x100 * i;
            m.set_u16(entry, 0x8001 + i as u16).unwrap();
            m.set_u32(entry + 4, 0x0005_0009).unwrap();
            m.set_u32(entry + 0xc, 100).unwrap();
        }
        // Frame 100: both records are 0 frames old, under the floor.
        m.set_u32(0x004b_a034, 100).unwrap();
        m.set_f32(0x0044_09d4, 10.0).unwrap();
        m.set_f32(0x0045_dccc, 1000.0).unwrap();
        let mut c = cpu();
        record_call(&mut c, &mut m).unwrap();
        assert_eq!(c.eax, u32::MAX);
        assert_eq!(c.esp, STACK + 8);
        assert_eq!(m.u32(BODY + 0xcc).unwrap(), u32::MAX);
    }
}
