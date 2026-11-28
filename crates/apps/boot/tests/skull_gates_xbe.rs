//! The AI skulls' campaign gates (`guest_layout::points::skull_gates`) against
//! the retail XBE: each site is a short `jne` falling through to the load
//! of its skull's byte, and those loads are every absolute reference to
//! an AI skull's byte but Famine's multiplayer reader. Addresses and
//! opcodes only; no bytes are kept.
//!
//! `H2_XBE=/path/default.xbe cargo xtask test -p boot -- --ignored skull_gates`

use guest_layout::points::skull_gates::GATES;

/// The skull array (`SKULLS`).
const ARRAY: u32 = 0x004f_55dc;
/// Famine's reader in `WEAPON_FROM_PLACEMENT` (`0xfda00`): the placed
/// weapon's rounds, which the multiplayer path answers at the entry.
const FAMINE_PLACED: u32 = 0x000f_da55;

fn xbe_bytes() -> Vec<u8> {
    std::fs::read(std::env::var_os("H2_XBE").expect("set H2_XBE to a retail default.xbe")).unwrap()
}

/// The address the instruction at `at` loads a byte from: `mov al, [imm]`
/// (`a0`) or `mov cl, [imm]` (`8a 0d`).
fn byte_load(x: &xbe::Xbe, at: u32) -> (u32, u32) {
    let b = x.bytes_at(at, 6).unwrap();
    match b {
        [0xa0, ..] => (at + 1, u32::from_le_bytes(b[1..5].try_into().unwrap())),
        [0x8a, 0x0d, ..] => (at + 2, u32::from_le_bytes(b[2..6].try_into().unwrap())),
        _ => panic!("{at:#010x} is not a byte load: {b:02x?}"),
    }
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn each_gate_is_a_jne_falling_through_to_its_skulls_byte() {
    let data = xbe_bytes();
    let x = xbe::Xbe::parse(&data).unwrap();
    for g in &GATES {
        let jne = x.bytes_at(g.site.base, 1).unwrap();
        assert_eq!(jne, [0x75], "{} at {:#010x}", g.site.name, g.site.base);
        let (_, addr) = byte_load(&x, g.resume);
        assert_eq!(addr, ARRAY + u32::from(g.skull), "{}", g.site.name);
    }
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn the_gates_cover_every_reader_of_an_ai_skull() {
    let data = xbe_bytes();
    let x = xbe::Xbe::parse(&data).unwrap();
    let mut skulls: Vec<u8> = GATES.iter().map(|g| g.skull).collect();
    skulls.dedup();
    let mut found = Vec::new();
    // Every section: an absolute scan finds these and nothing else.
    for s in &x.sections {
        let Ok(bytes) = x.section_bytes(s) else { continue };
        for (i, w) in bytes.windows(4).enumerate() {
            let v = u32::from_le_bytes(w.try_into().unwrap());
            if v >= ARRAY && v - ARRAY < 15 && skulls.contains(&((v - ARRAY) as u8)) {
                found.push(s.va + i as u32);
            }
        }
    }
    let mut want: Vec<u32> = GATES.iter().map(|g| byte_load(&x, g.resume).0).collect();
    want.push(FAMINE_PLACED);
    want.sort_unstable();
    found.sort_unstable();
    assert_eq!(found, want);
}
