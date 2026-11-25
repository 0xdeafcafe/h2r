//! The base calls: edits of the map's own entries (`forge::base`), over
//! the fake game and the vehicle placement fixture.
use super::*;
use crate::forge::base::{BaseStep, FieldValue, RowBytes, inert_row};
use ::forge::BlockKind;

const ROW: u32 = ENTRY;
const VEHICLE_ROW: usize = 0x54;

fn guard() -> crate::forge::local::Guard {
    crate::forge::local::Guard {
        scenario: 0x8100_0000,
        generation: 0,
        admitted: |_, _| true,
    }
}

fn call(step: BaseStep) -> Call {
    Call::Forge {
        guard: guard(),
        steps: vec![ForgeStep::Base(step)],
    }
}

/// The placed vehicle with its object gone from the placement.
fn without_object() -> World {
    let (mut w, _) = placed_vehicle();
    w.g.put_u16(
        UNIT_BODY + guest_layout::Object::PLACEMENT_INDEX.offset,
        u16::MAX,
    );
    w
}

#[test]
fn a_base_move_moves_the_live_object_and_writes_the_placement() {
    let (w, id) = placed_vehicle();
    let frame = ::forge::Frame {
        position: [1.0, 2.0, 3.0],
        rotation: [0.5, 0.25, 0.125],
    };
    let (out, calls, g) = one(
        w,
        call(BaseStep::Move {
            block: BlockKind::Vehicles,
            index: 0,
            frame,
        }),
        |c, m| {
            assert_eq!(c.target, addr::OBJECT_SET_POSITION);
            assert_eq!(c.args, [id.key, 0, 0]);
            assert_eq!(vec3(m, c.eax.unwrap()), frame.position);
            al(0)
        },
    );
    assert_eq!(out.unwrap()["handles"], json!([]));
    assert_eq!(calls.len(), 1);
    assert_eq!(vec3(&g, ROW + 8), frame.position);
    assert_eq!(vec3(&g, ROW + 0x14), frame.rotation);
    // The palette index and the rest of the row are as they were.
    assert_eq!(g.u16(ROW).unwrap(), 1);
    assert_eq!(g.u16(ROW + 2).unwrap(), u16::MAX);
}

#[test]
fn a_base_move_of_an_object_that_is_gone_writes_nothing() {
    for placement in [u16::MAX, 7] {
        let (mut w, _) = placed_vehicle();
        // No placement at all, or another placement's object.
        w.g.put_u16(
            UNIT_BODY + guest_layout::Object::PLACEMENT_INDEX.offset,
            placement,
        );
        let before = w.g.bytes(ROW, VEHICLE_ROW).unwrap();
        let frame = ::forge::Frame {
            position: [1.0; 3],
            ..Default::default()
        };
        let (out, calls, g) = one(
            w,
            call(BaseStep::Move {
                block: BlockKind::Vehicles,
                index: 0,
                frame,
            }),
            |_, _| panic!("a missing object reached the game"),
        );
        let error = out.unwrap_err();
        assert_eq!(error.code, REFUSED);
        assert!(
            error.message.starts_with(crate::forge::local::OBJECT_GONE),
            "{}",
            error.message
        );
        assert!(calls.is_empty());
        assert_eq!(g.bytes(ROW, VEHICLE_ROW).unwrap(), before);
    }
}

#[test]
fn a_base_step_outside_the_block_is_refused_before_anything_is_written() {
    let (w, _) = placed_vehicle();
    let (out, calls, _) = one(
        w,
        call(BaseStep::Move {
            block: BlockKind::Vehicles,
            index: 1,
            frame: ::forge::Frame::default(),
        }),
        |_, _| panic!("a missing entry reached the game"),
    );
    assert!(out.unwrap_err().message.contains("no vehicles entry 1"));
    assert!(calls.is_empty());
}

