//! The light frame body (`engine::render::light_gather::frame`, slice L2
//! of docs/graphics/native-light-gather.md) over `entry::testing::run`:
//! synthetic guest state, every guest call answered by a stub.

use std::sync::{Arc, Mutex};

use guest_layout::entry::{Cx, EntryFuture, GuestCall, Regs, Ret, testing};
use engine::light_constants::Fpu;
use guest_layout::memory::GuestMemory;
use engine::render::light_gather::frame::{self, LightFrame, call, site, slot};
use engine::render::light_gather::{History, HistoryRow, MAIN_VIS, Refusal, VisibleLight, Writes};
use guest_layout::testing::FakeGuest;

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
    g.put_u32(guest_layout::tags::TAG_ENTRIES_GLOBAL, TAG_TABLE);
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
            Ok::<_, guest_layout::entry::EntryError>(ret)
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
    // The shadow dword: the byte over stack bytes nothing in the frame
    // writes (zero here); the list's argument is pushed below it.
    let shadow = 0;
    assert_eq!(passes.args, [datum(0), r + 0x84, shadow, 1.0f32.to_bits()]);

    // The visible list is in the frame's rows, as the guest leaves it.
    let g = &ran.run.guest;
    assert_eq!(g.u32(S + slot::ROWS + 4).unwrap(), datum(0));
    assert_eq!(g.u32(S + slot::ROWS + 0x14 + 4).unwrap(), datum(1));
    assert_eq!(g.u32(S + slot::LIST_ARGUMENT).unwrap(), S + slot::ROWS);
    let light_frame = ran.frame.expect("the frame");
    assert_eq!(light_frame.lights.len(), 2);
    assert!(light_frame.lights[0].drawn && !light_frame.lights[1].drawn);
    let gathered = light_frame.lights[0].gathered.as_ref().unwrap();
    assert_eq!(gathered.repose, frame::Repose::Asked { marker: 0 });
    assert_eq!(gathered.volume, frame::By::Guest(Refusal::Layout));
    assert_eq!(gathered.slot, Some(0));
    assert!(!gathered.shadow && gathered.source_vis);
}

/// What a frame did, in order: a light handed over, or a guest call.
#[derive(Debug, PartialEq)]
enum Event {
    Handed(frame::Passes),
    Call(u32),
}

#[test]
fn each_light_is_handed_over_just_before_its_passes_with_their_call() {
    let lit = Lit {
        weight: 1.0,
        flags: 0,
        owner: None,
    };
    let g = world(&[lit, lit]);
    let plan = frame::plan(&g, ENTRY_ESP, FPU).expect("the plan");
    let events = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&events);
    let f = move |cx: Cx| -> EntryFuture {
        let plan = plan.clone();
        let kept = Arc::clone(&kept);
        Box::pin(async move {
            let mut hand = |g: &frame::Gathered| kept.lock().unwrap().push(Event::Handed(g.passes));
            let (ret, _) = frame::run_with(&cx, plan, &mut hand).await?;
            Ok::<_, guest_layout::entry::EntryError>(ret)
        })
    };
    let stubbed = Arc::clone(&events);
    let regs = Regs {
        esp: ENTRY_ESP,
        ..Regs::default()
    };
    let run = testing::run(&f, g, regs, |c: &GuestCall, _m: &mut dyn GuestMemory| {
        stubbed.lock().unwrap().push(Event::Call(c.target));
        match c.target {
            call::SCENE_VISIBILITY => Regs {
                eax: VIS,
                ..Regs::default()
            },
            _ => Regs::default(),
        }
    });
    assert_eq!(run.result, Ok(Ret::new(0, 0)));
    let events = events.lock().unwrap();
    let handed: Vec<frame::Passes> = events
        .iter()
        .filter_map(|e| match e {
            Event::Handed(p) => Some(*p),
            Event::Call(_) => None,
        })
        .collect();
    let calls: Vec<_> = run
        .calls
        .iter()
        .filter(|c| c.target == call::LIGHT_PASSES)
        .collect();
    assert_eq!((handed.len(), calls.len()), (2, 2), "one per drawn light");
    for (p, c) in handed.iter().zip(&calls) {
        assert_eq!(c.eax, Some(p.definition), "EAX the ligh datum");
        assert_eq!(c.ecx, Some(p.context), "ECX the posed volume");
        assert_eq!(c.args, p.args(), "the four stack words");
        assert_eq!(p.context, S + slot::VOLUME);
        assert_eq!(p.position, record(p.light & 0xffff) + 0x84);
        assert_eq!(p.weight, 1.0f32.to_bits());
    }
    for (i, e) in events.iter().enumerate() {
        if matches!(e, Event::Handed(_)) {
            assert_eq!(
                events.get(i + 1),
                Some(&Event::Call(call::LIGHT_PASSES)),
                "no guest call between a hand-off and its passes"
            );
        }
    }
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
        g.put_u32(guest_layout::tags::TAG_ENTRIES_GLOBAL, TABLE);
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
                Ok::<_, guest_layout::entry::EntryError>(ret)
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

