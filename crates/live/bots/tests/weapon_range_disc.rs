//! Feed retail and converted weapon metadata to the same reader used in play.
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::PathBuf;

use bots::arms::{self, Profile, Role, WeaponDef};
use guest_layout::{
    memory::GuestMemory,
    points,
    tags::{entry_layout, index_layout},
    testing::FakeGuest,
};
use h1map::{Block, Group, H1Map, TagRef};
use map::{FourCC, Map, TagId, TagQuery};

const H1_SHOTGUN: &str = r"weapons\shotgun\shotgun";
const H1_AR: &str = r"weapons\assault rifle\assault rifle";
const H2_SHOTGUN: &str = r"objects\weapons\rifle\shotgun\shotgun";

fn h1_range(map: &H1Map, name: &str) -> f32 {
    let weapon = map.find(Group::new(b"weap"), name).unwrap();
    let weapon = map.tag_data(weapon).unwrap();
    let triggers = Block::read(weapon, 0x4fc).unwrap();
    assert!(triggers.count > 0);
    let trigger = map.read(triggers.address, 0xa4).unwrap();
    let projectile = map.tag(TagRef::read(trigger, 0x94).unwrap().id).unwrap();
    assert_eq!(projectile.group, Group::new(b"proj"));
    let end = map.read(projectile.data_address + 0x1d4, 4).unwrap();
    f32::from_le_bytes(end.try_into().unwrap())
}

fn profile<R: Read + Seek>(map: &mut Map<R>, weapon: &str) -> Profile {
    const INDEX: u32 = 0x0060_0000;
    const ENTRIES: u32 = 0x0060_1000;
    let weapon = map
        .tag(TagQuery::NameInGroup(weapon, FourCC(*b"weap")))
        .unwrap()
        .location
        .unwrap()
        .address;
    let space = map.metadata_space().unwrap();
    let bytes = map
        .read_file_range(space.file_offset, space.size.into())
        .unwrap();
    let mut guest = FakeGuest::new();
    guest
        .put(space.virtual_base, &bytes)
        .put_u32(points::TAG_INDEX.base, INDEX)
        .put_u32(points::TAG_ENTRIES.base, ENTRIES)
        .put_u32(INDEX + index_layout::TAG_COUNT, map.index_slots());

    let barrel = guest.u32(weapon + arms::WEAP_BARRELS + 4).unwrap();
    let id = TagId(guest.u32(barrel + arms::BARREL_DEF_PROJECTILE + 4).unwrap());
    let projectile = map.tag(id).unwrap();
    assert_eq!(projectile.group, FourCC(*b"proj"));
    let slot = ENTRIES + id.index() as u32 * entry_layout::STRIDE;
    guest
        .put_u32(slot + entry_layout::GROUP, u32::from_be_bytes(*b"proj"))
        .put_u32(slot + entry_layout::DATUM, id.0)
        .put_u32(
            slot + entry_layout::ADDRESS,
            projectile.location.unwrap().address,
        );
    WeaponDef::profile(&guest, weapon).unwrap()
}

#[test]
#[ignore = "reads H1_ISO, H2_DISC and both installed Blood Gulch sandboxes under H2_DATA"]
fn bots_use_the_loaded_shotguns_damage_range_in_both_sandboxes() {
    let iso = |var| xiso::Xiso::open(File::open(std::env::var_os(var).unwrap()).unwrap()).unwrap();
    let mut h1 = iso("H1_ISO");
    let bloodgulch = h1.find("maps/bloodgulch.map").unwrap();
    let source = H1Map::from_bytes(&h1.read(&bloodgulch).unwrap()).unwrap();
    let mut h2 = iso("H2_DISC");
    let coagulation = h2.find("maps/coagulation.map").unwrap();
    let mut retail = Map::from_reader(Cursor::new(h2.read(&coagulation).unwrap())).unwrap();
    let retail = profile(&mut retail, H2_SHOTGUN);
    assert_eq!(retail.role(), Role::Scatter);
    assert_eq!(retail.damage_reach, 8.0);
    assert_eq!(bots::weapons::reach(&retail).fire.1, 8.0);

    let data = PathBuf::from(std::env::var_os("H2_DATA").expect("set H2_DATA"));
    let converted = |sandbox| {
        Map::open(data.join(format!("maps/h1_bloodgulch/{sandbox}/h1_bloodgulch.map"))).unwrap()
    };
    let h2_sandbox = profile(&mut converted("h2"), H2_SHOTGUN);
    assert_eq!(h2_sandbox, retail);

    let mut h1_sandbox = converted("h1");
    let shotgun = profile(&mut h1_sandbox, H1_SHOTGUN);
    assert_eq!(shotgun.role(), Role::Scatter);
    assert_eq!(shotgun.damage_reach, h1_range(&source, H1_SHOTGUN));
    assert_eq!(bots::weapons::reach(&shotgun).fire.1, 3.0);
    assert!(shotgun.cone_range > shotgun.damage_reach);

    let ar = profile(&mut h1_sandbox, H1_AR);
    assert_eq!(ar.damage_reach, h1_range(&source, H1_AR));
    assert_eq!(bots::weapons::reach(&ar).fire.1, 15.0);
}
