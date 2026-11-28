//! Serial oracle for two synthetic implementations of one shader. Both
//! implementations write the same stage, texture slot and constant register.
//! They write different lanes of the constant and leave a second stage alone.

use super::{OpaqueDraw, OpaqueSubmitError, opaque_handler, opaque_material};
use guest_layout::entry::{self, Cx, EntryFuture, GuestCall, Regs, Ret};
use guest_layout::memory::GuestMemory;
use engine::render_loop::states;
use guest_layout::testing::FakeGuest;
use native_render::skip::RENDER_RECORDS;
use native_render::world::Owner;
use native_render::world_ranges::SortedPart;

const BASE: u32 = 0x0040_0000;
const SIZE: u32 = 0x0018_0000;
const CACHE: u32 = 0x0048_5b48;
const STAGE: u32 = 0x0040_4dd0;
const WANT: u32 = 0x0051_f3d8;
const VALUES: u32 = 0x0048_5a64;
const TABLE: u32 = 0x0054_0000;
const SHADER: u32 = TABLE + 0x100;
const INSTANCE: u32 = TABLE + 0x200;
const TEMPLATE_TAG: u32 = TABLE + 0x300;
const TEMPLATE: u32 = TABLE + 0x400;
const LOD: u32 = TABLE + 0x500;
const LAYERS: u32 = TABLE + 0x540;
const PASS: u32 = TABLE + 0x600;
const PASS_TAG: u32 = TABLE + 0x640;
const DEFINITION: u32 = TABLE + 0x700;
const IMPLEMENTATIONS: u32 = TABLE + 0x1000;
const BINDINGS: u32 = TABLE + 0x1400;
const RECORDS: u32 = TABLE + 0x1500;
const VECTORS: u32 = TABLE + 0x1600;
const SOURCES: u32 = TABLE + 0x1700;
const SAMPLERS: u32 = TABLE + 0x1800;
const FIXED: u32 = TABLE + 0x1900;
const PART: u32 = TABLE + 0x1a00;
const HEADERS: u32 = TABLE + 0x1b00;
const DEVICE: u32 = TABLE + 0x2000;
const SHADER_DATUM: u32 = 0x1234_0001;
const VECTOR: u32 = CACHE + 0x1630; // vertex bank, register zero
const SLOT: u32 = CACHE + 0x320 + 2 * 64;
const PS: u32 = CACHE + 0x1424;
const FCW: u16 = 0x037f;
const MXCSR: u32 = 0x1f80;

// Synthetic register file behind the test's D3D setters.
const DEVICE_TEXTURE: u32 = DEVICE + 0x100;
const DEVICE_VECTOR: u32 = DEVICE + 0x200;
const DEVICE_BUMP: u32 = DEVICE + 0x300;
const DEVICE_PS: u32 = DEVICE + 0x400;
const DRAWS: u32 = DEVICE + 0x600;
const INTERNAL_REF: u32 = 1 << 19;
const LAST_USE: u32 = 43;

fn words(m: &mut dyn GuestMemory, at: u32, values: &[u32]) {
    for (i, v) in values.iter().copied().enumerate() {
        m.set_u32(at + i as u32 * 4, v).unwrap();
    }
}

