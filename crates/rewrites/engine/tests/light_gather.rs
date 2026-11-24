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
