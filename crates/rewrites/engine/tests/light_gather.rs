//! The light frame body (`engine::render::light_gather::frame`, slice L2
//! of docs/graphics/native-light-gather.md) over `entry::testing::run`:
//! synthetic guest state, every guest call answered by a stub.

use std::sync::{Arc, Mutex};

use engine::entry::{Cx, EntryFuture, GuestCall, Regs, Ret, testing};
use engine::light_constants::Fpu;
use engine::memory::GuestMemory;
use engine::render::light_gather::frame::{self, LightFrame, call, site, slot};
use engine::render::light_gather::{History, HistoryRow, MAIN_VIS, Refusal, VisibleLight, Writes};
use engine::testing::FakeGuest;

const ENTRY_ESP: u32 = 0x0010_0000;
const S: u32 = ENTRY_ESP - frame::FRAME;
const FPU: Fpu = Fpu {
    mxcsr: 0x1f80,
    x87_control: 0x027f,
};

const CANDIDATES: u32 = MAIN_VIS + 0x10;
const RENDER_FRAME: u32 = 0x004b_a034;
const VIEW_INDEX: u32 = 0x004b_9ed4;
const OVERRIDE: u32 = 0x0051_0c50;
const HISTORY: u32 = 0x0052_34e8;
const OBJECTS: u32 = 0x004e_0300;
const RECORDS_VIS: u32 = 0x004c_0b80;

const CANDIDATE_LIST: u32 = 0x0200_0000;
const FROZEN: u32 = 0x0201_0000;
const RECORDS: u32 = 0x0203_0000;
const LIGHT_POOL: u32 = 0x0204_0000;
const LIGHT_DATA: u32 = 0x0205_0000;
const TAG_TABLE: u32 = 0x0206_0000;
const DEFINITIONS: u32 = 0x0207_0000;
const OBJECT_POOL: u32 = 0x0208_0000;
const OBJECT_DATA: u32 = 0x0209_0000;
const OWNER_BODY: u32 = 0x020a_0000;
const OWNER: u32 = 0x0001_0003;
const VIS: u32 = 0x0300_0000;
const FRAME_NUMBER: u32 = 100;

/// One light of the fixture.
#[derive(Clone, Copy)]
struct Lit {
    weight: f32,
    flags: u32,
    /// `Some(owner)` attaches it (`+0x54` 2, `+0x4c` the owner).
    owner: Option<u32>,
}

fn datum(i: u32) -> u32 {
    0x0001_0000 | i
}

fn record(i: u32) -> u32 {
    LIGHT_DATA + i * 0x110
}