fn scene() -> FakeGuest {
    let mut m = FakeGuest::new();
    m.map(BASE, SIZE);
    for (at, value) in [
        (guest_layout::tags::TAG_ENTRIES_GLOBAL, TABLE),
        (TABLE + 16, u32::from_be_bytes(*b"shad")), (TABLE + 24, SHADER),
        (TABLE + 40, TEMPLATE_TAG), (TABLE + 56, PASS_TAG),
        (SHADER + 4, u32::MAX), (SHADER + 0x24, INSTANCE),
        (INSTANCE, 2), (INSTANCE + 8, SOURCES), (INSTANCE + 0x18, VECTORS),
        (TEMPLATE_TAG + 0x5c, TEMPLATE), (TEMPLATE + 4, LOD),
        (TEMPLATE + 12, LAYERS), (TEMPLATE + 20, PASS), (TEMPLATE + 28, BINDINGS),
        (TEMPLATE + 0x24, RECORDS), (PASS + 4, 3), (PASS_TAG + 0x20, DEFINITION),
        (DEFINITION + 4, IMPLEMENTATIONS), (DEFINITION + 0x14, FIXED),
        (DEFINITION + 0x1c, SAMPLERS), (RENDER_RECORDS + 4, u32::MAX),
        (RENDER_RECORDS + 8, u32::MAX), (states::DEVICE, DEVICE),
        (TABLE + 0x1e00 + 0x24, TABLE + 0x1f00), (PART + 8, 3),
        (0x0045_de10, 1f32.to_bits()), (0x0044_a04c, 1f32.to_bits()),
        (0x0045_dbd4, 7f32.to_bits()), (states::MASKS + 0x5f * 4, 0x40),
        (SHADER + 0x44, 0.75f32.to_bits()), (SHADER + 0x48, 0.5f32.to_bits()),
    ] {
        m.put_u32(at, value);
    }
    m.put_u8(RENDER_RECORDS + 0x1a, 0xff).put_u16(SHADER + 0x3e, 19).put_u16(SHADER + 0x40, 43);
    words(&mut m, VECTOR, &[11, 22, 33, 44]);
    // Sentinels in bytes that none of the three draws owns.
    m.put_u32(CACHE + 0x1515, 0x0055_aa70);
    m.put_u32(SLOT + 0x30, 0xc001_0001);
    m.put_u32(STAGE + 0x80 + 4, 0xabcd_1234);
    m.put_u32(WANT + 12, HEADERS + 24);
    m.put_u32(WANT - 16 + 12, HEADERS + 24);
    m.put_u32(DEVICE_TEXTURE + 12, HEADERS + 24);
    m.put_u32(HEADERS + 24, 1 + INTERNAL_REF); // external ref + bound stage 3
    for n in 0..2 {
        let imp = IMPLEMENTATIONS + 0x132 * n;
        m.put_u16(imp + 2, (512 + n) as u16).put_u16(imp + 4, (512 + n) as u16);
        let definition = std::array::from_fn::<_, 240, _>(|i| (i as u8).wrapping_mul(7) ^ n as u8);
        m.put(imp + 6, &definition);
        m.put_u16(BINDINGS + n * 6, (512 + 2 * n) as u16);
        m.put_u16(BINDINGS + n * 6 + 4, (513 + 2 * n) as u16);
        m.put_u8(RECORDS + n * 8, 2).put_u8(RECORDS + n * 8 + 3, n as u8);
        // The first implementation updates x; the interleaved guest updates y.
        m.put_u16(RECORDS + n * 8 + 4, (16 + 32 * n) as u16);
        m.put_u8(RECORDS + n * 8 + 7, n as u8);
        words(&mut m, SOURCES + n * 12, &[31 + n, 2 + n, 7 + n]);
        words(&mut m, VECTORS + n * 16, &[101 + n, 201 + n, 301 + n, 401 + n]);
        m.put_u8(FIXED + n * 5, 0x5f).put_u32(FIXED + n * 5 + 1, 54 + n);
        let sampler = SAMPLERS + n * 24;
        m.put_u32(sampler, 0x8765_4321).put(sampler + 4, &[4, 5, 6]);
        words(&mut m, sampler + 8, &[10 + n, 20 + n, 30 + n, 40 + n]);
        m.put_u32(HEADERS + n * 12, 1);
    }
    m
}

fn geometry(n: u32) -> SortedPart {
    SortedPart {
        sorted_slot: TABLE + 0x1c00, owner: Owner::Cluster(0), part: 0, record: 0,
        record_addr: RENDER_RECORDS, section: TABLE + 0x1d00, geometry: TABLE + 0x1e00,
        part_addr: PART, shader: SHADER_DATUM, layer_argument: 0, lod: 0, variant: 0,
        projected_size: 0, layer: 3, pass: 0, implementation: n as u8,
        program: IMPLEMENTATIONS + 0x132 * n, classification: 0, strip: false,
        index_buffer: TABLE + 0x1f00, indices: std::iter::once(0..3).collect(),
    }
}

