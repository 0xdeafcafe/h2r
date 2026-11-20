//! Compare both representations at the point a guest handler inherits
//! them: all of guest RAM and the actual D3D device's snapshot section.

use super::*;
use crate::native_lights_glue::DeviceMemory;
use d3d::{Device, DeviceConfig, GuestRange, RecordingBackend};
use engine::entry::{self, EntryFuture, Regs, Ret};
use engine::testing::FakeGuest;
use engine::memory::FlatMemory;
use snapshot::{BuildKey, Header, SnapshotWriter};

const SECTION: u32 = 0x0054_0000;
const GEOMETRY: u32 = SECTION + 0x100;
const IMPLEMENTATION: u32 = SECTION + 0x200;
const PART: u32 = SECTION + 0x380;
const TABLE: u32 = SECTION + 0x400;
const VERTEX: u32 = SECTION + 0x600;
const CLASS: u32 = SECTION + 0x800;
const SEMANTICS: u32 = SECTION + 0x900;
const FUNCTION: u32 = SECTION + 0xa00;
const DESCRIPTOR: u32 = SECTION + 0xb00;
const BUFFERS: u32 = SECTION + 0xc00;
const RECORD: u32 = SECTION + 0xd00;
const LIGHTMAP: u32 = SECTION + 0xe00;
const GROUP: u32 = SECTION + 0xf00;
const INFOS: u32 = SECTION + 0x1000;
const ACTIVE_LIGHTMAP: u32 = 0x004e_0344;
const PALETTES: u32 = 0x0048_4dd0;

type Scene = (Device<RecordingBackend>, FlatMemory);

fn scene() -> Scene {
    let push = GuestRange { addr: 0x0041_0000, len: 0x10000 };
    let scratch = GuestRange { addr: 0x0042_0000, len: 0x1000 };
    let config = DeviceConfig::halo2(push, scratch);
    let mut device = Device::new(config, RecordingBackend::default());
    let mut m = FlatMemory::new(0x0040_0000, 0x180000);
    for slot in 0..d3d::pushbuf::SIMPLE_ENCODE_SLOTS as u32 {
        m.set_u32(config.layout.simple_encode + 4 * slot, (1 << 18) | (4 * (slot + 1))).unwrap();
    }
    let params = scratch.addr + scratch.len;
    let mut dm = DeviceMemory(&mut m);
    d3d::PresentParameters {
        width: 32, height: 32, back_buffer_format: gfx::D3DFormat::A8R8G8B8.raw(),
        back_buffer_count: 1, multisample_type: 0, swap_effect: 0,
        enable_auto_depth_stencil: false, auto_depth_stencil_format: 0,
        flags: 0, refresh_rate: 0, presentation_interval: 0,
        buffer_surface: 0, depth_stencil_surface: 0,
    }.write(&mut dm, params).unwrap();
    device.create_device(&mut dm, 0, params, 0).unwrap();
    for (at, value) in [
        (engine::tags::TAG_ENTRIES_GLOBAL, TABLE), (SECTION + 0x50, GEOMETRY),
        (GEOMETRY + 0x38, 1), (GEOMETRY + 0x3c, DESCRIPTOR),
        (VERTEX + 4, 1), (VERTEX + 8, CLASS),
        (CLASS + 4, 1), (CLASS + 8, SEMANTICS), (CLASS + 0x10, FUNCTION),
        (FUNCTION, 1 << 16), (RECORD + 4, u32::MAX),
        (ACTIVE_LIGHTMAP, LIGHTMAP), (LIGHTMAP + 0x84, GROUP), (GROUP + 0x2c, INFOS),
        (world_lightmap::PALETTE_COUNT, 2), (world_lightmap::LAST_PALETTE, u32::MAX),
        (world_lightmap::STAGE_PALETTES + 12, u32::MAX),
    ] {
        m.set_u32(at, value).unwrap();
    }
    for n in 0..2 {
        m.set_u32(TABLE + 16 * (n + 1) + 8, VERTEX).unwrap();
        m.set_u32(BUFFERS + 12 * n, 1).unwrap(); // one external ref, initially unbound
        m.set_u32(PALETTES + 12 * n, 1).unwrap();
        m.set_u16(INFOS + 4 * n, (13 + n) as u16).unwrap();
        m.set_u8(INFOS + 4 * n + 2, n as u8).unwrap();
        let format = STREAM_FORMATS + n * STREAM_FORMAT_SIZE as u32;
        // One synthetic input for v0. The second declaration has an explicit
        // four-byte hole before its single float.
        if n == 0 {
            m.write(format + 1, &[0, 0, 0xff, 0]).unwrap();
        } else {
            m.write(format + 1, &[0xfe, 4, 0, 0, 0xff, 0]).unwrap();
        }
    }
    (device, m)
}

fn select(m: &mut FlatMemory, n: u32) {
    m.set_u32(IMPLEMENTATION + 0x100, n + 1).unwrap();
    m.set_u32(RECORD + 12, (0x7ff << 18) | (n << 9)).unwrap();
    m.set_u8(DESCRIPTOR, n as u8).unwrap();
    m.set_u8(DESCRIPTOR + 1, (4 + n * 4) as u8).unwrap();
    m.set_u32(DESCRIPTOR + 8, 16 * (n + 1)).unwrap();
    m.set_u32(DESCRIPTOR + 0x10, BUFFERS + 12 * n).unwrap();
    // No XBE program is needed to load and select an instruction.
    m.set_u32(FUNCTION + 4, 0x0101_0000 | n).unwrap();
}