/// The guest a frame starts from: no candidates and the frozen override,
/// so the visible list is the history as seeded, one row per light.
fn world(lights: &[Lit]) -> FakeGuest {
    let mut g = FakeGuest::new();
    g.map(ENTRY_ESP - 0x2000, 0x2010);
    g.put_u32(CANDIDATES, CANDIDATE_LIST);
    g.put_u32(CANDIDATE_LIST, 128)
        .put_u16(CANDIDATE_LIST + 4, 0);
    g.put_u32(RENDER_FRAME, FRAME_NUMBER).put_u32(VIEW_INDEX, 0);
    g.put_u32(OVERRIDE, FROZEN).put_u8(FROZEN + 5, 1);
    // The threshold and zero words, the gather's globals and the table.
    g.map(0x0045_d000, 0x1000);
    g.map(0x004b_9000, 0x2000);
    g.map(frame::FIRST_PERSON_TABLE, 8 * frame::FIRST_PERSON_SLOT);
    g.map(frame::FIRST_PERSON_COUNT, 4);
    g.put_u32(RECORDS_VIS, RECORDS);
    // The light pool's stride word is left 0, so the Rust leaves refuse
    // (Layout) and give their steps to the guest.
    g.put_u32(engine::render::light::ARRAY_POINTER, LIGHT_POOL);
    g.put_u32(LIGHT_POOL + 0x44, LIGHT_DATA);
    g.map(LIGHT_DATA, 0x110 * lights.len().max(1) as u32);
    g.put_u32(engine::tags::TAG_ENTRIES_GLOBAL, TAG_TABLE);
    g.put_u32(OBJECTS, OBJECT_POOL)
        .put_u32(OBJECT_POOL + 0x44, OBJECT_DATA);
    g.put_u32(OBJECT_DATA + (OWNER & 0xffff) * 12 + 8, OWNER_BODY);
    g.put_u32(OWNER_BODY + 0x14, u32::MAX);

    let mut slots = Vec::new();
    for (i, lit) in lights.iter().enumerate() {
        let i = i as u32;
        let r = record(i);
        let definition = 0xa000_0000 | i;
        let data = DEFINITIONS + i * 0x100;
        g.put_u32(r + 4, definition);
        g.put_u16(r + 0x48, 5);
        match lit.owner {
            Some(owner) => {
                g.put_u16(r + 0x54, 2).put_u32(r + 0x4c, owner);
            }
            None => {
                g.put_u16(r + 0x54, 0xffff);
            }
        }
        g.put_u32(TAG_TABLE + (definition & 0xffff) * 16 + 8, data);
        g.put_u32(data, lit.flags);
        let weight = lit.weight.to_bits();
        slots.push(HistoryRow {
            visible: VisibleLight {
                light: datum(i),
                weight,
                ..VisibleLight::default()
            },
            rate: 0,
            weight,
            seen: FRAME_NUMBER,
            first: FRAME_NUMBER,
        });
    }
    let count = slots.len();
    let mut writes = Writes::new();
    History { slots, count }.encode(HISTORY, &mut writes);
    g.map(HISTORY, 0x484);
    writes.apply(&mut g).unwrap();
    g
}

struct Ran {
    run: testing::Run,
    frame: Option<LightFrame>,
    /// Each call's target and the call site the body named while it ran.
    sites: Vec<(u32, Option<u32>)>,
}

fn run_frame(g: FakeGuest) -> Ran {
    let plan = frame::plan(&g, ENTRY_ESP, FPU).expect("the plan");
    let out = Arc::new(Mutex::new(None));
    let kept = Arc::clone(&out);
    let f = move |cx: Cx| -> EntryFuture {
        let plan = plan.clone();
        let kept = Arc::clone(&kept);
        Box::pin(async move {
            let (ret, light_frame) = frame::run(&cx, plan).await?;
            *kept.lock().unwrap() = Some(light_frame);
            Ok::<_, engine::entry::EntryError>(ret)
        })
    };
    let mut sites = Vec::new();
    let regs = Regs {
        esp: ENTRY_ESP,
        ..Regs::default()
    };
    let run = testing::run(&f, g, regs, |c: &GuestCall, _m: &mut dyn GuestMemory| {
        sites.push((c.target, frame::call_site()));
        match c.target {
            call::SCENE_VISIBILITY => Regs {
                eax: VIS,
                ..Regs::default()
            },
            _ => Regs::default(),
        }
    });
    let frame = out.lock().unwrap().take();
    Ran { run, frame, sites }
}

