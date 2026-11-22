use super::*;
use crate::map_select::{Catalogue, Map};

#[test]
fn a_refresh_releases_only_an_obsolete_generated_slot() {
    let slot = |at, id: u32, path: &str| {
        let mut bytes = vec![0; SLOT_LEN];
        bytes[..4].copy_from_slice(&id.to_le_bytes());
        bytes[MP_PATH..MP_PATH + path.len()].copy_from_slice(path.as_bytes());
        Slot { at, bytes }
    };
    let current_path = "t:\\$C\\4d53006480020002\\another-mod";
    let current = Catalogue::new([Map {
        id: 0x20002,
        path: current_path.into(),
        ..Map::default()
    }]);
    let slots = [
        slot(1, 10, "scenarios\\multi\\lockout\\lockout"),
        slot(2, 101, "t:\\$C\\4d53006400000101\\turf"),
        slot(3, 0x20001, "t:\\$C\\4d53006480020001\\old-mod"),
        slot(4, 0x20002, current_path),
        slot(5, FREE, ""),
    ];
    assert_eq!(obsolete(&slots, &current), [3]);
}

#[test]
fn selected_path_reads_a_used_slot_without_the_catalogue() {
    use runtime::{Guest, guest::Permissions};

    let mut guest = Guest::new();
    let header = 0x0100_0000;
    let matg = header + 0x1000;
    let ui = matg + 0x1000;
    let slots = ui + 0x1000;
    guest
        .map_zeroed(GLOBALS_TAG & !0xfff, 0x1000, Permissions::RW)
        .unwrap();
    guest.map_zeroed(header, 0x5000, Permissions::RW).unwrap();
    guest.set_u32(GLOBALS_TAG, matg).unwrap();
    guest.set_u32(CACHE_HEADER, header).unwrap();
    guest.set_u16(header + CACHE_TYPE, MAIN_MENU).unwrap();
    guest.set_u32(matg + MATG_UI, 1).unwrap();
    guest.set_u32(matg + MATG_UI + 4, ui).unwrap();
    guest.set_u32(ui + UI_MP, 2).unwrap();
    guest.set_u32(ui + UI_MP + 4, slots).unwrap();
    guest.set_u32(slots, FREE).unwrap();
    let slot = slots + SLOT_LEN as u32;
    guest.set_u32(slot, 12345).unwrap();
    let path = r"scenarios\multi\test_map\test_map";
    guest
        .write_bytes(slot + MP_PATH as u32, path.as_bytes(), 1)
        .unwrap();

    assert_eq!(selected_path(&guest, 12345).as_deref(), Some(path));
    assert_eq!(selected_path(&guest, FREE), None);
    assert_eq!(selected_path(&guest, 12344), None);
    guest.set_u16(header + CACHE_TYPE, 1).unwrap();
    assert_eq!(selected_path(&guest, 12345), None);
}
