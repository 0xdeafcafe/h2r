use super::{cast::*, *};
use guest_layout::datum::layout;
use guest_layout::entry::EntryFuture;
use guest_layout::entry::Regs;
use guest_layout::entry::testing::{Run, run};
use guest_layout::testing::FakeGuest;

const SCENARIO: u32 = 0x1000;
const DEFINITIONS: u32 = 0x2000;
const GLOBALS: u32 = 0x3000;
const GLOBAL_ROWS: u32 = 0xe000;
const OBJECT_POOL: u32 = 0x12000;
const OBJECT_ROWS: u32 = 0x13000;
const OBJECT_BODY: u32 = 0x14000;
const SPEAKER: u32 = 0x8900_0000;
const ACTOR_POOL: u32 = 0x15000;
const ACTOR_ROWS: u32 = 0x16000;
const SPEAKER_ACTOR: u32 = 0x8a00_0000;
const COMMAND_POOL: u32 = 0x17000;
const COMMAND_ROWS: u32 = 0x18000;
const ORDINARY_COMMAND: u32 = 0x8b00_0000;
const STAGING_COMMAND: u32 = 0x8b01_0001;
const SOUNDS: u32 = 0x5000;
const INSTANCES: u32 = 0x6000;
const SCRATCH: u32 = 0x7000;
const TAG_INDEX: u32 = 0x8000;
const TAG_ENTRIES: u32 = 0x9000;
const TAG_GROUP: u32 = 0xa000;
const DESCRIPTOR: u32 = 0xb000;
const STACK: u32 = 0xc000;
const ARGS: u32 = 0xd000;
const TEST_TAG: u32 = 0x8000_0000;
const FIRST: u32 = 0x8100_0000;
const SECOND: u32 = 0x8101_0001;

fn cell(i: u32) -> u32 {
    GLOBAL_ROWS + (SCENARIO_GLOBAL_OFFSET + i) * 8 + 4
}

fn fixture() -> FakeGuest {
    let mut g = FakeGuest::new();
    g.put_u32(points::SCENARIO.base, SCENARIO)
        .put_u32(SCENARIO + 0x1c0, 4)
        .put_u32(SCENARIO + 0x1c4, DEFINITIONS)
        .put_u32(GLOBAL_VALUES, GLOBALS)
        .put_u32(GLOBALS + layout::DATA, GLOBAL_ROWS)
        .put_u32(GLOBALS + layout::HIGH_WATER, SCENARIO_GLOBAL_OFFSET + 4)
        .put_u32(GLOBALS + layout::ELEMENT_SIZE, 8)
        .put_u32(SOUND_INSTANCES, SOUNDS)
        .put_u32(SOUNDS + layout::DATA, INSTANCES)
        .put_u32(SOUNDS + layout::HIGH_WATER, 2)
        .put_u32(SOUNDS + layout::ELEMENT_SIZE, 0xbc)
        // The native start stubs write their instance rows here.
        .map(INSTANCES, 2 * 0xbc)
        .put_u32(tags::TAG_INDEX_GLOBAL, TAG_INDEX)
        .put_u32(tags::TAG_ENTRIES_GLOBAL, TAG_ENTRIES)
        .put_u32(TAG_INDEX + tags::index_layout::GROUPS, TAG_GROUP)
        .put_u32(TAG_INDEX + tags::index_layout::GROUP_COUNT, 1)
        .put_u32(TAG_INDEX + tags::index_layout::TAG_COUNT, 1)
        .put_u32(TAG_ENTRIES, GroupTag::from_chars(*b"snd!").0)
        .put_u32(TAG_ENTRIES + tags::entry_layout::DATUM, TEST_TAG)
        .put_u32(TAG_ENTRIES + tags::entry_layout::ADDRESS, SCRATCH + 0x100)
        .put_u32(TAG_GROUP, GroupTag::from_chars(*b"snd!").0)
        .put_u32(BUILTIN_TABLE + u32::from(BUILTINS[0].1) * 4, DESCRIPTOR)
        .put_u32(STACK + 8, 0x8110_0001)
        .put_u32(STACK + 12, 1)
        .map(SCRATCH, 0x100)
        .put_u32(points::OBJECTS.base, OBJECT_POOL)
        .put_u32(OBJECT_POOL + layout::DATA, OBJECT_ROWS)
        .put_u32(OBJECT_POOL + layout::HIGH_WATER, 1)
        .put_u32(OBJECT_POOL + layout::ELEMENT_SIZE, 12)
        .put_u16(OBJECT_ROWS, (SPEAKER >> 16) as u16)
        .put_u32(OBJECT_ROWS + 8, OBJECT_BODY);
    for (i, (ty, value)) in [
        (SOUND, TEST_TAG),
        (LONG, NONE),
        (LONG, NONE),
        (OBJECT, SPEAKER),
    ]
    .into_iter()
    .enumerate()
    {
        let i = i as u32;
        g.put_u16(DEFINITIONS + i * 0x28 + 0x20, ty);
        g.put_u16(cell(i) - 4, 0x8000);
        g.put_u32(cell(i), value);
    }
    g
}