// ---------------------------------------------------------------------
// L5: one light's passes (`engine::render::light_gather::passes`).

mod passes_body {
    use super::*;
    use engine::render::light_gather::passes::{self, LightPasses, call, site};

    const E: u32 = 0x0010_0000;
    const TABLE: u32 = 0x0206_0000;
    const DATA: u32 = 0x0207_0000;
    const CONTEXT: u32 = 0x0210_0000;
    const DEFINITION: u32 = 0xa000_0003;
    const POSITION: u32 = 0x0205_0084;
    const LIGHT: u32 = 0x0001_0000;
    const PUSHED: u32 = 0x1234_5678;
    const LIGHT_MASK: u32 = 0x004c_1a68;
    const CASTER_MASK: u32 = 0x004c_1a80;
    const PASS_FLAGS: u32 = 0x004b_a014;
    const PASSES: u32 = 0x004b_a018;
    const VIEW_TARGET: u32 = 0x0048_58b8;

    /// A light whose constants left colour B bright, in a context that is
    /// not a sphere, with the light's list's `receivers` and the casters'
    /// `casters` masks as its item lists made them.
    fn guest(shadow: u8, receivers: u32, casters: u32) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(E - 0x2000, 0x2020);
        g.put_u32(E + 4, LIGHT)
            .put_u32(E + 8, POSITION)
            .put_u32(E + 0xc, 0x0000_ab00 | u32::from(shadow))
            .put_u32(E + 0x10, 0.5f32.to_bits());
        g.put_u32(guest_layout::tags::TAG_ENTRIES_GLOBAL, TABLE)
            .put_u32(TABLE + (DEFINITION & 0xffff) * 16 + 8, DATA);
        g.put_u8(DATA, 0)
            .put_u16(DATA + 4, 1)
            .put_u16(DATA + 0x14, 0);
        g.put_u16(CONTEXT, 1);
        g.put_u32(0x0003_11ba, PUSHED);
        g.put_u32(0x0045_dc7c, 0);
        g.map(0x0046_7130, 0x10);
        g.put_u32(VIEW_TARGET, 0x0000_3407);
        g.put_f32(0x0048_5950, 0.5).put_f32(0x0048_5a40, 1.0);
        g.map(0x0048_58b4, 0x10);
        g.map(0x004b_843c, 4);
        g.put_u32(PASS_FLAGS, 0x55).put_u8(PASSES, 0);
        g.put_u32(0x004b_a134, 0);
        g.put_u32(LIGHT_MASK, receivers)
            .put_u32(CASTER_MASK, casters);
        g.put_u32(0x0051_0c50, 0);
        g
    }

    struct Ran {
        run: testing::Run,
        passes: Option<LightPasses>,
        /// Each call's target and the return site its slot names.
        sites: Vec<(u32, Option<u32>)>,
    }

    fn run_passes(g: FakeGuest) -> Ran {
        let plan = passes::plan(&g, E, DEFINITION, CONTEXT, FPU).expect("the plan");
        let out = Arc::new(Mutex::new(None));
        let kept = Arc::clone(&out);
        let f = move |cx: Cx| -> EntryFuture {
            let plan = plan.clone();
            let kept = Arc::clone(&kept);
            Box::pin(async move {
                let (ret, light) = passes::run(&cx, plan).await?;
                *kept.lock().unwrap() = Some(light);
                Ok::<_, guest_layout::entry::EntryError>(ret)
            })
        };
        let mut sites = Vec::new();
        let regs = Regs {
            esp: E,
            ..Regs::default()
        };
        let run = testing::run(&f, g, regs, |c: &GuestCall, _m: &mut dyn GuestMemory| {
            sites.push((c.target, passes::return_site(c.esp(E))));
            match c.target {
                call::LAYER_BEGIN => Regs {
                    eax: 0xffff_ff01,
                    ..Regs::default()
                },
                call::TARGET => Regs {
                    eax: 0x77,
                    ..Regs::default()
                },
                _ => Regs::default(),
            }
        });
        let passes = out.lock().unwrap().take();
        Ran { run, passes, sites }
    }

    #[test]
    fn whole_pass_admission_uses_this_gather_and_refusal_continues_at_constants() {
        for accept in [false, true] {
            let g = guest(0, 1 << 14, 0);
            let plan = passes::plan(&g, E, DEFINITION, CONTEXT, FPU).unwrap();
            let body = move |cx: Cx| -> EntryFuture {
                let plan = plan.clone();
                Box::pin(async move {
                    let (ret, _) = passes::run_with(&cx, plan, |m, _| {
                        // ITEMS replaced the previous light's mask, but the pass
                        // has not changed its argument or installed any flags yet.
                        assert_eq!(m.u32(LIGHT_MASK)?, 1 << 12);
                        assert_eq!(m.u32(E + 0xc)?, 0xab00);
                        assert_eq!(m.u32(PASS_FLAGS)?, 0x55);
                        Ok(accept.then(|| (Ret::new(7, 16), LightPasses::default())))
                    }).await?;
                    Ok(ret)
                })
            };
            let mut calls = Vec::new();
            let regs = Regs { esp: E, ..Regs::default() };
            let run = testing::run(&body, g, regs, |call, mem| {
                calls.push(call.target);
                if call.target == call::ITEMS {
                    mem.set_u32(LIGHT_MASK, 1 << 12).unwrap();
                }
                Regs::default()
            });
            if accept {
                assert_eq!(run.result, Ok(Ret::new(7, 16)));
                assert_eq!(calls, [call::ITEMS]);
            } else {
                assert_eq!(run.result, Ok(Ret::new(0, 16)));
                assert_eq!(calls[..2], [call::ITEMS, call::CONSTANTS]);
                assert_eq!(calls.iter().filter(|&&target| target == call::ITEMS).count(), 1);
                assert_eq!(calls.iter().filter(|&&target| target == call::LAYER_DRAW).count(), 1);
                assert_eq!(calls.last(), Some(&call::TARGET));
            }
        }
    }

    #[test]
    fn a_shadowed_lights_passes_call_the_guest_in_its_order() {
        let ran = run_passes(guest(1, 1 << 11 | 1 << 12, 1 << 10));
        assert_eq!(ran.run.result, Ok(Ret::new(0x77, 0x10)));
        let targets: Vec<u32> = ran.run.calls.iter().map(|c| c.target).collect();
        assert_eq!(
            targets,
            [
                call::ITEMS,
                call::CONSTANTS,
                call::SHADOW_SETUP,
                call::LAYER_BEGIN,
                call::LAYER_DRAW,
                call::LAYER_END,
                // 11: in the list.
                call::TARGET,
                call::CLIP_MODE,
                call::RECEIVER_BLEND,
                call::STENCIL,
                call::LAYER_DRAW,
                call::LAYER_END,
                // 14: not in the list.
                call::STENCIL,
                // 12.
                call::TARGET,
                call::CLIP_MODE,
                call::STENCIL,
                call::LAYER_DRAW,
                call::LAYER_END,
                // No 13: not a sphere. The tail.
                call::STENCIL_ENABLE,
                call::SCISSORS,
                call::DEPTH_RANGE,
                call::TARGET,
            ]
        );
        for c in &ran.run.calls {
            assert_eq!(c.reserve, passes::FRAME, "{:#x}", c.target);
        }
        let named: Vec<Option<u32>> = ran.sites.iter().map(|s| s.1).collect();
        assert_eq!(named[0], Some(site::ITEMS));
        assert_eq!(named[4], Some(site::CASTER_DRAW));
        assert_eq!(named[10], Some(passes::RECEIVERS.draw));
        assert_eq!(named[16], Some(passes::DIFFUSE.draw));
        assert_eq!(named[21], Some(site::VIEW_TARGET));
        assert!(named.iter().all(Option::is_some));
        assert_eq!(
            passes::return_site(E - 0x14),
            None,
            "cleared after the last call"
        );

        let made = |i: usize| &ran.run.calls[i];
        assert_eq!(made(0).eax, Some(0), "the item lists' mode");
        let constants = made(1);
        assert_eq!(
            (constants.eax, constants.ecx, constants.edx),
            (Some(POSITION), Some(CONTEXT), Some(LIGHT))
        );
        assert_eq!(
            constants.args,
            [DATA, PUSHED, 0x0000_ab01, 0.5f32.to_bits()],
            "the shadow byte kept by the casters' layer 10"
        );
        assert_eq!((made(2).ecx, made(2).ebx), (Some(CONTEXT), Some(POSITION)));
        assert_eq!(made(2).args, [DATA]);
        assert_eq!(made(3).edi, Some(10));
        assert_eq!(
            made(6).eax,
            Some(0x0000_3401),
            "AL 1 over the view's target"
        );
        assert_eq!(made(6).args, [0x0000_3407, 0]);
        assert_eq!(made(9).ebx, Some(0), "the stencil off");
        assert_eq!(made(21).eax, Some(0x0000_3401));
        assert_eq!(made(21).args, [0x3407, 0]);

        let light = ran.passes.expect("what it did");
        let flags = 2 | 4 | 8 | 0x20;
        assert_eq!(
            light,
            LightPasses {
                shadow: true,
                flags,
                reached: 1 << 10 | 1 << 11 | 1 << 14 | 1 << 12,
                drawn: 1 << 10 | 1 << 11 | 1 << 12,
                additional_casters: false,
            }
        );
        let g = &ran.run.guest;
        assert_eq!(g.u32(PASS_FLAGS).unwrap(), 0, "zeroed by the tail");
        assert_eq!(g.u8(PASSES).unwrap(), 0);
        assert_eq!(g.u8(E + 0xc).unwrap(), 1, "the last layer's draw byte");
        assert_eq!(g.u32(0x0046_7130).unwrap(), 12, "the last layer word");
    }

    #[test]
    fn a_shadow_the_casters_do_not_take_draws_no_layer_10() {
        let ran = run_passes(guest(1, 1 << 12, 0));
        let light = ran.passes.unwrap();
        assert!(!light.shadow);
        assert_eq!(light.flags & 4, 0);
        assert!(
            !ran.run
                .calls
                .iter()
                .any(|c| c.target == call::SHADOW_SETUP || c.target == call::LAYER_BEGIN)
        );
        // Colour B still runs layer 11, which its list does not hold.
        assert_eq!(light.reached & 1 << 11, 1 << 11);
        assert_eq!(light.drawn, 1 << 12);
        assert_eq!(
            ran.run.calls[1].args[2], 0x0000_ab00,
            "the byte is the casters' bit"
        );
    }

    #[test]
    fn a_refused_plan_reads_only() {
        let g = guest(0, 0, 0);
        let flush = Fpu {
            mxcsr: 0x9f80,
            ..FPU
        };
        assert_eq!(
            passes::plan(&g, E, DEFINITION, CONTEXT, flush).err(),
            Some(Refusal::Unsupported)
        );
        assert_eq!(
            passes::plan(&g, E, DEFINITION, 0x0700_0000, FPU).err(),
            Some(Refusal::Memory),
            "a context that does not read"
        );
    }
}