#[test]
fn an_inert_row_is_the_kits_spare_form_and_its_object_is_deleted() {
    let (w, id) = placed_vehicle();
    let (out, calls, g) = one(
        w,
        call(BaseStep::Inert {
            block: BlockKind::Vehicles,
            index: 0,
        }),
        |c, _| {
            assert_eq!(c.target, addr::OBJECT_DELETE);
            assert_eq!(c.args, [id.key]);
            al(0)
        },
    );
    assert_eq!(out.unwrap()["handles"], json!([]));
    assert_eq!(calls.len(), 1);
    let inert = inert_row(BlockKind::Vehicles);
    assert_eq!(g.bytes(ROW, VEHICLE_ROW).unwrap(), inert);
    assert_eq!(&inert[..4], [0xff; 4]);
    assert!(inert[4..].iter().all(|b| *b == 0));

    // An entry whose object is already gone is still made inert.
    let (out, calls, g) = one(
        without_object(),
        call(BaseStep::Inert {
            block: BlockKind::Vehicles,
            index: 0,
        }),
        |_, _| panic!("there was no object to delete"),
    );
    assert!(out.is_ok() && calls.is_empty());
    assert_eq!(g.bytes(ROW, VEHICLE_ROW).unwrap(), inert);
}

#[test]
fn restoring_a_row_writes_the_kept_bytes_then_makes_its_object_from_them() {
    let (w, _) = placed_vehicle();
    let kept = w.g.bytes(ROW, VEHICLE_ROW).unwrap();
    let row = RowBytes::new(BlockKind::Vehicles, &kept).unwrap();
    let restore = || {
        call(BaseStep::Restore {
            block: BlockKind::Vehicles,
            index: 0,
            row,
        })
    };
    // The row was made inert and its object deleted.
    let inert = || {
        let mut w = without_object();
        w.g.put(ROW, &inert_row(BlockKind::Vehicles));
        w
    };
    let (out, calls, g) = one(inert(), restore(), |c, m| match c.target {
        addr::PLACEMENT_NEW => al(0),
        addr::OBJECT_NEW => {
            assert_eq!(
                vec3(m, SCRATCH + addr::PLACEMENT_POSITION),
                [3.0, -4.0, 8.0]
            );
            assert_eq!(m.u16(SCRATCH + addr::PLACEMENT_INDEX).unwrap(), 0);
            al(0x0042_0003)
        }
        _ => panic!("unexpected guest call"),
    });
    assert_eq!(out.unwrap()["handles"], json!([]));
    assert_eq!(
        calls.iter().map(|c| c.target).collect::<Vec<_>>(),
        [addr::PLACEMENT_NEW, addr::OBJECT_NEW]
    );
    assert_eq!(g.bytes(ROW, VEHICLE_ROW).unwrap(), kept);

    // An object the game refuses leaves the row inert, and says so.
    let (out, _, g) = one(inert(), restore(), |c, _| {
        if c.target == addr::OBJECT_NEW {
            al(u32::MAX)
        } else {
            al(0)
        }
    });
    assert!(out.unwrap_err().message.contains("object_new"));
    assert_eq!(
        g.bytes(ROW, VEHICLE_ROW).unwrap(),
        inert_row(BlockKind::Vehicles)
    );

    // A row whose object is still there is not restored over it.
    let (w, _) = placed_vehicle();
    let (out, calls, _) = one(w, restore(), |_, _| {
        panic!("a restore over a live object reached the game")
    });
    assert!(
        out.unwrap_err()
            .message
            .contains("already has a live object")
    );
    assert!(calls.is_empty());
}

#[test]
fn a_base_field_write_changes_one_field_of_the_row() {
    let (w, _) = placed_vehicle();
    let (out, calls, g) = one(
        w,
        call(BaseStep::Field {
            block: BlockKind::Vehicles,
            index: 0,
            field: "scale",
            value: FieldValue::Float(2.0),
        }),
        |_, _| panic!("a field write needs no guest call"),
    );
    assert!(out.is_ok() && calls.is_empty());
    assert_eq!(g.f32(ROW + 0x20).unwrap(), 2.0);
    assert_eq!(vec3(&g, ROW + 8), [3.0, -4.0, 8.0]);

    // A palette index is found by the datum of the entry it selects.
    let step = |value| BaseStep::Field {
        block: BlockKind::Vehicles,
        index: 0,
        field: "palette index",
        value,
    };
    let (mut w, _) = placed_vehicle();
    w.g.put_u16(ROW, 0);
    let (out, _, g) = one(w, call(step(FieldValue::Palette { datum: TAG })), |_, _| {
        panic!("no guest call")
    });
    assert!(out.is_ok());
    assert_eq!(g.u16(ROW).unwrap(), 1);

    // A tag the palette does not hold, and a field the layout does not
    // have, are refused with the row as it was.
    for bad in [
        step(FieldValue::Palette { datum: 0x1111_1111 }),
        BaseStep::Field {
            block: BlockKind::Vehicles,
            index: 0,
            field: "no such field",
            value: FieldValue::Int(1),
        },
    ] {
        let (w, _) = placed_vehicle();
        let before = w.g.bytes(ROW, VEHICLE_ROW).unwrap();
        let (out, _, g) = one(w, call(bad), |_, _| panic!("no guest call"));
        assert!(out.is_err());
        assert_eq!(g.bytes(ROW, VEHICLE_ROW).unwrap(), before);
    }
}

