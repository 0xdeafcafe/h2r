//! WGSL overrides of translated programs, hot (docs/graphics/hot-iteration.md):
//! a fixed-function draw through `Nv2aShaders` on a real GPU, then its
//! program dumped, overridden from a directory, re-parameterised, refused
//! when it would not fit, and switched off, each seen in the pixels of the
//! next frame. Every shader and state is synthetic. Skips when no adapter
//! is available.

mod common;

use common::*;
use d3d::nv2a::Nv2aShaders;
use d3d::{Event, GfxBackend, GuestMemory, SparseMemory, UploadPolicy};
use gfx::GfxContext;
use gfx::surface::HostPixelOrder;
use gfx::vertex::fvf;
use hot_render::{Applied, DumpRequest, Hub, OVERRIDES};

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("d3d-hot-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn back_buffer(d: &mut d3d::Device<GfxBackend>) -> impl Fn(u32, u32) -> [u8; 4] + use<> {
    let (px, order) = d.backend.read_color(d.back_buffer().unwrap()).unwrap();
    move |x, y| {
        let p = pixel(&px, x, y);
        match order {
            HostPixelOrder::Bgra8 => [p[2], p[1], p[0], p[3]],
            _ => p,
        }
    }
}

fn no_skips(d: &mut d3d::Device<GfxBackend>) {
    let events = d.take_events();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, Event::DrawSkipped { .. } | Event::BadState(_))),
        "{events:?}"
    );
}

/// A full-target quad whose colour is `TFACTOR` (`0xFF204060`): fixed
/// function, so it runs a program the provider translates.
fn setup(d: &mut d3d::Device<GfxBackend>, m: &mut SparseMemory) {
    set_states(d, m);
    texture(m, 0);
    d.set_texture(m, 0, TEX_HEADER).unwrap();
    let mut vb = Vec::new();
    for (x, y) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [W as f32 * x, H as f32 * y, 0.5, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&0xFF80_FFFFu32.to_le_bytes());
        for f in [16.0 * x, 4.0 * y] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100).unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 28).unwrap();
    let l = d3d::GuestLayout::HALO2;
    m.write_u32(l.texture_state_slot(0, 14), 3).unwrap(); // COLORARG1 = TFACTOR
    m.write_u32(l.texture_state_slot(0, 12), 2).unwrap(); // COLOROP = SELECTARG1
    m.write_u32(l.texture_state_slot(0, 18), 0).unwrap(); // ALPHAARG1 = DIFFUSE
    d.set_render_state(m, 148, 0xFF20_4060).unwrap(); // TEXTUREFACTOR
}

/// Swaps (the provider's once-a-frame look at the hub), draws the quad
/// and reads the middle pixel.
fn frame(d: &mut d3d::Device<GfxBackend>, m: &mut SparseMemory) -> [u8; 4] {
    d.swap(m, 0).unwrap();
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(d);
    back_buffer(d)(32, 32)
}

#[test]
fn an_override_replaces_its_program_on_the_next_frame() {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => return eprintln!("skipping GPU test: {e}"),
    };
    let scope = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let hub: &'static Hub = Box::leak(Box::new(Hub::new()));
    let backend = GfxBackend::new(ctx).with_shaders(Box::new(Nv2aShaders::new().with_hot(hub)));
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    setup(&mut d, &mut m);
    d.draw_vertices(&mut m, TRIANGLESTRIP, 0, 4).unwrap();
    no_skips(&mut d);
    assert_eq!(back_buffer(&mut d)(32, 32), [0x20, 0x40, 0x60, 0xFF]);

    // The provider publishes the program at its next frame.
    assert_eq!(frame(&mut d, &mut m), [0x20, 0x40, 0x60, 0xFF]);
    let programs = hub.programs();
    assert_eq!(programs.len(), 1, "{programs:?}");
    let (id, info) = &programs[0];
    assert_eq!(info.vertex, "pretransformed");

    // Dump it: the starting point for an override.
    let dump = scratch("dump");
    hub.request_dump(DumpRequest {
        program: hot_render::parse_program(id),
        to: dump.clone(),
        tries: 0,
    });
    frame(&mut d, &mut m);
    let report = hub.status().last_dump.expect("dumped");
    assert_eq!(report.written, vec![format!("{id}.wgsl")], "{report:?}");
    let translated = std::fs::read_to_string(dump.join(format!("{id}.wgsl"))).unwrap();
    assert!(translated.contains("return r.color;"), "{translated}");

    // Override it: a parameterised constant colour.
    let dir = scratch("overrides");
    hub.set_dir(Some(dir.clone()));
    let file = dir.join(format!("{id}_flat.wgsl"));
    let flat = translated.replace(
        "return r.color;",
        "return vec4f({{flat.r=1.0}}, 0.0, 1.0, 1.0);",
    );
    std::fs::write(&file, &flat).unwrap();
    let r = hub.reload();
    assert!(r.problems.is_empty(), "{r:?}");
    assert_eq!(frame(&mut d, &mut m), [0xFF, 0x00, 0xFF, 0xFF]);
    assert_eq!(hub.status().overrides[0].applied, Applied::Applied);

    // A parameter recompiles it.
    hub.set_param("flat.r", Some(0.0)).unwrap();
    assert_eq!(frame(&mut d, &mut m), [0x00, 0x00, 0xFF, 0xFF]);

    // An override that asks for a binding the program doesn't have is
    // refused, and the last good one keeps running.
    let bad = flat.replacen(
        "@fragment",
        "@group(1) @binding(9) var extra: texture_2d<f32>;\n@fragment",
        1,
    );
    std::fs::write(&file, bad).unwrap();
    let r = hub.reload();
    assert!(r.problems.is_empty(), "it parses: {r:?}");
    assert_eq!(frame(&mut d, &mut m), [0x00, 0x00, 0xFF, 0xFF]);
    match &hub.status().overrides[0].applied {
        Applied::Rejected { error } => assert!(error.contains("@binding(9)"), "{error}"),
        other => panic!("{other:?}"),
    }

    // Switched off, the translation runs again; on, the override.
    std::fs::write(&file, &flat).unwrap();
    hub.reload();
    assert_eq!(frame(&mut d, &mut m), [0x00, 0x00, 0xFF, 0xFF]);
    hub.set_toggle(OVERRIDES, Some(false)).unwrap();
    assert_eq!(frame(&mut d, &mut m), [0x20, 0x40, 0x60, 0xFF]);
    hub.set_toggle(OVERRIDES, Some(true)).unwrap();
    assert_eq!(frame(&mut d, &mut m), [0x00, 0x00, 0xFF, 0xFF]);

    // Removing the file is the translation again.
    std::fs::remove_file(&file).unwrap();
    hub.reload();
    assert_eq!(frame(&mut d, &mut m), [0x20, 0x40, 0x60, 0xFF]);

    let err = pollster::block_on(scope.pop());
    assert!(err.is_none(), "wgpu validation error: {err:?}");
}