#[test]
fn a_light_frame_calls_the_gather_in_the_guests_order() {
    let attached = Lit {
        weight: 1.0,
        flags: 0x20,
        owner: Some(OWNER),
    };
    let unlit = Lit {
        weight: 0.0,
        flags: 0,
        owner: None,
    };
    let ran = run_frame(world(&[attached, unlit]));
    assert_eq!(ran.run.result, Ok(Ret::new(0, 0)));
    let targets: Vec<u32> = ran.run.calls.iter().map(|c| c.target).collect();
    assert_eq!(
        targets,
        [
            call::REPOSE_MARKER,
            call::FIRST_PERSON_MARKER,
            call::REPOSE_BASIS,
            call::LIGHT_VOLUME,
            call::LIGHT_FRUSTA,
            call::SCENE_VISIBILITY,
            call::GEOMETRY_FLAGS,
            call::RESET_RECORDS,
            call::CLUSTER_RECORDS,
            call::PLACEMENT_RECORDS,
            call::OBJECT_RECORDS,
            call::PART_VISIBILITY,
            call::LIGHT_PASSES,
        ],
        "the second row's weight is not above the threshold: no steps"
    );
    for c in &ran.run.calls {
        assert_eq!(
            c.reserve,
            frame::FRAME,
            "{:#x} is made below the frame",
            c.target
        );
        assert_eq!(c.esp(ENTRY_ESP), S - 4 * c.args.len() as u32 - 4);
    }
    assert!(
        ran.sites
            .contains(&(call::RESET_RECORDS, Some(site::RESET_RECORDS)))
    );
    assert!(
        ran.sites
            .contains(&(call::LIGHT_PASSES, Some(site::LIGHT_PASSES)))
    );
    assert_eq!(frame::call_site(), None, "cleared after the last call");

    let r = record(0);
    let made = |target| ran.run.calls.iter().find(|c| c.target == target).unwrap();
    assert_eq!(made(call::REPOSE_MARKER).ecx, Some(OWNER));
    assert_eq!(
        made(call::REPOSE_MARKER).eax,
        Some(2),
        "AX the attachment, the definition's index in the high half"
    );
    assert_eq!(made(call::LIGHT_VOLUME).eax, Some(S + slot::VOLUME));
    assert_eq!(made(call::LIGHT_VOLUME).edx, Some(datum(0)));
    assert_eq!(made(call::LIGHT_VOLUME).args, [1]);
    assert_eq!(
        made(call::LIGHT_FRUSTA).args,
        [
            r + 0x84,
            S + slot::VOLUME,
            S + slot::FRUSTA,
            S + slot::FRUSTA_COUNT
        ]
    );
    assert_eq!(
        made(call::SCENE_VISIBILITY).args,
        [0, S + slot::FRUSTA, r + 0x18, 0, MAIN_VIS, 0, 5]
    );
    let flags = made(call::GEOMETRY_FLAGS);
    assert_eq!((flags.eax, flags.esi), (Some(datum(0)), Some(VIS)));
    assert_eq!(
        flags.args,
        [OWNER, OWNER, 1, S + slot::SHADOW],
        "an empty volume sets the point byte"
    );
    assert_eq!(made(call::RESET_RECORDS).ebx, Some(RECORDS));
    assert_eq!(made(call::PLACEMENT_RECORDS).args, [RECORDS]);
    assert_eq!(made(call::OBJECT_RECORDS).args, [1]);
    let passes = made(call::LIGHT_PASSES);
    assert_eq!(passes.eax, Some(0xa000_0000));
    assert_eq!(passes.ecx, Some(S + slot::VOLUME));
    // The shadow dword: the byte over the list's argument pushed there.
    let shadow = (S + slot::ROWS) & !0xff;
    assert_eq!(passes.args, [datum(0), r + 0x84, shadow, 1.0f32.to_bits()]);

    // The visible list is in the frame's rows, as the guest leaves it.
    let g = &ran.run.guest;
    assert_eq!(g.u32(S + slot::ROWS + 4).unwrap(), datum(0));
    assert_eq!(g.u32(S + slot::ROWS + 0x14 + 4).unwrap(), datum(1));
    let light_frame = ran.frame.expect("the frame");
    assert_eq!(light_frame.lights.len(), 2);
    assert!(light_frame.lights[0].drawn && !light_frame.lights[1].drawn);
    let gathered = light_frame.lights[0].gathered.as_ref().unwrap();
    assert_eq!(gathered.repose, frame::Repose::Asked { marker: 0 });
    assert_eq!(gathered.volume, frame::By::Guest(Refusal::Layout));
    assert_eq!(gathered.slot, Some(0));
    assert!(!gathered.shadow && gathered.source_vis);
}

