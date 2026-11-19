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