struct Draw;

impl OpaqueDraw for Draw {
    fn bind_streams<'a>(&'a self, _: &'a Cx) -> super::OpaqueStreams<'a> {
        // The actual stream/device port is separately exercised in boot over
        // RecordingBackend. This fixture isolates the real material/handler.
        Box::pin(async { Ok(true) })
    }

    fn submit(self: Box<Self>, m: &mut dyn GuestMemory, _: &SortedPart) -> Result<(), OpaqueSubmitError> {
        let count = m.u32(DRAWS).unwrap();
        m.set_u32(DRAWS, count + 1).unwrap();
        Ok(())
    }
}

fn copy(m: &mut dyn GuestMemory, from: u32, to: u32, len: usize) {
    let mut buf = vec![0; len];
    m.read(from, &mut buf).unwrap();
    m.write(to, &buf).unwrap();
}

fn bind_texture(m: &mut dyn GuestMemory, stage: u32, resource: u32) {
    let old = m.u32(DEVICE_TEXTURE + 4 * stage).unwrap();
    if old != resource {
        if resource != 0 {
            let refs = m.u32(resource).unwrap();
            m.set_u32(resource, refs + INTERNAL_REF).unwrap();
        }
        if old != 0 {
            let refs = m.u32(old).unwrap();
            m.set_u32(old, refs - INTERNAL_REF).unwrap();
            m.set_u32(old + 8, LAST_USE).unwrap();
        }
        m.set_u32(DEVICE_TEXTURE + 4 * stage, resource).unwrap();
    }
}

fn setter(call: &GuestCall, m: &mut dyn GuestMemory) -> Regs {
    match call.target {
        0x003f_7230 => {
            assert_eq!(call.ecx, Some(18));
            copy(m, call.edx.unwrap(), DEVICE_VECTOR, 16);
        }
        0x003f_8320 => bind_texture(m, call.eax.unwrap(), call.args[0]),
        0x003f_a3d0 => {
            let handle = call.eax.unwrap();
            let definition = m.u32(handle + 8).unwrap();
            copy(m, definition, DEVICE_PS, 240);
        }
        0x003f_5ea0 | 0x003f_5ef0 => {
            assert_eq!(call.eax, Some(0));
            let offset = if call.target == 0x003f_5ea0 { 0 } else { 4 };
            m.set_u32(DEVICE_BUMP + offset, call.ebx.unwrap()).unwrap();
        }
        unknown => panic!("unexpected synthetic material call {unknown:#010x}"),
    }
    Regs::default()
}

fn native(mut m: FakeGuest, n: u32) -> FakeGuest {
    m.put_u32(WANT, HEADERS + n * 12);
    let run = entry::testing::run(&move |cx: Cx| -> EntryFuture {
        Box::pin(async move {
            let part = geometry(n);
            let material = cx.read(|m| opaque_material::prepare(m, &part, FCW, MXCSR)).unwrap();
            let outcome =
                opaque_handler::handoff(&cx, &part, Box::new(Draw), FCW, MXCSR, &material,
                    super::DisplayFade::Retail.into()).await?;
            assert_eq!(outcome, opaque_handler::Emission::Native(1));
            Ok(Ret::new(0, 28))
        })
    }, m, Regs { esp: TABLE + 0x3000, ..Regs::default() }, setter);
    assert_eq!(run.result, Ok(Ret::new(0, 28)));
    run.guest
}

