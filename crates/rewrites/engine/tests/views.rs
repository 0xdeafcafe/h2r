//! The views on a fake guest. Datum arrays are laid out by the ported datum
//! functions, so the records sit where the game's arrays put them; the tag
//! index is built from its documented layout. Synthetic values only.

use guest_layout::DatumHandle;
use guest_layout::datum::layout;
use guest_layout::memory::GuestMemory;
use guest_layout::points;
use guest_layout::tags::{GroupTag, entry_layout, group_layout, index_layout};
use guest_layout::testing::FakeGuest;
use guest_layout::view::{Tags, fake, objects, players};

const PLAYERS_HEADER: u32 = 0x0010_0000;
const OBJECTS_HEADER: u32 = 0x0020_0000;

#[test]
fn a_record_round_trips_through_its_builder_and_accessors() {
    let mut g = FakeGuest::new();
    let p = players::Player::build()
        .salt(0x8001)
        .local_user(-1)
        .unit(Some(DatumHandle::new(0xe0, 3)))
        .team(-2)
        .write(&mut g, 0x5000);
    assert_eq!(p.salt(&g), Ok(0x8001));
    assert_eq!(p.local_user(&g), Ok(-1));
    assert_eq!(p.unit(&g), Ok(Some(DatumHandle::new(0xe0, 3))));
    assert_eq!(p.team(&g), Ok(-2));
    assert_eq!(p.flags(&g), Ok(0), "unset fields stay as they were");
    p.set_unit(&mut g, None).unwrap();
    assert_eq!(g.u32(0x5000 + players::Player::UNIT), Ok(u32::MAX));
    assert_eq!(
        (players::Player::SIZE, players::Player::TEAM),
        (0x21c, 0xc0)
    );
    assert!(
        players::Player::at(0x9000).salt(&g).is_err(),
        "unmapped faults"
    );
}

#[test]
fn players_are_listed_from_the_global_with_their_names() {
    let mut g = FakeGuest::new();
    g.put_u32(points::PLAYERS.base, PLAYERS_HEADER);
    let d = fake::datums::<players::Player>(&mut g, PLAYERS_HEADER, 16);
    let (h0, p0) = fake::push(&mut g, &d);
    let (_, p1) = fake::push(&mut g, &d);
    let (h2, _) = fake::push(&mut g, &d);
    let name: Vec<u8> = "Chief".encode_utf16().flat_map(u16::to_le_bytes).collect();
    g.put(p0.addr + players::NAME, &name);
    p1.set_flags(&mut g, 2).unwrap(); // leaving
    d.array.delete(&mut g, h2).unwrap();

    let array = players::array(&g).unwrap().expect("published");
    let live = array.live(&g, 16).unwrap();
    assert_eq!(live.len(), 2, "the deleted slot is not live");
    assert_eq!(live[0].0, h0);
    assert_eq!(live[0].1.name(&g).unwrap(), "Chief");
    assert!(live[0].1.listed(&g).unwrap());
    assert!(!live[1].1.listed(&g).unwrap());
    assert_eq!(array.get(&g, h0).unwrap(), Some(p0));
    assert_eq!(array.get(&g, h2).unwrap(), None, "a stale handle");
    assert_eq!(array.get(&g, DatumHandle::NONE).unwrap(), None);
    assert_eq!(array.live(&g, 1).unwrap().len(), 1, "the limit");
    // The element size the array was built with is the record's.
    assert_eq!(d.array.header_field(&g, layout::ELEMENT_SIZE), Ok(0x21c));
}

#[test]
fn no_published_array_reads_as_none() {
    let mut g = FakeGuest::new();
    g.put_u32(points::PLAYERS.base, 0)
        .put_u32(points::OBJECTS.base, 0);
    assert_eq!(players::array(&g).unwrap(), None);
    assert_eq!(objects::get(&g, DatumHandle::new(1, 0)).unwrap(), None);
}

#[test]
fn an_object_is_found_by_handle_with_its_type_and_body() {
    let mut g = FakeGuest::new();
    g.put_u32(points::OBJECTS.base, OBJECTS_HEADER);
    let d = fake::datums::<objects::Header>(&mut g, OBJECTS_HEADER, 8);
    let (h, header) = fake::push(&mut g, &d);
    let (bodiless, _) = fake::push(&mut g, &d);
    let body = objects::Body::build()
        .definition(Some(DatumHandle::new(0xe100, 7)))
        .position([1.0, 2.0, 3.0])
        .damage_flags(1 << 2)
        .write(&mut g, 0x0030_0000);
    header.set_kind(&mut g, objects::VEHICLE).unwrap();
    header.set_body(&mut g, body.addr).unwrap();

    let (kind, found) = objects::get(&g, h).unwrap().expect("live");
    assert_eq!((kind, found), (objects::VEHICLE, body));
    assert_eq!(found.position(&g), Ok([1.0, 2.0, 3.0]));
    assert_eq!(found.definition(&g), Ok(Some(DatumHandle::new(0xe100, 7))));
    assert_eq!(
        found.parent(&g),
        Ok(Some(DatumHandle(0))),
        "fresh pages are zero"
    );
    assert_eq!(objects::get(&g, bodiless).unwrap(), None, "no body");
    let stale = DatumHandle::new(h.salt().wrapping_add(1), h.index() as u32);
    assert_eq!(objects::get(&g, stale).unwrap(), None, "wrong salt");
}

