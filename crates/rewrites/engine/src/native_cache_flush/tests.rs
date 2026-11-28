//! The flushes on a synthetic cache: the guest callees are played by the
//! test at each call, as retail's would answer.

use super::*;
use runtime::guest::Permissions;
use runtime::TransferAbi;

const STACK: u32 = 0x0010_0000;
const CALLER: u32 = 0x00de_ad00;
const ARRAY: u32 = 0x0020_0000;
const DATA: u32 = 0x0020_1000;
const RESOURCES: u32 = 0x0021_0000;
const OWNED_ARRAY: u32 = 0x0022_0000;
const OWNED_DATA: u32 = 0x0022_1000;
const PENDING_NODE: u32 = 0x0023_0000;
const COMPLETION: u32 = 0x0030_0000;

fn guest() -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(STACK - 0x1000, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(0x004e_0000, 0x10000, Permissions::RW).unwrap();
    g.map_zeroed(ARRAY, 0x40000, Permissions::RW).unwrap();
    let mut c = CpuAbi::default();
    c.esp = STACK;
    g.set_u32(c.esp, CALLER).unwrap();
    (c, g)
}

/// A data array at `array` with `used` set: `+0x24` 8-byte datums at
/// `data`, `+0x38` the count, `+0x48` the bitmap, `+0x29` its used byte;
/// datum `i` has salt `0x100 + i` and the resource at `RESOURCES + i * 0x40`.
fn array(g: &mut Guest, array: u32, data: u32, count: u32, used: &[u32]) {
    g.set_u8(array + 0x29, 1).unwrap();
    g.set_u32(array + 0x24, 8).unwrap();
    g.set_u32(array + 0x38, count).unwrap();
    g.set_u32(array + 0x44, data).unwrap();
    g.set_u32(array + 0x48, array + 0x100).unwrap();
    for &i in used {
        let word = array + 0x100 + (i >> 5) * 4;
        let bits = g.u32(word).unwrap();
        g.set_u32(word, bits | 1 << (i & 31)).unwrap();
        g.set_u16(data + i * 8, 0x100 + i as u16).unwrap();
        g.set_u32(data + i * 8 + 4, RESOURCES + i * 0x40).unwrap();
    }
}

/// What the test's callees saw.
#[derive(Default, Debug, PartialEq)]
struct Seen {
    deleted: Vec<(u32, u32)>,
    resets: Vec<u32>,
    unloaded: Vec<u32>,
    completions: Vec<(u32, u32)>,
}

/// Run `body` from its entry to its return, answering each callout.
fn run(body: &Body, c: &mut CpuAbi, g: &mut Guest, used: &dyn Fn(u32, u32) -> bool) -> Seen {
    let mut seen = Seen::default();
    let mut pc = body.point.base;
    loop {
        let t: TransferAbi = body.segment(c, g, pc).unwrap();
        if t.kind == runtime::abi::RETURN {
            assert_eq!(t.target_pc, CALLER);
            return seen;
        }
        assert_eq!(t.kind, runtime::abi::CALL, "{pc:#x}");
        let site = frame::pop(c, g).unwrap();
        assert_eq!(site, t.resume_pc);
        match t.target_pc {
            DATUM_NEXT => {
                let (array, from) = (c.ecx, c.edx);
                let count = g.u32(array + 0x38).unwrap();
                c.eax = (from..count).find(|&i| used(array, i)).unwrap_or(u32::MAX);
            }
            DATUM_DELETE => seen.deleted.push((c.edx, c.eax)),
            ALLOCATOR_RESET => seen.resets.push(c.eax),
            RESOURCE_UNLOAD => {
                seen.unloaded.push(frame::pop(c, g).unwrap());
            }
            COMPLETION => {
                let a = frame::pop(c, g).unwrap();
                let b = frame::pop(c, g).unwrap();
                seen.completions.push((a, b));
                g.set_u32(PENDING, 0).unwrap();
            }
            other => panic!("unexpected call {other:#x} from {pc:#x}"),
        }
        pc = site;
    }
}