// Serial observation for these two fixed shaders. Express the values they
// write directly, including the register lane inherited from the guest draw.
// No production material decoders or handoff functions generate the oracle.
fn serial(m: &mut FakeGuest, n: u32) {
    let imp = IMPLEMENTATIONS + 0x132 * n;
    let old = m.u32(CACHE).unwrap();
    words(m, CACHE, &[SHADER_DATUM, old, SHADER_DATUM, INSTANCE, TEMPLATE, LOD, PASS,
        BINDINGS + 6 * n, DEFINITION, imp, 1f32.to_bits(), 0]);
    words(m, VALUES, &[19, 43, 0.75f32.to_bits(), 0.5f32.to_bits()]);
    words(m, SLOT + 0x10, &[7f32.to_bits(), 7 + n, ((2 + n) as f32).to_bits()]);
    m.put_u32(SLOT, 31 + n);
    if n == 0 { m.put_u32(VECTOR, 101); } else { m.put_u32(VECTOR + 4, 202); }
    copy(m, VECTOR, DEVICE_VECTOR, 16);
    m.put_u32(states::SHADOW + 0x5f * 4, 54 + n);
    m.put_u32(states::DIRTY, 0x41);
    words(m, STAGE, &[4, 5, 6, 3, 2, 4, 40 + n, 8, 7, 6, 30 + n, 5]);
    words(m, DEVICE_BUMP, &[10 + n, 20 + n]);
    m.put_u32(WANT, HEADERS + 12 * n);
    bind_texture(m, 0, HEADERS + 12 * n);
    m.put_u32(WANT - 16, HEADERS + 12 * n);
    copy(m, imp + 6, PS, 240);
    words(m, DEVICE + 0x924, &[1, 0, PS]);
    copy(m, PS, DEVICE_PS, 240);
    let count = m.u32(DRAWS).unwrap();
    m.put_u32(DRAWS, count + 1);
}

fn observe(reference: &FakeGuest, candidate: &FakeGuest) {
    let mut a = [0u8; 4096];
    let mut b = a;
    for page in (BASE..BASE + SIZE).step_by(a.len()) {
        reference.read(page, &mut a).unwrap();
        candidate.read(page, &mut b).unwrap();
        let first = a.iter().zip(b.iter()).position(|(a, b)| a != b);
        assert_eq!(first.map(|n| page + n as u32), None, "first differing guest/device address");
    }
}

#[test]
fn guest_observes_serial_device_and_shader_cache_between_native_handlers() {
    let mut reference = scene();
    let mut mixed = native(scene(), 0);
    serial(&mut reference, 0);
    observe(&reference, &mixed);
    serial(&mut reference, 1);
    serial(&mut mixed, 1);
    serial(&mut reference, 0);
    mixed = native(mixed, 0);
    observe(&reference, &mixed);
    assert_eq!(mixed.u32(DRAWS), Ok(3));
    assert_eq!(mixed.u32(DEVICE_VECTOR + 4), Ok(202));
    assert_eq!(mixed.u32(HEADERS), Ok(1 + INTERNAL_REF));
    assert_eq!(mixed.u32(HEADERS + 12), Ok(1));
    assert_eq!(mixed.u32(HEADERS + 20), Ok(LAST_USE));
}

const STACK: u32 = TABLE + 0x3000;
const SORTED: u32 = STACK + 0x2c;
const LIST: u32 = TABLE + 0x4000;
const ITEMS: u32 = TABLE + 0x4100;
const BSP: u32 = TABLE + 0x4200;
const SECTION: u32 = TABLE + 0x4300;
const GEOMETRY: u32 = TABLE + 0x4400;
const RECORD_COUNT: u32 = 0x004c_0b78;
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, GEOMETRY_DRAW, ITEM_SIZE};
use native_render::world_ranges::SortedRun;