/// A tag index with one group (`'bipd'`, parent `'unit'`) and two tags.
fn tag_index(g: &mut FakeGuest) -> (DatumHandle, DatumHandle) {
    const INDEX: u32 = 0x0040_0000;
    const GROUPS: u32 = 0x0040_1000;
    const ENTRIES: u32 = 0x0040_2000;
    let bipd = GroupTag::from_chars(*b"bipd");
    let unit = GroupTag::from_chars(*b"unit");
    g.put_u32(points::TAG_INDEX.base, INDEX)
        .put_u32(points::TAG_ENTRIES.base, ENTRIES);
    g.put_u32(INDEX + index_layout::GROUPS, GROUPS)
        .put_u32(INDEX + index_layout::GROUP_COUNT, 1)
        .put_u32(INDEX + index_layout::TAG_COUNT, 2);
    g.put_u32(GROUPS + group_layout::GROUP, bipd.0)
        .put_u32(GROUPS + group_layout::PARENT, unit.0)
        .put_u32(GROUPS + group_layout::GRANDPARENT, u32::MAX);
    let (a, b) = (DatumHandle::new(0xe000, 0), DatumHandle::new(0xe001, 1));
    for (i, h) in [a, b].into_iter().enumerate() {
        let e = ENTRIES + i as u32 * entry_layout::STRIDE;
        g.put_u32(e + entry_layout::GROUP, bipd.0)
            .put_u32(e + entry_layout::DATUM, h.raw())
            .put_u32(e + entry_layout::ADDRESS, 0x0050_0000 + 0x100 * i as u32)
            .put_u32(e + entry_layout::SIZE, 0x40);
    }
    g.put_u32(INDEX + index_layout::SCENARIO, b.raw());
    (a, b)
}

#[test]
fn tags_resolve_by_handle_and_group() {
    let mut g = FakeGuest::new();
    let (a, b) = tag_index(&mut g);
    let tags = Tags;
    let entry = tags.entry(&g, b).unwrap().expect("in range");
    assert_eq!(entry.group(&g), Ok(GroupTag::from_chars(*b"bipd")));
    assert_eq!(entry.datum(&g), Ok(Some(b)));
    assert_eq!(entry.address(&g), Ok(0x0050_0100));
    assert_eq!(entry.size(&g), Ok(0x40));
    let unit = GroupTag::from_chars(*b"unit");
    assert_eq!(
        tags.get(&g, a, unit),
        Ok(Some(0x0050_0000)),
        "a parent group"
    );
    assert_eq!(tags.get(&g, a, GroupTag::from_chars(*b"vehi")), Ok(None));
    assert_eq!(
        tags.entry(&g, DatumHandle::new(0xe000, 5)),
        Ok(None),
        "out of range"
    );
    assert_eq!(
        tags.entry(&g, DatumHandle::new(0x1234, 0)),
        Ok(None),
        "stale"
    );
    assert_eq!(tags.scenario(&g), Ok(Some(b)));
}

#[test]
fn a_controls_shaped_screen_reaches_its_list_items_press_record_and_slots() {
    use guest_layout::view::ui::{self, ListHandler, PressRecord, Screen};
    const SCREEN: u32 = 0x0060_0000;
    const VTABLE: u32 = 0x0061_0000;
    let mut g = FakeGuest::new();
    g.map(SCREEN, ui::CONTROLS_SCREEN_SIZE);
    let screen = Screen::at(SCREEN);
    screen.set_id(&mut g, 39).unwrap();
    let list = screen.list();
    assert_eq!(list, ListHandler::at(SCREEN + 0x614));
    assert_eq!(screen.header(), SCREEN + 0x80);
    // The rows: a datum array built by the ported allocator, as the
    // constructor builds it.
    let d = fake::datums::<ui::Row>(&mut g, 0x0070_0000, 6);
    let rows: Vec<_> = (0..6).map(|_| fake::push(&mut g, &d).0).collect();
    list.set_rows(&mut g, d.array.addr).unwrap();
    // The third item shows row 2.
    let item = list.item(2);
    assert_eq!(item.addr, SCREEN + 0x614 + 0x88 + 2 * 0x80);
    item.set_parent(&mut g, list.addr).unwrap();
    item.set_row(&mut g, Some(rows[2])).unwrap();
    assert_eq!(item.row_index(&g), Ok(Some(2)));
    assert_eq!(item.parent(&g), Ok(list.addr));
    let array = list.row_array(&g).unwrap();
    assert_eq!(array.live(&g, 16).unwrap().len(), 6);
    // The press record calls its function on its target.
    let press = list.press(ui::CONTROLS_PRESS);
    assert_eq!(press, PressRecord::at(SCREEN + 0x614 + 0x388));
    press.set_target(&mut g, list.addr).unwrap();
    press.set_function(&mut g, 0x002c_8719).unwrap();
    assert_eq!(g.u32(SCREEN + 0x614 + 0x388 + 0x14), Ok(0x002c_8719));
    // Slots are read from the vtable the handler points at.
    g.map(VTABLE, 4 * ui::list_slot::COUNT);
    g.put_u32(VTABLE + 4 * ui::list_slot::ROW_TEXT, 0x1234);
    list.set_vtable(&mut g, VTABLE).unwrap();
    let vt = list.vtable(&g).unwrap();
    assert_eq!(ui::slot(&g, vt, ui::list_slot::ROW_TEXT), Ok(0x1234));
    assert_eq!(screen.id(&g), Ok(39));
}
