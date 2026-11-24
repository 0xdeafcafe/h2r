//! The vehicle a script's `object` operand names, resolved to a live
//! vehicle datum (docs/game/rules-world-api.md, "Vehicles and drop
//! ships": a vehicle is an `object` the game checks at the call).
//!
//! An operand is one `u32` and is one of two things, told apart by its
//! top bits ([`classify`]):
//!
//! - A **raw datum handle**: `(salt << 16) | index`. A datum array's salt
//!   has bit 15 set (`next_salt` is seeded `name[0..2] | 0x8000`,
//!   docs/engine/datum_new.md), so a raw handle has bit 31 set.
//! - A **census placement handle**: [`HANDLE`] (bit 30) with the placement
//!   id (`block << 16 | index`, `census::Thing::id`). A census handle is
//!   below `2^31`: the block number is small, so bit 31 is never set.
//!
//! The encodings overlap in bit 30: a raw handle with salt `0xc000` or
//! above has it set too. Bit 31 is therefore tested first, and an operand
//! with bit 31 clear and bit 30 set is a placement. A value with neither
//! bit (0, a small integer) is neither and is refused, as is the none
//! handle `0xffffffff`.
//!
//! A placement names a scenario vehicle, which the engine spawns at map
//! load: the live vehicle is the one object that is a vehicle by its
//! header's type, whose body's placement index (`Object::PLACEMENT_INDEX`,
//! copied by `object_new` from the placement) is the placement's index in
//! the vehicles block, and whose definition tag is the tag of the
//! placement's palette entry. All three must hold, and exactly one object
//! must hold them: none is [`Refusal::NotSpawned`] and several is
//! [`Refusal::Ambiguous`]. Nothing is picked by position or by being the
//! nearest, so a host never acts on a guess.
//!
//! A call that acts later pins the salted datum [`resolve`] answers and
//! asks [`is_vehicle`] again when the host executes: a vehicle that died
//! and whose slot was reused fails on the salt, and an object that is no
//! longer a vehicle fails on its type. A placement is resolved once, at
//! the call, and never re-pointed at a later spawn of the same placement.
//! `is_vehicle` does not compare the definition tag: a pinned live
//! vehicle datum keeps its definition for as long as it is live.

use std::fmt;

use engine::objectives::check_span;
use engine::view::objects;
use engine::{GuestMemory, MemFault, points};
use forge_doc::BlockKind;
use guest_layout::Object;
use guest_layout::handle::DatumHandle;

use super::super::super::census::{HANDLE, Table};

/// "No tag" in a palette entry's datum.
const NULL_DATUM: u32 = u32::MAX;
/// A palette entry's tag datum (`+4`, after the group; the editor reads the
/// same field, `forge_map::rows`).
const PALETTE_TAG: u32 = 4;
/// A placement's palette index (`i16`, `scenario_layout`'s `OBJECT` head).
const PLACEMENT_PALETTE: u32 = 0;
/// How many objects the walk looks at: the objects array's capacity bound.
const OBJECT_LIMIT: i32 = 0x8000;

/// What an operand is, before the guest is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operand {
    /// A raw datum handle.
    Datum(DatumHandle),
    /// A census placement: the id `block << 16 | index`.
    Placement(u32),
}

/// Why an operand names no usable vehicle. A refusal changes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Neither a raw datum nor a census handle, or the none handle.
    NotAHandle(u32),
    /// The census names a thing that is not in the vehicles block.
    NotAVehiclePlacement(u32),
    /// The census names a placement the scenario does not hold, or one
    /// without a palette tag.
    NoSuchPlacement(u32),
    /// No live object is this datum: never made, deleted, or its slot
    /// reused under another salt.
    Stale(DatumHandle),
    /// The object is live but its type is not a vehicle's.
    WrongType { handle: DatumHandle, kind: u8 },
    /// The placement has no live vehicle (not yet spawned, destroyed).
    NotSpawned(u32),
    /// More than one live vehicle claims the placement; none is guessed.
    Ambiguous(u32),
    /// A guest read faulted.
    Fault(u32),
}