// ---------------------------------------------------------------------
// L6: the first-person lights (`engine::render::light_gather::first_person`).

mod first_person_body {
    use super::*;
    use engine::render::light_gather::first_person::{
        self, DIFFUSE, FirstPersonLights, RECEIVERS, SlotRecords, call, records_slot, site,
    };
    use engine::render::light_gather::items::Made;
    use engine::render::light_gather::passes;

    const E: u32 = 0x0010_0000;
    const TABLE: u32 = frame::FIRST_PERSON_TABLE;
    const LIGHT: u32 = 0x0001_0002;
    const LIGHT_POOL: u32 = 0x0204_0000;
    const LIGHT_DATA: u32 = 0x0205_0000;
    const TAG_TABLE: u32 = 0x0206_0000;
    const DATA: u32 = 0x0207_0000;
    const DEFINITION: u32 = 0xa000_0003;
    const DEVICE: u32 = 0x0208_0000;
    const WEIGHT: u32 = 0x1111_1111;
    const SCALE: u32 = 0x2222_2222;
    const LIGHT_MASK: u32 = 0x004c_1a68;
    const PASS_FLAGS: u32 = 0x004b_a014;
    const PASSES: u32 = 0x004b_a018;
    const VIEW_TARGET: u32 = 0x0048_58b8;
    const MAIN_COUNT: u32 = 0x004c_0b7c;
    const COUNT: u32 = 0x004c_0b78;
    const SCENE_RECORDS: u32 = 0x004b_a138;
    const PIXEL_BYTES: u32 = 0x004c_0b84;
    const ATTACHMENT_BYTES: u32 = 0x004c_0ed6;
    const OVERRIDES: u32 = 0x004c_1228;
    const GEOMETRY: u32 = 0x0209_0000;