fn sorted_scene(foreign: bool) -> FakeGuest {
    let mut m = scene();
    for (at, value) in [
        (super::ACTIVE_BSP, BSP), (CURRENT_LAYER, 3), (CURRENT_LIST, LIST),
        (RECORD_COUNT, 1), (BSP + 0x9c, 1), (BSP + 0xa0, SECTION), (SECTION + 0x50, GEOMETRY),
        (GEOMETRY, 1), (GEOMETRY + 4, PART), (GEOMETRY + 0x20, 3),
        (GEOMETRY + 0x24, TABLE + 0x4500), (LIST, 4), (LIST + 4, 4),
        (LIST + 0x14, ITEMS), (STACK + 0x28, 4), (WANT, HEADERS),
    ] {
        m.put_u32(at, value);
    }
    m.put_u16(PART + 8, 3);
    words(&mut m, STACK + 4, &[SHADER_DATUM, 0, 0, 3, 0, 0, SORTED]);
    for i in 0..4 {
        let slot = SORTED + i * ITEM_SIZE;
        m.put_u16(slot, i as u16);
        // Slot 1 becomes ready in the first draw's callback.
        m.put_u32(slot + 16, if i == 1 { 0 } else { IMPLEMENTATIONS });
        let handler = if foreign && i == 2 { 0x1000 } else { GEOMETRY_DRAW };
        words(&mut m, ITEMS + i * ITEM_SIZE, &[GEOMETRY_BUILD, handler, SHADER_DATUM]);
    }
    m
}

fn finish_first(m: &mut dyn GuestMemory) {
    m.set_u32(SORTED + ITEM_SIZE + 16, IMPLEMENTATIONS + 0x132).unwrap();
    m.set_u8(SORTED + ITEM_SIZE + 4, 1).unwrap();
    m.set_u32(WANT, HEADERS + 12).unwrap();
}

struct SpanView;

impl super::OpaqueView for SpanView {
    fn prepare_opaque(&self, _: &mut dyn GuestMemory, part: &SortedPart) -> Result<Box<dyn OpaqueDraw>, &'static str> {
        if part.sorted_slot == SORTED + 2 * ITEM_SIZE {
            return Err("unsupported synthetic section");
        }
        assert!(part.sorted_slot <= SORTED + ITEM_SIZE, "crossed the interleaved entry");
        Ok(Box::new(SpanView))
    }
}

impl OpaqueDraw for SpanView {
    fn bind_streams<'a>(&'a self, _: &'a Cx) -> super::OpaqueStreams<'a> {
        Box::pin(async { Ok(true) })
    }

    fn submit(self: Box<Self>, m: &mut dyn GuestMemory, part: &SortedPart) -> Result<(), OpaqueSubmitError> {
        let slot = part.sorted_slot;
        Box::new(Draw).submit(m, part)?;
        if slot == SORTED { finish_first(m); }
        Ok(())
    }
}

#[test]
fn sorted_handoff_returns_last_consumed_esi_and_guest_counter_at_first_refusal() {
    for foreign in [true, false] {
        let mut reference = sorted_scene(foreign);
        let run = entry::testing::run(&|cx: Cx| -> EntryFuture {
            Box::pin(async move {
                let mut materials = opaque_material::SectionMaterials::default();
                let (run, current) = cx.write(|m| {
                    let run = SortedRun::read(&super::Mem(m), LIST, STACK, SORTED).unwrap();
                    let first = run.next(&super::Mem(m), BSP, 0).unwrap();
                    let current = super::opaque::prepare(m, &SpanView, &mut materials, first, FCW, MXCSR).unwrap();
                    (run, current)
                });
                super::opaque::handoff(&cx, current, &SpanView, &run, 0, BSP, &mut materials,
                    FCW, MXCSR, super::DisplayFade::Retail.into()).await
            })
        }, reference.clone(), Regs { esp: STACK, esi: SORTED, ..Regs::default() }, setter);

        serial(&mut reference, 0);
        finish_first(&mut reference);
        reference.put_u32(STACK + 28, SORTED + ITEM_SIZE).put_u32(STACK + 0x28, 3);
        serial(&mut reference, 1);
        assert_eq!(run.result, Ok(Ret::new(0, 28).with_esi(SORTED + ITEM_SIZE)));
        assert_eq!(run.guest.u32(STACK + 0x28), Ok(3));
        assert_eq!(run.guest.u32(DRAWS), Ok(2));
        // The caller's ordinary advance lands on slot 2 with two live slots.
        // Its incoming shader, sampler, constants and references match both
        // serial draws, including the mutation between them.
        observe(&reference, &run.guest);
    }
}