#[test]
fn the_first_flush_runs_the_completions_and_deletes_every_entry() {
    let (mut c, mut g) = guest();
    array(&mut g, ARRAY, DATA, 40, &[1, 33]);
    for i in [1u32, 33] {
        g.set_u8(RESOURCES + i * 0x40 + 0xf, 0xff).unwrap();
    }
    g.set_u32(ENTRIES, ARRAY).unwrap();
    g.set_u32(ALLOCATOR, 0x0012_3450).unwrap();
    g.set_u32(PENDING, PENDING_NODE).unwrap();
    g.set_u32(PENDING_NODE + 8, 7).unwrap();
    g.set_u32(PENDING_NODE + 0x10, 9).unwrap();
    g.set_u32(PENDING_NODE + 0x18, COMPLETION).unwrap();
    (c.esi, c.ebx, c.ebp) = (0x51, 0x52, 0x53);
    let seen = run(&BODIES[0], &mut c, &mut g, &|_, _| unreachable!("the first flush scans its bitmap itself"));
    assert_eq!(seen.completions, vec![(7, 9)]);
    assert_eq!(seen.resets, vec![0x0012_3450]);
    assert_eq!(seen.deleted, vec![(ARRAY, 0x101 << 16 | 1), (ARRAY, 0x121 << 16 | 33)]);
    for i in [1u32, 33] {
        assert_eq!(g.u8(RESOURCES + i * 0x40 + 0xf).unwrap(), 0xfb);
    }
    assert_eq!((c.esi, c.ebx, c.ebp, c.esp), (0x51, 0x52, 0x53, STACK + 4));
}

#[test]
fn the_second_flush_resets_its_allocator_and_deletes_every_entry() {
    let (mut c, mut g) = guest();
    array(&mut g, ARRAY, DATA, 4, &[0, 2]);
    g.set_u32(SECOND_ENTRIES, ARRAY).unwrap();
    g.set_u32(SECOND_ALLOCATOR, 0x0045_6780).unwrap();
    for i in [0u32, 2] {
        g.set_u8(RESOURCES + i * 0x40 + 0x1f, 0xff).unwrap();
    }
    (c.esi, c.edi) = (0x61, 0x62);
    let used = |_: u32, i: u32| i == 0 || i == 2;
    let seen = run(&BODIES[1], &mut c, &mut g, &used);
    assert_eq!(seen.resets, vec![0x0045_6780]);
    assert_eq!(seen.deleted, vec![(ARRAY, 0x100 << 16), (ARRAY, 0x102 << 16 | 2)]);
    for i in [0u32, 2] {
        assert_eq!(g.u8(RESOURCES + i * 0x40 + 0x1f).unwrap(), 0xfb);
    }
    assert_eq!((c.esi, c.edi, c.esp), (0x61, 0x62, STACK + 4));
}

#[test]
fn the_second_flush_with_no_allocator_does_nothing() {
    let (mut c, mut g) = guest();
    c.esi = 0x61;
    let seen = run(&BODIES[1], &mut c, &mut g, &|_, _| unreachable!());
    assert_eq!(seen, Seen::default());
    assert_eq!((c.esi, c.esp), (0x61, STACK + 4));
}

#[test]
fn an_owners_unload_takes_only_its_resources() {
    let (mut c, mut g) = guest();
    const OWNER: u32 = 5;
    // Owned entries: datum `i`'s `+8` is its resource; 1 is the owner's.
    g.set_u8(OWNED_ARRAY + 0x29, 1).unwrap();
    g.set_u32(OWNED_ARRAY + 0x24, 0x10).unwrap();
    g.set_u32(OWNED_ARRAY + 0x38, 3).unwrap();
    g.set_u32(OWNED_ARRAY + 0x44, OWNED_DATA).unwrap();
    for i in 0..3u32 {
        let resource = RESOURCES + 0x1000 + i * 0x40;
        g.set_u32(OWNED_DATA + i * 0x10 + 8, resource).unwrap();
        g.set_u32(resource + 0x18, if i == 1 { OWNER } else { 9 }).unwrap();
    }
    g.set_u32(OWNED, OWNED_ARRAY).unwrap();
    // Cache entries: 0 and 3 belong to the owner.
    array(&mut g, ARRAY, DATA, 4, &[0, 2, 3]);
    for i in [0u32, 2, 3] {
        g.set_u32(RESOURCES + i * 0x40 + 0x18, if i == 2 { 9 } else { OWNER }).unwrap();
    }
    g.set_u32(SECOND_ENTRIES, ARRAY).unwrap();
    (c.ebx, c.esi, c.edi) = (OWNER, 0x71, 0x72);
    let used = |a: u32, i: u32| if a == OWNED_ARRAY { i < 3 } else { matches!(i, 0 | 2 | 3) };
    let seen = run(&BODIES[2], &mut c, &mut g, &used);
    assert_eq!(seen.unloaded, vec![RESOURCES + 0x1000 + 0x40]);
    assert_eq!(seen.deleted, vec![(ARRAY, 0x100 << 16), (ARRAY, 0x103 << 16 | 3)]);
    assert_eq!((c.ebx, c.esi, c.edi, c.esp), (OWNER, 0x71, 0x72, STACK + 4));
}

#[test]
fn the_table_takes_every_body() {
    for body in BODIES {
        assert!(TABLE.index(body.point.base).is_some(), "{}", body.point.name);
    }
}