    fn record(i: u32) -> u32 {
        LIGHT_DATA + i * 0x110
    }

    /// One first-person light in slot 0 with `records` records, its
    /// list's mask `receivers`, colour B bright, and a light pool whose
    /// stride word is 0 so the volume's leaf refuses.
    fn guest(receivers: u32) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(E - 0x2000, 0x2020);
        g.put_u32(E + 4, 2);
        g.map(TABLE, 8 * frame::FIRST_PERSON_SLOT);
        g.put_u32(frame::FIRST_PERSON_COUNT, 1);
        g.put_u32(TABLE + 0x400, LIGHT).put_u16(TABLE + 0x404, 2);
        g.put_u32(engine::render::light::ARRAY_POINTER, LIGHT_POOL);
        g.put_u32(LIGHT_POOL + 0x44, LIGHT_DATA);
        g.map(LIGHT_DATA, 0x400);
        g.put_u32(record(2) + 4, DEFINITION);
        g.put_u32(guest_layout::tags::TAG_ENTRIES_GLOBAL, TAG_TABLE)
            .put_u32(TAG_TABLE + (DEFINITION & 0xffff) * 16 + 8, DATA);
        g.put_u32(0x0004_0566, WEIGHT).put_u32(0x0004_056d, SCALE);
        g.put_f32(0x0048_5950, 0.5);
        g.map(0x0048_5ad0, 0x20);
        g.map(0x0048_58b4, 0x10);
        g.put_u32(VIEW_TARGET, 0x0000_3407);
        g.map(0x0046_7130, 0x10);
        g.map(0x004b_843c, 4);
        g.put_u32(PASS_FLAGS, 0x55).put_u8(PASSES, 0);
        g.put_u32(LIGHT_MASK, receivers);
        g.put_u32(0x0040_7488, DEVICE)
            .put_u32(DEVICE, 0x100)
            .put_u32(DEVICE + 4, 0x200);
        g.map(DEVICE + 0x1c10, 12);
        g.put_u32(0x0040_8648, 0x40);
        g
    }

    struct Ran {
        run: testing::Run,
        lights: Option<FirstPersonLights>,
        /// Each call's target and the return site its slot names.
        sites: Vec<(u32, Option<u32>)>,
        /// At each layer draw: the loop's index and slot pointer in its
        /// frame, as `native_lights::first_person_entry` reads them.
        frames: Vec<(u32, u32)>,
    }

    fn run_lights(g: FakeGuest) -> Ran {
        let plan = first_person::plan(&g, E, FPU).expect("the plan");
        let out = Arc::new(Mutex::new(None));
        let kept = Arc::clone(&out);
        let f = move |cx: Cx| -> EntryFuture {
            let plan = plan.clone();
            let kept = Arc::clone(&kept);
            Box::pin(async move {
                let (ret, lights) = first_person::run(&cx, plan).await?;
                *kept.lock().unwrap() = Some(lights);
                Ok::<_, guest_layout::entry::EntryError>(ret)
            })
        };
        let mut sites = Vec::new();
        let mut frames = Vec::new();
        let regs = Regs {
            esp: E,
            ..Regs::default()
        };
        let run = testing::run(&f, g, regs, |c: &GuestCall, m: &mut dyn GuestMemory| {
            let esp = c.esp(E);
            sites.push((c.target, passes::return_site(esp)));
            if c.target == call::LAYER_DRAW {
                frames.push((
                    m.u32(esp + 4 + 0x14).unwrap(),
                    m.u32(esp + 4 + 0x18).unwrap(),
                ));
            }
            match c.target {
                call::PUSH_STATE => Regs {
                    eax: 0x1234_5600,
                    ..Regs::default()
                },
                _ => Regs::default(),
            }
        });
        let lights = out.lock().unwrap().take();
        Ran {
            run,
            lights,
            sites,
            frames,
        }
    }

    #[test]
    fn a_typed_slot_uses_its_replayed_lists_and_a_refused_slot_continues_at_constants() {
        let mut g = guest(0);
        g.put_u32(frame::FIRST_PERSON_COUNT, 2);
        let second = TABLE + frame::FIRST_PERSON_SLOT;
        g.put_u32(second + 0x400, LIGHT).put_u16(second + 0x404, 1);
        let plan = first_person::plan(&g, E, FPU).unwrap();
        let f = move |cx: Cx| -> EntryFuture {
            let plan = plan.clone();
            Box::pin(async move {
                let mut seen = 0;
                let (ret, lights) = first_person::run_with(&cx, plan, &mut |m, plan, call| {
                    let index = m.u32(plan.entry_esp - first_person::slot::INDEX)?;
                    assert_eq!(index, seen);
                    assert_eq!(call.light, LIGHT);
                    assert_eq!(call.frame, record(2) + 0x84);
                    assert_eq!(call.context, E - first_person::slot::VOLUME);
                    assert_eq!(call.definition, DATA);
                    assert_eq!(call.shadow, 0);
                    assert_eq!(call.weight, WEIGHT);
                    let mask = m.u32(LIGHT_MASK)?;
                    assert_eq!(mask, 1 << (12 + index));
                    seen += 1;
                    if index != 0 {
                        return Ok(None);
                    }
                    // The accepted packet transaction owns the slot's last draw byte.
                    m.set_u8(plan.draw_byte(), 1)?;
                    Ok(Some(first_person::SlotPasses {
                        flags: 0,
                        reached: (1 << 11) | (1 << 14) | (1 << 12),
                        drawn: mask,
                    }))
                })
                .await?;
                assert_eq!(seen, 2);
                assert_eq!(lights.lights.len(), 2);
                assert_eq!(lights.lights[0].drawn, 1 << 12);
                Ok(ret)
            })
        };
        let mut replayed = 0;
        let run = testing::run(
            &f,
            g,
            Regs {
                esp: E,
                ..Default::default()
            },
            |c, m| {
                if c.target == call::RECORDS {
                    m.set_u32(LIGHT_MASK, 1 << (12 + replayed)).unwrap();
                    replayed += 1;
                }
                if c.target == call::LIGHT_VOLUME && replayed == 1 {
                    assert_eq!(m.u8(E - first_person::slot::DRAWS).unwrap(), 1);
                }
                Regs::default()
            },
        );
        assert!(run.result.is_ok());
        let calls: Vec<_> = run.calls.iter().map(|c| c.target).collect();
        assert_eq!(
            &calls[..5],
            &[
                call::LIGHT_VOLUME,
                call::RECORDS,
                call::LIGHT_VOLUME,
                call::RECORDS,
                call::CONSTANTS
            ]
        );
        assert_eq!(
            calls
                .iter()
                .filter(|&&target| target == call::CONSTANTS)
                .count(),
            1
        );
        assert_eq!(
            calls
                .iter()
                .filter(|&&target| target == call::PUSH_STATE)
                .count(),
            1
        );
    }

    #[test]
    fn the_first_person_lights_call_the_guest_in_its_order() {
        let ran = run_lights(guest(1 << 11 | 1 << 12));
        assert_eq!(
            ran.run.result,
            Ok(Ret::new(((TABLE + 0x400 + 0x408) & !0xff) | 1, 4))
        );
        let targets: Vec<u32> = ran.run.calls.iter().map(|c| c.target).collect();
        assert_eq!(
            targets,
            [
                // The volume's leaf refused; its guest function.
                call::LIGHT_VOLUME,
                call::RECORDS,
                call::CONSTANTS,
                // 11: in the list.
                call::TARGET,
                call::CLIP_MODE,
                call::RECEIVER_BLEND,
                call::STENCIL,
                call::LAYER_DRAW,
                call::LAYER_END,
                // 14: not in the list.
                call::STENCIL,
                // 12.
                call::TARGET,
                call::CLIP_MODE,
                call::STENCIL,
                call::LAYER_DRAW,
                call::LAYER_END,
                // 13: a kind-0 volume and colour B, not in the list.
                call::STENCIL,
                // The tail; the push buffer has room.
                call::STENCIL_ENABLE,
                call::SCISSORS,
                call::PUSH_STATE,
                call::TARGET,
            ]
        );
        for c in &ran.run.calls {
            assert_eq!(c.reserve, first_person::FRAME, "{:#x}", c.target);
        }
        let named: Vec<Option<u32>> = ran.sites.iter().map(|s| s.1).collect();
        assert!(named.iter().all(Option::is_some), "{named:x?}");
        assert_eq!(named[1], Some(site::RECORDS));
        assert_eq!(named[7], Some(RECEIVERS.draw));
        assert_eq!(named[13], Some(DIFFUSE.draw));
        assert_eq!(named[19], Some(site::VIEW_TARGET));
        assert_eq!(passes::return_site(E - 0x9c), None, "cleared after");
        // The layer-draw hook finds the slot in the loop's frame.
        assert_eq!(ran.frames, [(0, TABLE + 0x400), (0, TABLE + 0x400)]);

        let made = |i: usize| &ran.run.calls[i];
        assert_eq!(
            (made(0).eax, made(0).edx, made(0).args.as_slice()),
            (Some(E - 0x7c), Some(LIGHT), &[1][..])
        );
        assert_eq!(
            (made(1).ecx, made(1).edx, made(1).args.as_slice()),
            (Some(2), Some(0), &[2][..]),
            "the mode and the slot"
        );
        let constants = made(2);
        assert_eq!(
            (constants.eax, constants.ecx, constants.edx),
            (Some(record(2) + 0x84), Some(E - 0x7c), Some(LIGHT))
        );
        assert_eq!(
            constants.args,
            [DATA, SCALE, 0, WEIGHT],
            "shadow 0, weight one"
        );
        assert_eq!(
            made(3).eax,
            Some(0x0000_3401),
            "AL 1 over the view's target"
        );
        assert_eq!(made(3).ebx, Some(DATA), "layer 11's begin keeps EBX");
        assert_eq!(made(6).ebx, Some(0), "the stencil off");
        assert_eq!(made(10).ebx, Some(0));
        assert_eq!(made(18).esi, Some(DEVICE), "the push, ESI the device");
        assert_eq!(made(19).eax, Some(0x1234_5601));
        assert_eq!(made(19).args, [0x3407, 0]);

        let lights = ran.lights.expect("what it did");
        assert_eq!(lights.count, 1);
        let light = lights.lights[0];
        assert_eq!((light.index, light.light), (0, LIGHT));
        assert_eq!(light.flags, 3, "a kind-0 volume and colour B");
        assert_eq!(light.reached, 1 << 11 | 1 << 14 | 1 << 12 | 1 << 13);
        assert_eq!(light.drawn, 1 << 11 | 1 << 12);
        let g = &ran.run.guest;
        assert_eq!(g.u32(PASS_FLAGS).unwrap(), 0, "zeroed by the tail");
        assert_eq!(g.u8(PASSES).unwrap(), 0);
        assert_eq!(g.u32(DEVICE).unwrap(), 0x1234_5600, "the put pointer");
        assert_eq!(g.u32(DEVICE + 0x1c18).unwrap(), 2, "the depth range dirty");
        assert_eq!(g.u32(E - 0x84).unwrap(), 1, "the loop's index");
    }

    #[test]
    fn a_full_push_buffer_asks_for_room_first() {
        let mut g = guest(0);
        g.put_u32(DEVICE, 0x300);
        let ran = run_lights(g);
        let targets: Vec<u32> = ran.run.calls.iter().map(|c| c.target).collect();
        let at = targets.iter().position(|&t| t == call::PUSH_SPACE).unwrap();
        assert_eq!(targets[at + 1], call::PUSH_STATE);
        let room = &ran.run.calls[at];
        assert_eq!(
            (room.eax, room.ecx, room.args.as_slice()),
            (Some(0x40), Some(0x20), &[0x20][..])
        );
        assert_eq!(ran.sites[at].1, Some(site::PUSH_SPACE));
    }

    #[test]
    fn no_slots_draw_nothing_and_more_than_eight_are_the_guests() {
        let mut g = guest(0);
        g.put_u32(frame::FIRST_PERSON_COUNT, 0);
        let ran = run_lights(g);
        assert!(ran.run.calls.is_empty());
        assert_eq!(ran.run.result, Ok(Ret::new(1, 4)));
        let mut g = guest(0);
        g.put_u32(frame::FIRST_PERSON_COUNT, 9);
        assert_eq!(
            first_person::plan(&g, E, FPU).err(),
            Some(Refusal::Unsupported)
        );
        let flush = Fpu {
            mxcsr: 0x9f80,
            ..FPU
        };
        assert_eq!(
            first_person::plan(&guest(0), E, flush).err(),
            Some(Refusal::Unsupported)
        );
    }

    /// Slot 0 holds one object's record (its parts are the guest's): the
    /// main view's records end at 5, where the replay copies it.
    fn records_guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(E - 0x2000, 0x2020);
        g.put_u32(E + 4, 0);
        g.map(TABLE, 8 * frame::FIRST_PERSON_SLOT);
        g.put_u16(TABLE + 0x404, 1);
        g.put_u32(TABLE, 0x0000_0003)
            .put_u32(TABLE + 4, 0x0001_0007)
            .put_u32(TABLE + 0xc, 0x1f0 | 0x5)
            .put_u32(TABLE + 0x14, 0x0300_0000)
            .put_u8(TABLE + 0x1a, 1);
        g.map(0x004c_1a40, 0x60);
        g.put_u32(MAIN_COUNT, 5).put_u32(COUNT, 5);
        g.map(SCENE_RECORDS, 0x20 * 8);
        // The bytes at the record's old and new index.
        g.put_u8(PIXEL_BYTES, 0x77).put_u8(PIXEL_BYTES + 5, 0x33);
        g.put_u8(ATTACHMENT_BYTES, 0xff)
            .put_u8(ATTACHMENT_BYTES + 5, 2);
        g.put_u32(OVERRIDES + 8, 0xe100_0009);
        // What the frame's pixel dword holds above its byte.
        g.put_u32(E - records_slot::PIXEL, 0xefcd_ab00);
        g.put_u32(GEOMETRY, 1);
        g
    }

    #[test]
    fn a_replayed_record_reads_the_bytes_at_its_new_index() {
        let g = records_guest();
        let plan = first_person::plan_records(&g, E, 0xabcd_0000, 0, FPU).expect("the plan");
        let out = Arc::new(Mutex::new(None));
        let kept = Arc::clone(&out);
        let f = move |cx: Cx| -> EntryFuture {
            let plan = plan.clone();
            let kept = Arc::clone(&kept);
            Box::pin(async move {
                let (ret, records) = first_person::run_records(&cx, plan).await?;
                *kept.lock().unwrap() = Some(records);
                Ok::<_, guest_layout::entry::EntryError>(ret)
            })
        };
        let regs = Regs {
            esp: E,
            ..Regs::default()
        };
        let run = testing::run(&f, g, regs, |c: &GuestCall, _m: &mut dyn GuestMemory| {
            use engine::render::light_gather::items::call as part;
            Regs {
                eax: match c.target {
                    part::RECORD_GEOMETRY => GEOMETRY,
                    part::PART_MATERIAL | part::PART_VISIBLE => 1,
                    _ => 0,
                },
                ..Regs::default()
            }
        });
        assert_eq!(run.result, Ok(Ret::new(1, 4)), "one record walked");
        let records: SlotRecords = out.lock().unwrap().take().unwrap();
        assert_eq!(records.records, 5..6);
        assert_eq!(records.made, [(5, Made::Guest(Refusal::Unsupported))]);
        let g = &run.guest;
        assert_eq!(g.u32(COUNT).unwrap(), 6, "the tail past the copy");
        assert_eq!(
            g.bytes(SCENE_RECORDS + 5 * 0x20, 0x20).unwrap(),
            g.bytes(TABLE, 0x20).unwrap(),
            "the record at its new index"
        );
        let items = run.calls.last().unwrap();
        assert_eq!(
            items.target,
            engine::render::light_gather::items::call::PART_ITEMS
        );
        assert_eq!((items.ecx, items.edx), (Some(0), Some(5)));
        assert_eq!(
            items.args,
            [
                1,
                // The pixel byte at index 5, over the frame's stale bytes.
                0xefcd_ab33,
                E - records_slot::SCRATCH,
                // The override of the attachment byte at index 5.
                0xe100_0009,
                1,
                u32::MAX,
                0
            ]
        );
        assert_eq!(run.calls[0].eax, Some(5), "the geometry of index 5");
        assert_eq!(g.u32(E + 4).unwrap(), TABLE + 0x20, "the slot cursor");
    }

    #[test]
    fn an_empty_slot_zeroes_the_lists_and_returns_its_count() {
        let mut g = records_guest();
        g.put_u16(TABLE + 0x404, 0);
        g.put_u32(0x004c_1a64, 9);
        let plan = first_person::plan_records(&g, E, 0xabcd_1234, 0, FPU).unwrap();
        let f = move |cx: Cx| -> EntryFuture {
            let plan = plan.clone();
            Box::pin(async move {
                Ok::<_, guest_layout::entry::EntryError>(first_person::run_records(&cx, plan).await?.0)
            })
        };
        let regs = Regs {
            esp: E,
            ..Regs::default()
        };
        let run = testing::run(&f, g, regs, |_: &GuestCall, _: &mut dyn GuestMemory| {
            Regs::default()
        });
        assert_eq!(run.result, Ok(Ret::new(0xabcd_0000, 4)));
        assert!(run.calls.is_empty());
        assert_eq!(run.guest.u32(0x004c_1a64).unwrap(), 0);
        assert_eq!(run.guest.u32(COUNT).unwrap(), 5, "no copy");
        assert_eq!(
            first_person::plan_records(&records_guest(), E, 0, 8, FPU).err(),
            Some(Refusal::Unsupported),
            "a slot past the table"
        );
    }
}
