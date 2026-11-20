//! Checks what the crate assumes about `D3DSIMPLERENDERSTATEENCODE` against
//! the real retail XBE. Ignored by default (the image is not in the
//! repository, and neither is the table). Run with the path to a
//! `default.xbe`, read-only:
//!
//! `H2_XBE=/abs/path/default.xbe cargo test -p d3d --test it xbe_tables:: -- --ignored`

mod common;

use d3d::GuestLayout;
use d3d::pushbuf::{SIMPLE_ENCODE_SLOTS, SimpleEncode};
use d3d::runtime_glue::GuestAdapter;
use runtime::load_xbe;

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn simple_encode_table_matches_the_retail_image() {
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    let image = load_xbe(&std::fs::read(path).unwrap()).unwrap();
    let mut guest = image.runtime.memory();
    let mem = GuestAdapter::new(&mut guest);
    let at = GuestLayout::HALO2.simple_encode;

    // The device's own read and validation accept the image's table.
    let table = SimpleEncode::read(&mem, at).expect("valid table in the image");

    // Word for word: every slot is a one-word header whose method the
    // table reports back, and the slots the synthetic test table names
    // agree with the image.
    for slot in 0..SIMPLE_ENCODE_SLOTS {
        let word = d3d::GuestMemory::read_u32(&mem, at + 4 * slot as u32).unwrap();
        assert_eq!(word >> 18, 1, "slot {slot}: {word:#x}");
        assert_eq!(table.method(slot), Some(word & 0x3_FFFF), "slot {slot}");
    }
    for &(slot, m) in common::ENCODE_SLOTS {
        assert_eq!(table.method(slot), Some(m), "synthetic slot {slot}");
        assert_eq!(table.slot(m), Some(slot as u32), "method {m:#x}");
    }
    // The simple states (57..83) are invertible; 84..91 and slot 54 share
    // the placeholder 0x1D90, which is not.
    for slot in 57..84 {
        let m = table.method(slot).unwrap();
        assert_eq!(table.slot(m), Some(slot as u32), "slot {slot} ({m:#x})");
    }
    for slot in (84..SIMPLE_ENCODE_SLOTS).chain([54]) {
        assert_eq!(table.method(slot), Some(0x1D90), "slot {slot}");
    }
    assert_eq!(table.slot(0x1D90), None);
    // 0x030C is not in the table: the device maps it to slot 143 itself.
    assert_eq!(table.slot(0x030C), None);

    // The inlined generic setter (0x1828e) indexes this address.
    let mut code = [0u8; 0x20];
    d3d::GuestMemory::read(&mem, 0x1828e, &mut code).unwrap();
    assert!(
        code.windows(4).any(|w| w == at.to_le_bytes()),
        "0x1828e does not reference {at:#x}"
    );
}