#[test]
fn the_ninth_light_takes_no_first_person_slot() {
    let lit = Lit {
        weight: 1.0,
        flags: 0,
        owner: None,
    };
    let ran = run_frame(world(&[lit; 9]));
    assert_eq!(ran.run.result, Ok(Ret::new(0, 0)));
    let g = &ran.run.guest;
    assert_eq!(g.i32(frame::FIRST_PERSON_COUNT).unwrap(), 8);
    for k in 0..8 {
        let at = frame::FIRST_PERSON_TABLE + 0x400 + k * frame::FIRST_PERSON_SLOT;
        assert_eq!(g.u32(at).unwrap(), datum(k), "slot {k}");
    }
    let light_frame = ran.frame.unwrap();
    let slots: Vec<_> = light_frame
        .lights
        .iter()
        .map(|l| l.gathered.as_ref().unwrap().slot)
        .collect();
    assert_eq!(slots[7], Some(7));
    assert_eq!(slots[8], None);
    let passes = ran
        .run
        .calls
        .iter()
        .filter(|c| c.target == call::LIGHT_PASSES)
        .count();
    assert_eq!(passes, 9, "the ninth light is still drawn");
}

#[test]
fn a_refused_plan_writes_nothing() {
    // A re-pose is due and the light has no owner: the guest's marker
    // lookup would read through -1.
    let orphan = Lit {
        weight: 1.0,
        flags: 0x20,
        owner: Some(u32::MAX),
    };
    let g = world(&[orphan]);
    let before = (
        g.bytes(HISTORY, 0x484).unwrap(),
        g.bytes(S, frame::FRAME as usize).unwrap(),
    );
    assert_eq!(
        frame::plan(&g, ENTRY_ESP, FPU).err(),
        Some(Refusal::Unsupported)
    );
    let after = (
        g.bytes(HISTORY, 0x484).unwrap(),
        g.bytes(S, frame::FRAME as usize).unwrap(),
    );
    assert_eq!(before, after);

    // A float state the ports do not compute under.
    let g = world(&[Lit {
        weight: 1.0,
        flags: 0,
        owner: None,
    }]);
    let flush = Fpu {
        mxcsr: 0x9f80,
        ..FPU
    };
    assert_eq!(
        frame::plan(&g, ENTRY_ESP, flush).err(),
        Some(Refusal::Unsupported)
    );
    // A frame below the entry ESP that is not there.
    assert_eq!(
        frame::plan(&g, 0x0700_0000, FPU).err(),
        Some(Refusal::Memory)
    );
}

// ---------------------------------------------------------------------
// L4: a light's item lists (`light_gather::items`).

mod items_body {
    use super::*;
    use engine::render::light_gather::items::{self, FirstPerson, LightItems, Made, call, stack};

    const CURRENT_VIS: u32 = 0x004c_0b80;
    const COUNT: u32 = 0x004c_0b78;
    const MAIN_COUNT: u32 = 0x004c_0b7c;
    const RECORDS: u32 = 0x004b_a138;
    const PIXEL_SIZES: u32 = 0x004c_0b84;
    const ATTACHMENT_BYTES: u32 = 0x004c_0ed6;
    const LIST_ONE: u32 = items::LISTS + 0x18;
    const LIST_TWO: u32 = items::LISTS + 0x30;
    const ACTIVE_BSP: u32 = 0x004e_0348;
    const PART_PLANES: u32 = 0x0051_f400;

    const LIGHT_VIS: u32 = engine::render::light_gather::LIGHT_VIS;
    const BSP: u32 = 0x0210_0000;
    const CLUSTERS: u32 = BSP + 0x1000;
    const GEOMETRY: u32 = BSP + 0x2000;
    const PARTS: u32 = BSP + 0x2100;
    const MATERIALS: u32 = BSP + 0x3000;
    const PLANES: u32 = BSP + 0x4000;
    const TABLE: u32 = 0x0211_0000;
    const SHADER: u32 = 0xe100_0003;
    const TEMPLATE: u32 = 0xe200_0004;
    const SHADER_DATA: u32 = 0x0212_0000;
    const INSTANCE: u32 = 0x0212_1000;
    const TEMPLATE_DATA: u32 = 0x0212_2000;
    const LEVELS_HEADER: u32 = 0x0212_3000;
    const LEVELS: u32 = 0x0212_3100;
    const LAYERS: u32 = (1 << 12) | (1 << 14);
    const ITEMS_ONE: u32 = 0x0213_0000;
    const ITEMS_TWO: u32 = 0x0213_1000;
    const RECEIVER: u32 = 1 | 2;
    const CASTER: u32 = 1 | 4;
    const FIRST_PERSON: u32 = 0x400;

