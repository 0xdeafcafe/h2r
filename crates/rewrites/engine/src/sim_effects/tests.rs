//! Synthetic checks of the ports on a small guest. The retail comparison
//! runs in `tests/sim_effects_differential.rs`.
use super::*;
use crate::function_eval::constant::ONE;
use guest_layout::points;
use runtime::guest::Permissions;

const STACK: u32 = 0x007f_0000;
const RETURN: u32 = 0xdead_0000;
const NONE: u32 = u32::MAX;
const DECALS: u32 = 0x004e_a950;
const GROUPS: u32 = 0x004e_a94c;
const TRAILS: u32 = 0x004e_a944;
const HEADER: u32 = 0x0060_0000;
const DATA: u32 = 0x0061_0000;
const BUCKETS: u32 = 0x0062_0000;

fn guest() -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0, 0x0080_0000, Permissions::RW).unwrap();
    g.set_u32(STACK, RETURN).unwrap();
    g
}

fn cpu() -> CpuAbi {
    CpuAbi { esp: STACK, mxcsr: 0x1f80, ..Default::default() }
}

fn port(base: u32) -> &'static Port {
    ports().find(|p| p.point.base == base).expect("port registered")
}

fn run(port: &Port, c: &mut CpuAbi, g: &mut Guest, pc: u32) -> TransferAbi {
    assert!((port.supported)(c, g, pc));
    port.invoke(c, g, pc).unwrap()
}

fn decal_pool(g: &mut Guest) {
    g.set_u32(DECALS, HEADER).unwrap();
    g.set_u32(HEADER + 0x44, DATA).unwrap();
    g.set_u32(HEADER + 0x20, 16).unwrap();
    g.set_u32(GROUPS, BUCKETS).unwrap();
    for bucket in 0..2048 {
        g.set_u32(BUCKETS + 4 * bucket, NONE).unwrap();
    }
}

fn record(handle: u32) -> u32 {
    DATA + (handle & 0xffff) * 0x40
}

fn head(mode: u32, cluster: u32) -> u32 {
    BUCKETS + (mode * 512 + cluster) * 4
}

fn link(g: &mut Guest, mode: u32, cluster: u32, handle: u32) {
    let mut c = cpu();
    c.edx = mode;
    c.ebx = cluster;
    c.edi = handle;
    let x = run(port(0x17ce00), &mut c, g, 0x17ce00);
    assert_eq!(x.target_pc, RETURN);
    assert_eq!(c.esp, STACK + 4);
}

#[test]
fn a_decal_links_at_the_front_of_its_bucket() {
    let mut g = guest();
    decal_pool(&mut g);
    link(&mut g, 1, 5, 0x8100_0001);
    link(&mut g, 1, 5, 0x8200_0002);
    assert_eq!(g.u32(head(1, 5)).unwrap(), 0x8200_0002);
    let first = record(1);
    let second = record(2);
    assert_eq!(g.u32(second + 0x30).unwrap(), NONE);
    assert_eq!(g.u32(second + 0x34).unwrap(), 0x8100_0001);
    assert_eq!(g.u32(first + 0x30).unwrap(), 0x8200_0002);
    assert_eq!(g.u32(first + 0x34).unwrap(), NONE);
    assert_eq!(g.u16(second + 0x0a).unwrap(), 5);
    assert_eq!(g.u8(second + 3).unwrap(), 1);
    assert_eq!(g.u16(second + 8).unwrap(), u16::MAX);
}

#[test]
fn unlinking_the_front_decal_promotes_its_neighbour() {
    let mut g = guest();
    decal_pool(&mut g);
    link(&mut g, 1, 5, 0x8100_0001);
    link(&mut g, 1, 5, 0x8200_0002);
    let mut c = cpu();
    c.eax = 0x8200_0002;
    c.esi = 0x1111;
    c.edi = 0x2222;
    c.esp = STACK;
    let x = run(port(0x17d860), &mut c, &mut g, 0x17d860);
    // The unlink hands the slot back through the datum allocator.
    assert_eq!(x.target_pc, 0x16ba40);
    assert_eq!((c.esi, c.edi), (0x1111, 0x2222));
    assert_eq!(g.u32(head(1, 5)).unwrap(), 0x8100_0001);
    assert_eq!(g.u32(record(1) + 0x30).unwrap(), NONE);
}

