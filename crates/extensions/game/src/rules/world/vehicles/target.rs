//! The vehicle a script's `object` operand names, as a live salted datum.
//! Census handles occupy bit 30 with bit 31 clear. Retail datum salts
//! have bit 31 set and can also set bit 30 (docs/engine/datum_new.md).

use guest_layout::GuestMemory;
use guest_layout::handle::DatumHandle;
use guest_layout::view::objects::{self, Header};
use forge_doc::BlockKind;
use guest_layout::{Object, ObjectHeader, object::header::DELETE};

use super::super::super::census::Table;

fn vehicle_body(
    m: &dyn GuestMemory,
    header: Header,
) -> Result<Option<Object>, guest_layout::Fault> {
    let h = ObjectHeader::at(header.addr);
    if h.kind(m)? != objects::VEHICLE || h.flags(m)? & DELETE != 0 {
        return Ok(None);
    }
    let body = h.body(m)?;
    Ok((body != 0).then(|| Object::at(body)))
}

pub(super) fn is_vehicle(m: &dyn GuestMemory, datum: u32) -> bool {
    if datum & 0x8000_0000 == 0 || datum == u32::MAX {
        return false;
    }
    let Ok(Some(array)) = objects::array(m) else {
        return false;
    };
    let Ok(Some(header)) = array.get(m, DatumHandle(datum)) else {
        return false;
    };
    matches!(vehicle_body(m, header), Ok(Some(_)))
}

/// Pin the one live vehicle belonging to this scenario placement. A later
/// spawn into the same placement cannot receive work for the original.
pub(super) fn resolve(m: &dyn GuestMemory, table: Option<&Table>, operand: u32) -> Option<u32> {
    if operand & 0x8000_0000 != 0 {
        return is_vehicle(m, operand).then_some(operand);
    }
    let thing = table?.get(operand)?;
    if thing.block != BlockKind::Vehicles {
        return None;
    }
    let index = i16::try_from(thing.index).ok()?;
    super::super::objects::placed_datum(m, objects::VEHICLE, index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::census::{HANDLE, Thing};
    use ::rules::labels::Class;
    use guest_layout::testing::FakeGuest;
    use guest_layout::view::objects::Body;
    use guest_layout::view::{Datums, fake};

    struct World(FakeGuest, Datums<Header>, u32);

    impl World {
        fn new() -> World {
            let mut g = FakeGuest::new();
            let pool = fake::datums::<Header>(&mut g, 0x2000_0100, 16);
            g.map(guest_layout::points::OBJECTS.base, 4)
                .put_u32(guest_layout::points::OBJECTS.base, 0x2000_0100);
            World(g, pool, 0x3000_0000)
        }

        fn header(&self, h: u32) -> Header {
            self.1.get(&self.0, DatumHandle(h)).unwrap().unwrap()
        }

        fn spawn(&mut self, kind: u8, placement: i16) -> u32 {
            let (h, header) = fake::push(&mut self.0, &self.1);
            let body = self.2;
            self.2 += 0x1000;
            self.0.map(body, 0x400);
            header.set_kind(&mut self.0, kind).unwrap();
            header.set_body(&mut self.0, body).unwrap();
            Body::at(body).set_parent(&mut self.0, None).unwrap();
            self.0
                .put_u16(body + Object::PLACEMENT_INDEX.offset, placement as u16);
            h.raw()
        }
    }

    fn table() -> Table {
        let at = |block, index| Thing {
            text: "ship".into(),
            label: None,
            number: None,
            class: Class::Object,
            designator: None,
            block,
            index,
            area: None,
        };
        Table::from_things(vec![at(BlockKind::Vehicles, 2), at(BlockKind::Scenery, 2)])
    }

    #[test]
    fn a_raw_datum_and_a_placement_resolve_to_the_same_vehicle() {
        let mut w = World::new();
        w.spawn(objects::VEHICLE, 1);
        let v = w.spawn(objects::VEHICLE, 2);
        let (t, g) = (table(), &w.0);
        let (placed, scenery) = (t.things()[0].handle(), t.things()[1].handle());
        assert!(v & 0x8000_0000 != 0 && placed & HANDLE != 0 && placed & 0x8000_0000 == 0);
        assert_eq!(resolve(g, Some(&t), v), Some(v));
        assert_eq!(resolve(g, Some(&t), placed), Some(v));
        assert_eq!(resolve(g, None, placed), None);
        assert_eq!(resolve(g, Some(&t), scenery), None);
    }

    #[test]
    fn a_raw_datum_with_bit_30_is_a_datum_not_a_placement() {
        let mut w = World::new();
        let v = w.spawn(objects::VEHICLE, 2);
        w.header(v).set_salt(&mut w.0, 0xc001).unwrap();
        let raw = 0xc001_0000 | (v & 0xffff);
        assert!(raw & HANDLE != 0);
        assert_eq!(resolve(&w.0, Some(&table()), raw), Some(raw));
    }

    #[test]
    fn two_vehicles_of_one_placement_match_nothing() {
        let mut w = World::new();
        w.spawn(objects::VEHICLE, 2);
        let t = table();
        let placed = t.things()[0].handle();
        assert!(resolve(&w.0, Some(&t), placed).is_some());
        w.spawn(objects::VEHICLE, 2);
        assert_eq!(resolve(&w.0, Some(&t), placed), None);
    }

    #[test]
    fn stale_wrong_type_and_deleting_vehicles_are_refused() {
        let mut w = World::new();
        let (biped, gone, dying) = (
            w.spawn(objects::BIPED, 2),
            w.spawn(objects::VEHICLE, 2),
            w.spawn(objects::VEHICLE, 2),
        );
        w.1.array.delete(&mut w.0, DatumHandle(gone)).unwrap();
        let flags = w.header(dying).addr + ObjectHeader::FLAGS.offset;
        w.0.put_u8(flags, DELETE);
        for datum in [biped, gone, dying, u32::MAX, 0] {
            assert!(!is_vehicle(&w.0, datum), "{datum:#x}");
            assert_eq!(resolve(&w.0, Some(&table()), datum), None);
        }
        let (t, placed) = (table(), table().things()[0].handle());
        assert_eq!(resolve(&w.0, Some(&t), placed), None);
        let new = w.spawn(objects::VEHICLE, 2);
        assert_eq!(resolve(&w.0, Some(&t), placed), Some(new));
    }
}