fn native((device, mem): &mut Scene, n: u32) {
    select(mem, n);
    let plan = Plan::read(mem, SECTION, GEOMETRY, PART, IMPLEMENTATION, RECORD, 3, 0).unwrap();
    // Drive the production coroutine through the registered XDK ABI adapters.
    // The fixture's call frames live outside the rendered scene's address range.
    const STACK: u32 = 0x600000;
    let mut g = FakeGuest::new();
    g.put(mem.base, &mem.bytes).map(STACK, 4096);
    let entries = d3d::abi::entries::<RecordingBackend>();
    let run = entry::testing::run(&move |cx: Cx| -> EntryFuture {
        let plan = plan.clone();
        Box::pin(async move { Ok(Ret::new(u32::from(plan.handoff(&cx).await?), 0)) })
    }, g, Regs { esp: STACK, ..Regs::default() }, |call, m| {
        let setter = entries.iter().find(|e| e.addr == call.target).expect("registered D3D entry");
        for (i, arg) in call.args.iter().enumerate() {
            m.set_u32(STACK + 4 + 4 * i as u32, *arg).unwrap();
        }
        let regs = d3d::abi::Regs {
            esp: STACK, eax: call.eax.unwrap_or(0), ecx: call.ecx.unwrap_or(0),
            edx: call.edx.unwrap_or(0), ebx: call.ebx.unwrap_or(0),
            esi: call.esi.unwrap_or(0), edi: call.edi.unwrap_or(0),
        };
        let out = d3d::abi::call(setter, device, &mut DeviceMemory(m), &regs).unwrap();
        assert_eq!(out.cleanup, 4 * call.args.len() as u32, "{}", setter.name);
        Regs { eax: out.eax.unwrap_or(regs.eax), edx: out.edx.unwrap_or(regs.edx), ..Regs::default() }
    });
    assert_eq!(run.result, Ok(Ret::new(1, 0)));
    run.guest.read(mem.base, &mut mem.bytes).unwrap();
}

// The serial fixture has a program change, exactly one supplied v0 and
// exactly one enabled palette on every call. State comes from these literal
// declarations and the D3D entry points, independently of Plan and add_format.
fn serial((device, mem): &mut Scene, n: u32) {
    select(mem, n);
    device.set_palette(&mut DeviceMemory(mem), 3, PALETTES + n * 12).unwrap();
    mem.set_u32(world_lightmap::STAGE_PALETTES + 12, n).unwrap();
    mem.set_u32(world_lightmap::LAST_PALETTE, n).unwrap();
    mem.set_u32(world_lightmap::PAGE, 13 + n).unwrap();

    let shader = d3d::GuestLayout::HALO2.static_vertex_shader;
    device.load_vertex_shader_program(&mut DeviceMemory(mem), FUNCTION, 0).unwrap();
    let format = STREAM_FORMATS + n * STREAM_FORMAT_SIZE as u32;
    mem.set_u32(CACHE + 4, n + 1).unwrap();
    mem.set_u32(LAST_FORMATS, format).unwrap();
    mem.set_u32(INPUTS, BUFFERS + n * 12).unwrap();
    mem.set_u32(INPUTS + 4, 4 + 4 * n).unwrap();
    mem.set_u32(INPUTS + 8, 16 * (n + 1)).unwrap();
    let mut declaration = [0u8; 16 * 16];
    for v in declaration.chunks_exact_mut(16) {
        v[8] = 2;
    }
    declaration[4..8].copy_from_slice(&(4 * n).to_le_bytes());
    declaration[8] = 0x12; // FLOAT1
    mem.write(VAF, &declaration).unwrap();
    mem.write(shader + 0x14, &declaration).unwrap();
    device.set_vertex_shader_input(&mut DeviceMemory(mem), shader | 1, 1, INPUTS).unwrap();
    device.select_vertex_shader_direct(&mut DeviceMemory(mem), VAF, 0).unwrap();
}

fn device_bytes((device, mem): &mut Scene) -> Vec<u8> {
    let mut writer = SnapshotWriter::new(Header {
        build: BuildKey { stamp: "world-interleave".into(), switches: String::new() },
        name: "d3d".into(), tick: 0, presents: 0, notes: Vec::new(),
    });
    device.save_snapshot(&mut DeviceMemory(mem), &mut writer).unwrap();
    writer.finish().unwrap()
}

fn observe(serial: &mut Scene, actual: &mut Scene, n: u32) {
    // The retained buffer/palette and the previously released references
    // are part of the observation, even when the same B is rebound later.
    let one_internal = d3d::resource::common::INTREFCOUNT_ONE;
    assert_eq!(actual.1.u32(BUFFERS + n * 12), Ok(1 + one_internal));
    assert_eq!(actual.1.u32(PALETTES + n * 12), Ok(1 + one_internal));
    assert_eq!(actual.1.u32(BUFFERS + (1 - n) * 12), Ok(1));
    let reference = device_bytes(serial);
    let observed = device_bytes(actual);
    assert_eq!(reference.len(), observed.len());
    assert_eq!(reference.iter().zip(&observed).position(|(a, b)| a != b), None,
        "first differing byte in D3D snapshot");
    let first = serial.1.bytes.iter().zip(&actual.1.bytes).position(|(a, b)| a != b);
    assert_eq!(first.map(|off| serial.1.base + off as u32), None, "first differing guest address");
}

#[test]
fn stream_handoff_preserves_all_device_and_guest_bytes_across_interleaved_guest() {
    let (mut reference, mut mixed) = (scene(), scene());
    serial(&mut reference, 0);
    native(&mut mixed, 0);
    observe(&mut reference, &mut mixed, 0); // guest entry
    serial(&mut reference, 1);
    serial(&mut mixed, 1); // guest rebinds stream, declaration and palette
    serial(&mut reference, 0);
    native(&mut mixed, 0);
    observe(&mut reference, &mut mixed, 0); // end of list
}