impl From<MemFault> for Refusal {
    fn from(e: MemFault) -> Self {
        Refusal::Fault(e.addr)
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Refusal::NotAHandle(v) => write!(f, "{v:#010x} is no object handle"),
            Refusal::NotAVehiclePlacement(id) => {
                write!(f, "placement {id:#x} is not in the vehicles block")
            }
            Refusal::NoSuchPlacement(id) => {
                write!(f, "placement {id:#x} is not a vehicle the scenario holds")
            }
            Refusal::Stale(h) => write!(f, "{:#010x} is no live object", h.raw()),
            Refusal::WrongType { handle, kind } => {
                write!(
                    f,
                    "{:#010x} is an object of type {kind}, not a vehicle",
                    handle.raw()
                )
            }
            Refusal::NotSpawned(id) => write!(f, "placement {id:#x} has no live vehicle"),
            Refusal::Ambiguous(id) => {
                write!(f, "placement {id:#x} has more than one live vehicle")
            }
            Refusal::Fault(a) => write!(f, "guest memory at {a:#x} faulted"),
        }
    }
}

impl std::error::Error for Refusal {}

/// Which kind of operand `raw` is. Bit 31 first: a raw handle whose salt
/// has bit 14 set also has [`HANDLE`] set.
pub fn classify(raw: u32) -> Result<Operand, Refusal> {
    match DatumHandle::from_raw(raw) {
        None => Err(Refusal::NotAHandle(raw)),
        Some(h) if raw & 0x8000_0000 != 0 => Ok(Operand::Datum(h)),
        Some(_) if raw & HANDLE != 0 => Ok(Operand::Placement(raw & !HANDLE)),
        Some(_) => Err(Refusal::NotAHandle(raw)),
    }
}

/// A live vehicle: its datum and its definition tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vehicle {
    pub handle: DatumHandle,
    /// The definition tag's datum, as the object body holds it.
    pub definition: DatumHandle,
}

/// The vehicle datum `handle` names, if it is live and a vehicle.
fn vehicle_of(m: &dyn GuestMemory, handle: DatumHandle) -> Result<Vehicle, Refusal> {
    let Some((kind, body)) = objects::get(m, handle)? else {
        return Err(Refusal::Stale(handle));
    };
    if kind != objects::VEHICLE {
        return Err(Refusal::WrongType { handle, kind });
    }
    let definition = body.definition(m)?.ok_or(Refusal::Stale(handle))?;
    Ok(Vehicle { handle, definition })
}

/// The definition tag of vehicles-block placement `index`: its palette
/// entry's tag.
fn placement_tag(m: &dyn GuestMemory, id: u32, index: u32) -> Result<DatumHandle, Refusal> {
    let none = Refusal::NoSuchPlacement(id);
    let scnr = m.u32(points::SCENARIO.base)?;
    if scnr == 0 {
        return Err(none);
    }
    let block = map::scenario_layout::block(BlockKind::Vehicles);
    let palette = block.palette.as_ref().ok_or(none)?;
    check_span(scnr, block.offset.max(palette.offset) + 8)?;
    let count = m.i32(scnr + block.offset)?;
    let rows = m.u32(scnr + block.offset + 4)?;
    if i64::from(index) >= i64::from(count) {
        return Err(none);
    }
    check_span(rows, (index + 1) * block.stride)?;
    let slot = i32::from(m.u16(rows + index * block.stride + PLACEMENT_PALETTE)? as i16);
    let pcount = m.i32(scnr + palette.offset)?;
    let pbase = m.u32(scnr + palette.offset + 4)?;
    if slot < 0 || slot >= pcount {
        return Err(none);
    }
    let at = pbase
        .checked_add(slot as u32 * palette.stride + PALETTE_TAG)
        .ok_or(none)?;
    let datum = m.u32(at)?;
    if datum == NULL_DATUM {
        return Err(none);
    }
    Ok(DatumHandle(datum))
}

/// The one live vehicle spawned from vehicles-block placement `index`.
fn spawned(m: &dyn GuestMemory, id: u32, index: u32) -> Result<Vehicle, Refusal> {
    let tag = placement_tag(m, id, index)?;
    let Some(array) = objects::array(m)? else {
        return Err(Refusal::NotSpawned(id));
    };
    let mut found: Option<Vehicle> = None;
    for row in array.live_iter(m, OBJECT_LIMIT)? {
        let (handle, header) = row?;
        if header.kind(m)? != objects::VEHICLE {
            continue;
        }
        let body = header.body(m)?;
        if body == 0 {
            continue;
        }
        let placed = m.u16(body + Object::PLACEMENT_INDEX.offset)? as i16;
        if i64::from(placed) != i64::from(index) {
            continue;
        }
        if objects::Body::at(body).definition(m)? != Some(tag) {
            continue;
        }
        if found.is_some() {
            return Err(Refusal::Ambiguous(id));
        }
        found = Some(Vehicle {
            handle,
            definition: tag,
        });
    }
    found.ok_or(Refusal::NotSpawned(id))
}