    /// A light's records from index `main` (the main view's end), each
    /// with `flags`: structure records of cluster 0, whose one part's
    /// shader has one level.
    fn world(kind: u32, main: u32, flags: &[u32]) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(ENTRY_ESP - 0x200, 0x210);
        g.map(0x0045_d000, 0x1000)
            .map(0x0046_7000, 0x1000)
            .map(0x004b_a000, 0x1000);
        g.map(frame::FIRST_PERSON_TABLE, 8 * frame::FIRST_PERSON_SLOT);
        g.map(frame::FIRST_PERSON_COUNT, 4);
        g.map(RECORDS, 0x352 * 0x20)
            .map(PIXEL_SIZES, 0x352)
            .map(ATTACHMENT_BYTES, 0x352);
        g.map(items::LISTS, 0x48);
        g.put_u32(CURRENT_VIS, LIGHT_VIS)
            .put_u32(LIGHT_VIS + 0x2a78, kind);
        g.put_u32(MAIN_COUNT, main)
            .put_u32(COUNT, main + flags.len() as u32);
        for (header, items) in [(LIST_ONE, ITEMS_ONE), (LIST_TWO, ITEMS_TWO)] {
            g.put_u32(header, 8)
                .put_u32(header + 4, 5)
                .put_u32(header + 8, 0xdead)
                .put_u32(header + 0x14, items);
            g.map(items, 8 * 0x18);
        }
        g.put_f32(engine::light_constants::constant::ONE, 1.0);
        g.put_f32(0x0046_70e4, 1.0).put_f32(0x0045_dc10, 1.0);
        // The structure: cluster 0's section, one part of kind 1.
        g.put_u32(ACTIVE_BSP, BSP)
            .put_u32(BSP + 0xa0, CLUSTERS)
            .put_u32(BSP + 0xa8, MATERIALS);
        g.put_u32(CLUSTERS + 0x50, GEOMETRY);
        g.put_u32(GEOMETRY, 1).put_u32(GEOMETRY + 4, PARTS);
        g.put_u16(PARTS, 1).put_u16(PARTS + 4, 0);
        g.put_u8(MATERIALS + 0x1c, 0xff)
            .put_u32(MATERIALS + 0xc, SHADER);
        g.put_u32(PART_PLANES, PLANES).put_u32(PLANES + 0x20, 16);
        g.put_u32(PLANES + 0x2c, 0x0001_e8c0)
            .put_u32(PLANES + 0x30, 0x0001_e8f0);
        // The shader: classification 5, one level.
        g.put_u32(engine::tags::TAG_ENTRIES_GLOBAL, TABLE);
        g.put_u32(TABLE + (SHADER & 0xffff) * 16, u32::from_be_bytes(*b"shad"));
        g.put_u32(TABLE + (SHADER & 0xffff) * 16 + 8, SHADER_DATA);
        g.put_u16(SHADER_DATA + 0x3c, 5)
            .put_u32(SHADER_DATA + 0x24, INSTANCE);
        g.put_u32(INSTANCE, TEMPLATE);
        g.put_u32(TABLE + (TEMPLATE & 0xffff) * 16 + 8, TEMPLATE_DATA);
        g.put_u32(TEMPLATE_DATA + 0x5c, LEVELS_HEADER);
        g.put_u32(LEVELS_HEADER, 1)
            .put_u32(LEVELS_HEADER + 4, LEVELS);
        g.put_u32(LEVELS + 2, LAYERS);
        for (n, &f) in flags.iter().enumerate() {
            let i = main + n as u32;
            let r = RECORDS + i * 0x20;
            g.put_u32(r, f).put_u32(r + 4, u32::MAX);
            g.put_u32(r + 0xc, (0x7ff << 18) | (0x1f << 4));
            g.put_u8(r + 0x1a, 0xff);
            g.put_u8(PIXEL_SIZES + i, 10 + n as u8);
            g.put_u8(ATTACHMENT_BYTES + i, 0xff);
            // A first-person record's bytes, to see them copied.
            g.put_u32(r + 0x1c, 0x5100 + i);
        }
        g
    }

    struct Ran {
        run: testing::Run,
        items: Option<LightItems>,
    }

    fn run_items(
        g: FakeGuest,
        callee: impl FnMut(&GuestCall, &mut dyn GuestMemory) -> Regs,
    ) -> Ran {
        let plan = items::plan(&g, ENTRY_ESP, 0)
            .expect("the plan")
            .expect("a light's kind");
        let out = Arc::new(Mutex::new(None));
        let kept = Arc::clone(&out);
        let f = move |cx: Cx| -> EntryFuture {
            let plan = plan.clone();
            let kept = Arc::clone(&kept);
            Box::pin(async move {
                let (ret, light_items) = items::run(&cx, plan).await?;
                *kept.lock().unwrap() = Some(light_items);
                Ok::<_, engine::entry::EntryError>(ret)
            })
        };
        let regs = Regs {
            esp: ENTRY_ESP,
            ..Regs::default()
        };
        let run = testing::run(&f, g, regs, callee);
        let items = out.lock().unwrap().take();
        Ran { run, items }
    }

    fn item(g: &FakeGuest, items: u32, n: u32) -> (u32, u32, u32, u32, u32, u8, u8) {
        let at = items + n * 0x18;
        (
            g.u32(at).unwrap(),
            g.u32(at + 4).unwrap(),
            g.u32(at + 8).unwrap(),
            g.u32(at + 0xc).unwrap(),
            g.u32(at + 0x10).unwrap(),
            g.u8(at + 0x14).unwrap(),
            g.u8(at + 0x15).unwrap(),
        )
    }

    #[test]
    fn a_receiver_goes_to_list_one_and_a_caster_to_list_two() {
        let ran = run_items(
            world(1, 4, &[RECEIVER, CASTER, RECEIVER | CASTER]),
            |_, _| Regs::default(),
        );
        assert_eq!(ran.run.result, Ok(Ret::new(7, 0)), "EAX the walk's end");
        let targets: Vec<u32> = ran.run.calls.iter().map(|c| c.target).collect();
        assert_eq!(targets, [call::ATTACHMENTS], "every record made in Rust");
        let walk = &ran.run.calls[0];
        assert_eq!(
            (walk.edi, walk.ebx, walk.reserve),
            (Some(LIGHT_VIS), Some(1), stack::ATTACHMENTS)
        );
        let g = &ran.run.guest;
        let made = |i: u32| {
            (
                0x0004_cbf0,
                0x0004_d0b0,
                SHADER,
                ((10 + (i - 4)) << 24) | (i << 8),
                LAYERS,
                0,
                0,
            )
        };
        // List 1: the receiver, then the record with both bits.
        assert_eq!(item(g, ITEMS_ONE, 0), made(4));
        assert_eq!(item(g, ITEMS_ONE, 1), made(6));
        // List 2: the caster, then the record with both bits.
        assert_eq!(item(g, ITEMS_TWO, 0), made(5));
        assert_eq!(item(g, ITEMS_TWO, 1), made(6));
        for header in [LIST_ONE, LIST_TWO] {
            assert_eq!(g.u32(header + 4).unwrap(), 2, "zeroed, then two appended");
            assert_eq!(g.u32(header + 8).unwrap(), LAYERS);
            assert_eq!(g.u32(header + 0xc).unwrap(), 0);
        }
        let light_items = ran.items.unwrap();
        assert_eq!(light_items.records, 4..7);
        assert_eq!(
            light_items.made,
            [(4, Made::Rust), (5, Made::Rust), (6, Made::Rust)]
        );
        assert_eq!(
            (light_items.receivers.count, light_items.casters.count),
            (2, 2)
        );

        // In kind 3 a record's caster bit picks no list.
        let ran = run_items(world(3, 0, &[CASTER, RECEIVER | CASTER]), |_, _| {
            Regs::default()
        });
        let g = &ran.run.guest;
        assert_eq!(g.u32(LIST_TWO + 4).unwrap(), 0);
        assert_eq!(g.u32(LIST_ONE + 4).unwrap(), 1);
        assert_eq!(item(g, ITEMS_ONE, 0).3, (11 << 24) | (1 << 8));
    }

    #[test]
    fn a_first_person_record_is_copied_to_the_current_slot() {
        let mut g = world(1, 2, &[FIRST_PERSON, FIRST_PERSON]);
        g.put_u32(frame::FIRST_PERSON_COUNT, 3);
        let ran = run_items(g, |_, _| Regs::default());
        let g = &ran.run.guest;
        let slot = frame::FIRST_PERSON_TABLE + 2 * frame::FIRST_PERSON_SLOT;
        assert_eq!(g.u16(slot + 0x404).unwrap(), 2);
        assert_eq!(
            g.u32(slot).unwrap(),
            FIRST_PERSON,
            "the record whole: its flags"
        );
        assert_eq!(g.u32(slot + 0x1c).unwrap(), 0x5102);
        assert_eq!(g.u32(slot + 0x20 + 0x1c).unwrap(), 0x5103);
        let light_items = ran.items.unwrap();
        assert_eq!(
            light_items.first_person,
            [
                FirstPerson::Copied { slot: 2, index: 0 },
                FirstPerson::Copied { slot: 2, index: 1 }
            ]
        );
        assert!(
            light_items.made.is_empty(),
            "a first-person record has no items"
        );
        assert_eq!(g.u32(LIST_ONE + 4).unwrap(), 0);
    }

    #[test]
    fn records_past_the_eighth_light_join_the_last_slot() {
        // The frame body stopped counting at eight lights; a ninth
        // light's records go to the eighth's slot after its own.
        let mut g = world(1, 0, &[FIRST_PERSON]);
        g.put_u32(frame::FIRST_PERSON_COUNT, 8);
        let last = frame::FIRST_PERSON_TABLE + 7 * frame::FIRST_PERSON_SLOT;
        g.put_u16(last + 0x404, 3);
        let ran = run_items(g, |_, _| Regs::default());
        let g = &ran.run.guest;
        assert_eq!(g.u16(last + 0x404).unwrap(), 4);
        assert_eq!(g.u32(last + 3 * 0x20 + 0x1c).unwrap(), 0x5100);
        assert_eq!(
            ran.items.unwrap().first_person,
            [FirstPerson::Copied { slot: 7, index: 3 }]
        );
    }

    #[test]
    fn a_full_slot_drops_the_record() {
        let mut g = world(1, 0, &[FIRST_PERSON]);
        g.put_u32(frame::FIRST_PERSON_COUNT, 1);
        g.put_u16(frame::FIRST_PERSON_TABLE + 0x404, 32);
        let before = g
            .bytes(
                frame::FIRST_PERSON_TABLE,
                8 * frame::FIRST_PERSON_SLOT as usize,
            )
            .unwrap();
        let ran = run_items(g, |_, _| Regs::default());
        let g = &ran.run.guest;
        assert_eq!(
            g.bytes(
                frame::FIRST_PERSON_TABLE,
                8 * frame::FIRST_PERSON_SLOT as usize
            )
            .unwrap(),
            before
        );
        assert_eq!(
            ran.items.unwrap().first_person,
            [FirstPerson::Full { slot: 0 }]
        );
    }

    #[test]
    fn a_record_the_port_does_not_take_is_made_by_the_guests_part_calls() {
        // An object's record: its parts stay the guest's.
        let mut g = world(1, 0, &[RECEIVER | 0x40]);
        g.put_u32(RECORDS + 4, 0xe300_0001)
            .put_u32(RECORDS + 0x14, 0x0214_0000);
        const OBJECT_GEOMETRY: u32 = 0x0215_0000;
        g.put_u32(OBJECT_GEOMETRY, 1);
        let scratch = ENTRY_ESP - stack::SCRATCH;
        let ran = run_items(g, |c, m| match c.target {
            call::RECORD_GEOMETRY => Regs {
                eax: OBJECT_GEOMETRY,
                ..Regs::default()
            },
            call::PART_MATERIAL => {
                m.set_u32(c.eax.unwrap(), SHADER).unwrap();
                Regs {
                    eax: 1,
                    ..Regs::default()
                }
            }
            call::PART_VISIBLE => Regs {
                eax: 0xffff_ff01,
                ..Regs::default()
            },
            _ => Regs::default(),
        });
        assert_eq!(ran.run.result, Ok(Ret::new(1, 0)));
        let calls = &ran.run.calls;
        let targets: Vec<u32> = calls.iter().map(|c| c.target).collect();
        assert_eq!(
            targets,
            [
                call::ATTACHMENTS,
                call::RECORD_GEOMETRY,
                call::PART_MATERIAL,
                call::PART_VISIBLE,
                call::PART_ITEMS
            ]
        );
        for c in &calls[1..] {
            assert_eq!(
                c.reserve,
                stack::RECORDS,
                "{:#x} below the record walk's frame",
                c.target
            );
        }
        assert_eq!(
            (calls[1].eax, calls[1].args.as_slice()),
            (Some(0), &[0][..])
        );
        assert_eq!(
            (calls[2].ecx, calls[2].eax, calls[2].args.as_slice()),
            (Some(0), Some(scratch), &[0, 0][..])
        );
        assert_eq!(
            (calls[3].eax, calls[3].ebx, calls[3].args.as_slice()),
            (Some(0), Some(0x0214_0000), &[0][..])
        );
        assert_eq!(
            (calls[4].ecx, calls[4].edx, calls[4].args.as_slice()),
            (
                Some(0),
                Some(0),
                &[1, 10, scratch, SHADER, 0xffff_ff00, u32::MAX, 0][..]
            )
        );
        assert_eq!(
            ran.items.unwrap().made,
            [(0, Made::Guest(Refusal::Unsupported))]
        );
    }

    #[test]
    fn the_main_views_kind_is_not_the_bodys() {
        let g = world(0, 0, &[RECEIVER]);
        assert_eq!(items::plan(&g, ENTRY_ESP, 0), Ok(None));
        // A stack that is not there refuses before any write.
        let g = world(1, 0, &[RECEIVER]);
        assert_eq!(items::plan(&g, 0x0700_0000, 0).err(), Some(Refusal::Memory));
    }
}