fn invoke(mut g: FakeGuest, op: Op, args: [u32; 3], handle: u32) -> Run {
    g.put_u32(ARGS, op as u32);
    for (i, v) in args.into_iter().enumerate() {
        g.put_u32(ARGS + 4 + 4 * i as u32, v);
    }
    let entry = |cx| -> EntryFuture { Box::pin(evaluate(cx, SCRATCH, u32::from(BUILTINS[0].1))) };
    let r = run(
        &entry,
        g,
        Regs {
            esp: STACK,
            ..Regs::default()
        },
        |call, m| {
            let eax = match call.target {
                EVAL_ARGUMENTS => {
                    assert_eq!(call.args, [0x8110_0001, 4, DESCRIPTOR + 0xe, 1]);
                    ARGS
                }
                RADIO_START => {
                    assert_eq!(call.eax, Some(SCRATCH));
                    assert_eq!(call.args, [TEST_TAG]);
                    assert_eq!(m.u32(SCRATCH).unwrap(), 0x400);
                    assert_eq!(m.u32(SCRATCH + 0x44).unwrap(), NONE);
                    let row = INSTANCES + (handle & 0xffff) * 0xbc;
                    m.set_u16(row, (handle >> 16) as u16).unwrap();
                    m.set_u8(row + 2, 0).unwrap();
                    handle
                }
                SPATIAL_START => {
                    assert_eq!(call.esi, Some(SPEAKER));
                    assert_eq!(call.eax, Some(HEAD_MARKER));
                    assert_eq!(call.args, [SCRATCH]);
                    assert_eq!(m.u32(SCRATCH + 4).unwrap(), TEST_TAG);
                    let row = INSTANCES + (handle & 0xffff) * 0xbc;
                    m.set_u16(row, (handle >> 16) as u16).unwrap();
                    m.set_u8(row + 2, 0).unwrap();
                    handle
                }
                BIPED_LIP_SYNC => {
                    assert_eq!(call.ebx, Some(SPEAKER));
                    assert_eq!(call.args, [TEST_TAG, handle]);
                    0
                }
                STOP_INSTANCE => {
                    let stopped = call.ebx.unwrap();
                    m.set_u8(INSTANCES + (stopped & 0xffff) * 0xbc + 2, 1)
                        .unwrap();
                    0
                }
                DELETE_COMMAND => {
                    assert_eq!(call.ecx, Some(SPEAKER_ACTOR));
                    assert_eq!(call.args, [STAGING_COMMAND]);
                    m.set_u32(COMMAND_ROWS + COMMAND_NEXT, NONE).unwrap();
                    m.set_u16(COMMAND_ROWS + 0xd4, 0).unwrap();
                    0
                }
                EVAL_RETURN => {
                    assert_eq!(call.ecx, Some(0x8110_0001));
                    0
                }
                v => panic!("unexpected native call {v:#x}"),
            };
            Regs {
                eax,
                ..Regs::default()
            }
        },
    );
    assert_eq!(r.result, Ok(Ret::new(0, 12)));
    r
}