#[test]
fn a_block_with_no_objects_takes_row_writes_alone() {
    // Netgame flags: the block at 0x118, a row of 0x20 bytes.
    let (scenario, flags) = (0x8100_0000, 0x8200_0000);
    let mut w = world();
    w.g.put_u32(guest_layout::global::SCENARIO.base, scenario)
        .put_u32(scenario + 0x118, 2)
        .put_u32(scenario + 0x11c, flags)
        .map(flags, 0x40);
    let frame = ::forge::Frame {
        position: [5.0, 6.0, 7.0],
        rotation: [1.5, 0.0, 0.0],
    };
    let steps = vec![
        ForgeStep::Base(BaseStep::Move {
            block: BlockKind::NetgameFlags,
            index: 0,
            frame,
        }),
        ForgeStep::Base(BaseStep::Inert {
            block: BlockKind::NetgameFlags,
            index: 1,
        }),
    ];
    let (out, calls, g) = one(
        w,
        Call::Forge {
            guard: guard(),
            steps,
        },
        |_, _| panic!("a block with no objects needs no guest call"),
    );
    assert_eq!(out.unwrap()["handles"], json!([]));
    assert!(calls.is_empty());
    // A flag has a facing, not a rotation: the yaw is it.
    assert_eq!(vec3(&g, flags), frame.position);
    assert_eq!(g.f32(flags + 0x0c).unwrap(), 1.5);
    assert_eq!(
        g.bytes(flags + 0x20, 0x20).unwrap(),
        inert_row(BlockKind::NetgameFlags)
    );
}

/// A map with the kit: the starting location block at `0x100` holds one
/// active entry and two spare rows, in the spare form the kit gives them.
const START: usize = 0x34;
const SCENARIO: u32 = 0x8100_0000;
const STARTS: u32 = 0x8200_0000;

fn kit_world(spare_rows: u32) -> World {
    let mut w = world();
    w.g.put_u32(guest_layout::global::SCENARIO.base, SCENARIO)
        .put_u32(SCENARIO + 0x100, 1)
        .put_u32(SCENARIO + 0x104, STARTS)
        .map(STARTS, 0x100);
    let spare = inert_row(BlockKind::StartingLocations);
    for i in 0..spare_rows {
        w.g.put(STARTS + (1 + i) * START as u32, &spare);
    }
    w
}

fn start_row(team: i64) -> RowBytes {
    let f = |v: f32| forge_doc::F32::new(v).unwrap();
    crate::forge::base::new_row(
        BlockKind::StartingLocations,
        &[
            (
                "position",
                forge_doc::Value::Vec3([f(5.0), f(6.0), f(7.0)]),
            ),
            ("facing", forge_doc::Value::Float(f(1.5))),
            ("team", forge_doc::Value::Int(team)),
        ],
        |_| Ok(0),
    )
    .unwrap()
}

fn add(index: u32, team: i64) -> BaseStep {
    BaseStep::Add {
        block: BlockKind::StartingLocations,
        index,
        row: start_row(team),
    }
}

#[test]
fn a_kit_add_writes_the_spare_row_then_raises_the_count() {
    let (out, calls, g) = one(kit_world(2), call(add(1, 2)), |_, _| {
        panic!("a kit row needs no guest call")
    });
    assert_eq!(out.unwrap()["handles"], json!([]));
    assert!(calls.is_empty());
    assert_eq!(g.u32(SCENARIO + 0x100).unwrap(), 2);
    let row = STARTS + START as u32;
    assert_eq!(g.bytes(row, START).unwrap(), start_row(2).as_slice());
    assert_eq!(vec3(&g, row), [5.0, 6.0, 7.0]);
    assert_eq!(g.f32(row + 0x0c).unwrap(), 1.5);
    assert_eq!(g.u16(row + 0x10).unwrap(), 2);
    // The player type is the zero a materialised entry has, not the spare -1.
    assert_eq!(g.u16(row + 0x2c).unwrap(), 0);
    // The entry before it is as it was.
    assert_eq!(g.bytes(STARTS, START).unwrap(), vec![0; START]);
}