#[test]
fn unlinking_the_last_decal_empties_its_bucket() {
    let mut g = guest();
    decal_pool(&mut g);
    link(&mut g, 2, 7, 0x8100_0003);
    let mut c = cpu();
    c.eax = 0x8100_0003;
    run(port(0x17d860), &mut c, &mut g, 0x17d860);
    assert_eq!(g.u32(head(2, 7)).unwrap(), NONE);
}

const TRAIL_HEADER: u32 = 0x0063_0000;
const TRAIL_DATA: u32 = 0x0063_1000;
const TAGS: u32 = 0x0063_2000;
const DEFINITION: u32 = 0x0063_3000;

fn trail(g: &mut Guest, rate: f32, scale_flag: u8, countdown: f32, dt: f32) {
    g.set_u32(TRAILS, TRAIL_HEADER).unwrap();
    g.set_u32(TRAIL_HEADER + 0x44, TRAIL_DATA).unwrap();
    g.set_u32(points::TAG_ENTRIES.base, TAGS).unwrap();
    // The trail's definition is tag slot 3, entry 16 bytes, pointer at +8.
    g.set_u32(TAGS + 3 * 16 + 8, DEFINITION).unwrap();
    g.set_u32(TRAIL_DATA + 4, 3).unwrap();
    g.set_u32(DEFINITION + 4, rate.to_bits()).unwrap();
    g.set_u8(DEFINITION + 2, scale_flag).unwrap();
    g.set_u32(TRAIL_DATA + 0x14, 2.0f32.to_bits()).unwrap();
    g.set_u32(TRAIL_DATA + 0x24, countdown.to_bits()).unwrap();
    g.set_u32(ONE, 1.0f32.to_bits()).unwrap();
    g.set_u32(STACK + 4, dt.to_bits()).unwrap();
}

fn clock(g: &mut Guest) -> (u32, f32) {
    let mut c = cpu();
    c.eax = 0;
    let x = run(port(0x17c040), &mut c, g, 0x17c040);
    assert_eq!(x.target_pc, RETURN);
    // The caller pops its argument.
    assert_eq!(c.esp, STACK + 8);
    assert_eq!(c.ecx, TRAIL_DATA);
    (c.eax & 0xffff, f32::from_bits(g.u32(TRAIL_DATA + 0x24).unwrap()))
}

#[test]
fn a_contrail_emits_one_point_per_elapsed_interval() {
    let mut g = guest();
    trail(&mut g, 10.0, 0, 0.05, 0.27);
    let (pulses, left) = clock(&mut g);
    assert_eq!(pulses, 3);
    assert!((left - 0.08).abs() < 1e-5, "{left}");
}

#[test]
fn a_scaled_contrail_rate_shortens_the_interval() {
    let mut g = guest();
    // Rate 5 scaled by the record's 2.0 is ten points a second.
    trail(&mut g, 5.0, 1, 0.05, 0.27);
    let (pulses, _) = clock(&mut g);
    assert_eq!(pulses, 3);
}

#[test]
fn a_contrail_short_of_its_countdown_only_counts_down() {
    let mut g = guest();
    trail(&mut g, 10.0, 0, 0.5, 0.25);
    let (pulses, left) = clock(&mut g);
    assert_eq!(pulses, 0);
    assert!((left - 0.25).abs() < 1e-6);
}

#[test]
fn a_runaway_contrail_clock_is_left_to_the_original() {
    let mut g = guest();
    trail(&mut g, 10_000.0, 0, 0.0, 1.0);
    let c = cpu();
    assert!(!(port(0x17c040).supported)(&c, &g, 0x17c040));
}

#[test]
fn ports_are_disjoint_and_named_once() {
    let mut seen = std::collections::BTreeSet::new();
    for p in ports() {
        for pc in std::iter::once(p.point.base).chain(p.sites.iter().copied()) {
            assert!(seen.insert(pc), "{pc:#x} is claimed twice");
        }
    }
}