#[test]
fn conversation_sound_handles_do_not_share_timing_or_stop() {
    let a = invoke(fixture(), Op::StartSound, [0, 3, 1], FIRST);
    let b = invoke(a.guest, Op::StartSound, [0, NONE, 2], SECOND);
    assert_eq!(b.guest.u32(cell(1)).unwrap(), FIRST);
    assert_eq!(b.guest.u32(cell(2)).unwrap(), SECOND);

    let a = invoke(b.guest, Op::StopSound, [1, 0, 0], NONE);
    assert_eq!(
        a.calls
            .iter()
            .find(|c| c.target == STOP_INSTANCE)
            .unwrap()
            .ebx,
        Some(FIRST)
    );
    let b = invoke(a.guest, Op::PollSound, [2, 0, 0], NONE);
    assert_eq!(b.guest.u32(cell(1)).unwrap(), NONE);
    assert_eq!(b.guest.u32(cell(2)).unwrap(), SECOND);

    let mut g = b.guest;
    // Slot 1 has recycled. Its new occupant must not prolong or stop the line.
    g.put_u16(INSTANCES + 0xbc, 0x8300);
    let b = invoke(g, Op::PollSound, [2, 0, 0], NONE);
    assert_eq!(b.guest.u32(cell(2)).unwrap(), NONE);
    assert!(b.calls.iter().all(|c| c.target != STOP_INSTANCE));
}

#[test]
fn conversation_cancel_removes_its_command_from_a_stack() {
    let mut g = fixture();
    g.put_u32(OBJECT_BODY + UNIT_ACTOR, SPEAKER_ACTOR)
        .put_u32(ACTORS, ACTOR_POOL)
        .put_u32(ACTOR_POOL + layout::DATA, ACTOR_ROWS)
        .put_u32(ACTOR_POOL + layout::HIGH_WATER, 1)
        .put_u32(ACTOR_POOL + layout::ELEMENT_SIZE, 0x888)
        .put_u16(ACTOR_ROWS, (SPEAKER_ACTOR >> 16) as u16)
        .put_u32(ACTOR_ROWS + ACTOR_UNIT, SPEAKER)
        .put_u32(ACTOR_ROWS + ACTOR_COMMAND_STACK, ORDINARY_COMMAND)
        .put_u32(COMMANDS, COMMAND_POOL)
        .put_u32(COMMAND_POOL + layout::DATA, COMMAND_ROWS)
        .put_u32(COMMAND_POOL + layout::HIGH_WATER, 2)
        .put_u32(COMMAND_POOL + layout::ELEMENT_SIZE, 0xd4)
        .put_u16(COMMAND_ROWS, (ORDINARY_COMMAND >> 16) as u16)
        .put_u16(COMMAND_ROWS + 0xd4, (STAGING_COMMAND >> 16) as u16)
        .put_u32(COMMAND_ROWS + COMMAND_SCRIPT, 41)
        .put_u32(COMMAND_ROWS + COMMAND_NEXT, STAGING_COMMAND)
        .put_u32(COMMAND_ROWS + 0xd4 + COMMAND_SCRIPT, 73)
        .put_u32(COMMAND_ROWS + 0xd4 + COMMAND_NEXT, NONE);

    let r = invoke(g, Op::CancelCommand, [3, 73, 0], NONE);
    assert!(r.calls.iter().any(|c| c.target == DELETE_COMMAND));
    assert_eq!(
        r.guest.u32(ACTOR_ROWS + ACTOR_COMMAND_STACK).unwrap(),
        ORDINARY_COMMAND
    );
    assert_eq!(r.guest.u32(COMMAND_ROWS + COMMAND_NEXT).unwrap(), NONE);
    let r = invoke(r.guest, Op::CancelCommand, [3, 73, 0], NONE);
    assert!(!r.calls.iter().any(|c| c.target == DELETE_COMMAND));
}