/// Slice L3: with the records written for the light's struct in its
/// kind, the cluster and placement walks are made in Rust and the reset
/// and the object records stay guest calls.
#[test]
fn a_lights_walks_are_made_in_rust_in_its_kind() {
    use engine::render::light_gather::LIGHT_VIS;
    let lit = Lit {
        weight: 1.0,
        flags: 0,
        owner: None,
    };
    let mut g = world(&[lit]);
    const BSP: u32 = 0x020b_0000;
    const EMPTY: u32 = 0x020c_0000;
    g.put_u32(RECORDS_VIS, LIGHT_VIS)
        .put_u32(LIGHT_VIS + 0x2a78, 1)
        .put_u32(LIGHT_VIS + 0xc, EMPTY)
        .put_u32(LIGHT_VIS + 0x18, EMPTY);
    g.put_u32(EMPTY, 4).put_u16(EMPTY + 4, 0);
    g.put_u32(0x004e_0348, BSP);
    let ran = run_frame(g);
    assert_eq!(ran.run.result, Ok(Ret::new(0, 0)));
    let targets: Vec<u32> = ran.run.calls.iter().map(|c| c.target).collect();
    assert!(targets.contains(&call::RESET_RECORDS));
    assert!(targets.contains(&call::OBJECT_RECORDS));
    assert!(!targets.contains(&call::CLUSTER_RECORDS), "{targets:x?}");
    assert!(!targets.contains(&call::PLACEMENT_RECORDS), "{targets:x?}");
    let gathered = ran.frame.unwrap().lights[0].gathered.clone().unwrap();
    assert_eq!(
        (gathered.clusters, gathered.placements),
        (frame::By::Rust, frame::By::Rust)
    );

    // The main view's struct is not a light's: both walks go to the guest.
    let ran = run_frame(world(&[lit]));
    let gathered = ran.frame.unwrap().lights[0].gathered.clone().unwrap();
    assert_eq!(gathered.clusters, frame::By::Guest(Refusal::Layout));
}