/// Resolves a census placement id against `table` and the scenario.
fn placement(m: &dyn GuestMemory, table: &Table, id: u32) -> Result<Vehicle, Refusal> {
    let thing = table.get(HANDLE | id).ok_or(Refusal::NoSuchPlacement(id))?;
    if thing.block != BlockKind::Vehicles {
        return Err(Refusal::NotAVehiclePlacement(id));
    }
    spawned(m, id, thing.index)
}

/// The live vehicle `raw` names now, or why it names none.
pub fn explain(m: &dyn GuestMemory, table: &Table, raw: u32) -> Result<Vehicle, Refusal> {
    match classify(raw)? {
        Operand::Datum(h) => vehicle_of(m, h),
        Operand::Placement(id) => placement(m, table, id),
    }
}

/// The salted datum of the live vehicle `operand` names: a raw datum
/// handle, or a census placement handle when `table` is given. `None`
/// for anything stale, of another type, unplaced or ambiguous
/// ([`explain`] says which).
pub(super) fn resolve(m: &dyn GuestMemory, table: Option<&Table>, operand: u32) -> Option<u32> {
    let empty = Table::default();
    let table = table.unwrap_or(&empty);
    explain(m, table, operand).ok().map(|v| v.handle.raw())
}

/// Whether `datum` is a live object whose type is a vehicle's: the check
/// a dispatch repeats on a pinned datum.
pub(super) fn is_vehicle(m: &dyn GuestMemory, datum: u32) -> bool {
    DatumHandle::from_raw(datum).is_some_and(|h| vehicle_of(m, h).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::census::Thing;
    use ::rules::labels::Class;
    use engine::testing::FakeGuest;
    use engine::view::{Datums, fake};

    const SCNR: u32 = 0x1000_0000;
    const ROWS: u32 = 0x1001_0000;
    const PALETTE: u32 = 0x1002_0000;
    const OBJECTS: u32 = 0x2000_0100;
    const BODIES: u32 = 0x3000_0000;
    const TAG_A: u32 = 0xe123_0010;
    const TAG_B: u32 = 0xe123_0011;

    struct World {
        g: FakeGuest,
        objects: Datums<objects::Header>,
        next_body: u32,
    }

    impl World {
        /// Two palette entries (tags A and B) and vehicle placements 0
        /// (palette A) and 1 (palette B).
        fn new() -> World {
            let mut g = FakeGuest::new();
            let block = map::scenario_layout::block(BlockKind::Vehicles);
            let pal = block.palette.as_ref().unwrap();
            g.map(points::SCENARIO.base, 4).map(points::OBJECTS.base, 4);
            g.map(SCNR, 0x400).map(ROWS, 0x400).map(PALETTE, 0x400);
            g.put_u32(points::SCENARIO.base, SCNR);
            g.put_u32(SCNR + block.offset, 2)
                .put_u32(SCNR + block.offset + 4, ROWS);
            g.put_u32(SCNR + pal.offset, 2)
                .put_u32(SCNR + pal.offset + 4, PALETTE);
            g.put_u16(ROWS, 0).put_u16(ROWS + block.stride, 1);
            g.put_u32(PALETTE + PALETTE_TAG, TAG_A);
            g.put_u32(PALETTE + pal.stride + PALETTE_TAG, TAG_B);
            let objects = fake::datums::<objects::Header>(&mut g, OBJECTS, 16);
            g.put_u32(points::OBJECTS.base, OBJECTS);
            g.map(BODIES, 0x2000);
            World {
                g,
                objects,
                next_body: BODIES,
            }
        }

        fn spawn(&mut self, kind: u8, placement: i16, tag: u32) -> DatumHandle {
            let (h, header) = fake::push(&mut self.g, &self.objects);
            let body = self.next_body;
            self.next_body += 0x200;
            header.set_kind(&mut self.g, kind).unwrap();
            header.set_body(&mut self.g, body).unwrap();
            self.g.put_u32(body + Object::DEFINITION.offset, tag);
            self.g
                .put_u16(body + Object::PLACEMENT_INDEX.offset, placement as u16);
            h
        }

        fn delete(&mut self, h: DatumHandle) {
            self.objects.array.delete(&mut self.g, h).unwrap();
        }
    }

    fn thing(block: BlockKind, index: u32) -> Thing {
        Thing {
            text: "ship".into(),
            label: None,
            number: None,
            class: Class::Object,
            designator: None,
            block,
            index,
            area: None,
        }
    }

    fn table() -> Table {
        Table::from_things(vec![
            thing(BlockKind::Vehicles, 0),
            thing(BlockKind::Vehicles, 1),
            thing(BlockKind::Vehicles, 5),
            thing(BlockKind::Scenery, 0),
        ])
    }

    fn handle_of(t: &Table, i: usize) -> u32 {
        t.things()[i].handle()
    }

    #[test]
    fn classify_tells_the_encodings_apart() {
        let census = HANDLE | (3 << 16) | 7;
        assert_eq!(classify(census), Ok(Operand::Placement((3 << 16) | 7)));
        // A raw handle whose salt also has bit 14 set is still a datum.
        assert_eq!(
            classify(0xc001_0002),
            Ok(Operand::Datum(DatumHandle(0xc001_0002)))
        );
        assert_eq!(
            classify(0x8001_0002),
            Ok(Operand::Datum(DatumHandle(0x8001_0002)))
        );
        assert_eq!(classify(0), Err(Refusal::NotAHandle(0)));
        assert_eq!(classify(12), Err(Refusal::NotAHandle(12)));
        assert_eq!(classify(u32::MAX), Err(Refusal::NotAHandle(u32::MAX)));
    }

    #[test]
    fn a_census_handle_never_has_bit_31() {
        let t = table();
        for i in 0..t.len() {
            assert_eq!(handle_of(&t, i) & 0x8000_0000, 0);
            assert!(matches!(
                classify(handle_of(&t, i)),
                Ok(Operand::Placement(_))
            ));
        }
    }

    #[test]
    fn a_raw_datum_resolves_to_its_vehicle() {
        let mut w = World::new();
        let v = w.spawn(objects::VEHICLE, -1, TAG_A);
        let got = explain(&w.g, &table(), v.raw()).unwrap();
        assert_eq!(
            got,
            Vehicle {
                handle: v,
                definition: DatumHandle(TAG_A)
            }
        );
    }

    #[test]
    fn a_raw_datum_that_is_no_vehicle_or_no_object_is_refused() {
        let mut w = World::new();
        let biped = w.spawn(objects::BIPED, -1, TAG_A);
        assert_eq!(
            explain(&w.g, &table(), biped.raw()),
            Err(Refusal::WrongType {
                handle: biped,
                kind: objects::BIPED
            })
        );
        let wrong_salt = DatumHandle::new(biped.salt() ^ 1, u32::from(biped.index()));
        assert_eq!(
            explain(&w.g, &table(), wrong_salt.raw()),
            Err(Refusal::Stale(wrong_salt))
        );
        let beyond = DatumHandle::new(0x8000, 9);
        assert_eq!(
            explain(&w.g, &table(), beyond.raw()),
            Err(Refusal::Stale(beyond))
        );
    }

    #[test]
    fn a_placement_resolves_to_the_vehicle_spawned_from_it() {
        let mut w = World::new();
        let _other = w.spawn(objects::VEHICLE, 1, TAG_B);
        let v = w.spawn(objects::VEHICLE, 0, TAG_A);
        let t = table();
        assert_eq!(explain(&w.g, &t, handle_of(&t, 0)).unwrap().handle, v);
    }

    #[test]
    fn a_placement_match_needs_type_index_and_definition() {
        let mut w = World::new();
        // Same placement index but a scenery object, a vehicle of the
        // other palette tag, and a vehicle of another placement.
        w.spawn(objects::BIPED, 0, TAG_A);
        w.spawn(objects::VEHICLE, 0, TAG_B);
        w.spawn(objects::VEHICLE, 1, TAG_A);
        w.spawn(objects::VEHICLE, -1, TAG_A);
        let t = table();
        assert_eq!(
            explain(&w.g, &t, handle_of(&t, 0)),
            Err(Refusal::NotSpawned(t.things()[0].id()))
        );
    }

    #[test]
    fn two_vehicles_of_one_placement_are_ambiguous() {
        let mut w = World::new();
        w.spawn(objects::VEHICLE, 0, TAG_A);
        w.spawn(objects::VEHICLE, 0, TAG_A);
        let t = table();
        assert_eq!(
            explain(&w.g, &t, handle_of(&t, 0)),
            Err(Refusal::Ambiguous(t.things()[0].id()))
        );
    }

    #[test]
    fn placements_the_scenario_lacks_or_that_are_no_vehicle_are_refused() {
        let w = World::new();
        let t = table();
        // The census holds vehicle 5; the scenario has two.
        assert_eq!(
            explain(&w.g, &t, handle_of(&t, 2)),
            Err(Refusal::NoSuchPlacement(t.things()[2].id()))
        );
        assert_eq!(
            explain(&w.g, &t, handle_of(&t, 3)),
            Err(Refusal::NotAVehiclePlacement(t.things()[3].id()))
        );
        // A handle the census does not hold.
        let unknown = HANDLE | 0x00ff_0000;
        assert_eq!(
            explain(&w.g, &t, unknown),
            Err(Refusal::NoSuchPlacement(0x00ff_0000))
        );
    }

    #[test]
    fn a_palette_without_a_tag_is_refused() {
        let mut w = World::new();
        w.g.put_u32(PALETTE + PALETTE_TAG, NULL_DATUM);
        w.spawn(objects::VEHICLE, 0, TAG_A);
        let t = table();
        assert_eq!(
            explain(&w.g, &t, handle_of(&t, 0)),
            Err(Refusal::NoSuchPlacement(t.things()[0].id()))
        );
    }

    #[test]
    fn is_vehicle_holds_until_the_datum_dies_or_its_slot_is_reused() {
        let mut w = World::new();
        let v = w.spawn(objects::VEHICLE, -1, TAG_A);
        assert!(is_vehicle(&w.g, v.raw()));
        w.delete(v);
        assert!(!is_vehicle(&w.g, v.raw()));
        // The slot is reused by another vehicle under a new salt.
        let again = w.spawn(objects::VEHICLE, -1, TAG_A);
        assert_ne!(again, v);
        assert!(!is_vehicle(&w.g, v.raw()));
        assert!(is_vehicle(&w.g, again.raw()));
        assert!(!is_vehicle(&w.g, u32::MAX));
        assert!(!is_vehicle(&w.g, 0));
    }

    #[test]
    fn is_vehicle_refuses_an_object_of_another_type() {
        let mut w = World::new();
        let v = w.spawn(objects::VEHICLE, -1, TAG_A);
        let b = w.spawn(objects::BIPED, -1, TAG_A);
        assert!(is_vehicle(&w.g, v.raw()));
        assert!(!is_vehicle(&w.g, b.raw()));
    }

    #[test]
    fn resolve_answers_the_salted_datum_with_or_without_a_census() {
        let mut w = World::new();
        let v = w.spawn(objects::VEHICLE, 0, TAG_A);
        let t = table();
        assert_eq!(resolve(&w.g, Some(&t), v.raw()), Some(v.raw()));
        assert_eq!(resolve(&w.g, None, v.raw()), Some(v.raw()));
        assert_eq!(resolve(&w.g, Some(&t), handle_of(&t, 0)), Some(v.raw()));
        // No census: a placement names nothing.
        assert_eq!(resolve(&w.g, None, handle_of(&t, 0)), None);
        assert_eq!(resolve(&w.g, Some(&t), 0), None);
    }

    #[test]
    fn a_pinned_placement_is_not_repointed_at_a_respawn() {
        let mut w = World::new();
        let first = w.spawn(objects::VEHICLE, 0, TAG_A);
        let t = table();
        let pinned = resolve(&w.g, Some(&t), handle_of(&t, 0)).unwrap();
        assert_eq!(pinned, first.raw());
        w.delete(first);
        let second = w.spawn(objects::VEHICLE, 0, TAG_A);
        assert!(!is_vehicle(&w.g, pinned));
        assert_eq!(
            resolve(&w.g, Some(&t), handle_of(&t, 0)),
            Some(second.raw())
        );
    }

    #[test]
    fn a_second_vehicle_of_the_placement_refuses_a_fresh_resolve() {
        let mut w = World::new();
        w.spawn(objects::VEHICLE, 0, TAG_A);
        w.spawn(objects::VEHICLE, 0, TAG_A);
        let t = table();
        assert_eq!(resolve(&w.g, Some(&t), handle_of(&t, 0)), None);
    }

    #[test]
    fn an_unmapped_object_array_is_a_fault_not_a_panic() {
        let mut g = FakeGuest::new();
        g.map(points::SCENARIO.base, 4).map(points::OBJECTS.base, 4);
        g.put_u32(points::OBJECTS.base, 0x5000_0000);
        let h = DatumHandle::new(0x8000, 0);
        assert!(matches!(vehicle_of(&g, h), Err(Refusal::Fault(_))));
        assert!(!is_vehicle(&g, h.raw()));
    }
}