#[test]
fn a_batch_of_kit_adds_names_rows_past_the_count_in_order() {
    let steps = vec![
        ForgeStep::Base(add(1, 0)),
        ForgeStep::Base(add(2, 1)),
    ];
    let (out, _, g) = one(
        kit_world(2),
        Call::Forge {
            guard: guard(),
            steps,
        },
        |_, _| panic!("a kit row needs no guest call"),
    );
    assert!(out.is_ok());
    assert_eq!(g.u32(SCENARIO + 0x100).unwrap(), 3);
    assert_eq!(g.u16(STARTS + 2 * START as u32 + 0x10).unwrap(), 1);
}

/// A kit world whose block already counts `rows` of the session's own
/// entries after the map's one.
fn counted(rows: u32) -> World {
    let mut w = kit_world(2);
    w.g.put_u32(SCENARIO + 0x100, 1 + rows);
    for i in 0..rows {
        w.g.put(STARTS + (1 + i) * START as u32, start_row(2).as_slice());
    }
    w
}

fn drop_row(index: u32) -> ForgeStep {
    ForgeStep::Base(BaseStep::Drop {
        block: BlockKind::StartingLocations,
        index,
    })
}

#[test]
fn a_kit_drop_writes_the_spare_form_back_and_lowers_the_count() {
    let (out, _, g) = one(counted(1), call(BaseStep::Drop {
        block: BlockKind::StartingLocations,
        index: 1,
    }), |_, _| panic!("a kit row needs no guest call"));
    assert!(out.is_ok());
    assert_eq!(g.u32(SCENARIO + 0x100).unwrap(), 1);
    assert_eq!(
        g.bytes(STARTS + START as u32, START).unwrap(),
        inert_row(BlockKind::StartingLocations)
    );

    // Drops go last row first, in one batch.
    let (out, _, g) = one(
        counted(2),
        Call::Forge {
            guard: guard(),
            steps: vec![drop_row(2), drop_row(1)],
        },
        |_, _| panic!("a kit row needs no guest call"),
    );
    assert!(out.is_ok());
    assert_eq!(g.u32(SCENARIO + 0x100).unwrap(), 1);

    // Only the last row goes: the first of two is not the kit's to drop,
    // and nothing is written.
    let (out, _, g) = one(
        counted(2),
        Call::Forge {
            guard: guard(),
            steps: vec![drop_row(1)],
        },
        |_, _| panic!("a kit row needs no guest call"),
    );
    assert!(out.unwrap_err().message.contains("last starting_locations row is 2, not 1"));
    assert_eq!(g.u32(SCENARIO + 0x100).unwrap(), 3);
}

#[test]
fn a_map_without_the_kit_refuses_the_add_and_writes_nothing() {
    let (out, _, g) = one(kit_world(0), call(add(1, 2)), |_, _| panic!("no guest call"));
    let error = out.unwrap_err();
    assert_eq!(error.code, REFUSED);
    assert!(
        error.message.starts_with(crate::forge::local::NO_KIT),
        "{}",
        error.message
    );
    assert_eq!(g.u32(SCENARIO + 0x100).unwrap(), 1);
    assert_eq!(
        g.bytes(STARTS + START as u32, START).unwrap(),
        vec![0; START]
    );
}

#[test]
fn a_kit_add_names_the_next_row_and_a_kit_block() {
    let (out, _, g) = one(kit_world(2), call(add(2, 0)), |_, _| panic!("no guest call"));
    assert!(out.unwrap_err().message.contains("next starting_locations row is 1, not 2"));
    assert_eq!(g.u32(SCENARIO + 0x100).unwrap(), 1);

    // Weapons are placements, not kit rows.
    let step = BaseStep::Drop {
        block: BlockKind::Weapons,
        index: 0,
    };
    let (out, _, _) = one(kit_world(2), call(step), |_, _| panic!("no guest call"));
    assert!(out.unwrap_err().message.contains("the kit has no weapons rows"));
}
