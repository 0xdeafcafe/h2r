//! D3D round 6: the vertex-array methods and inline arrays that BeginPush
//! (`0x3fae10`) and `CDevice_SetStateVB` (`0x401030`) feed, the same-size
//! SetRenderTarget (`0x3f7b30`), and the NV2A register window behind
//! device `+0x934`. Synthetic values only; the ignored tests at the end
//! decode the retail image in `$H2_XBE`.

mod common;

use common::*;
use d3d::abi::Regs;
use d3d::backend::Viewport;
use d3d::hw::method as hw;
use d3d::layout::device_field as f;
use d3d::pushbuf::method;
use d3d::{D3dError, Device, GuestMemory, RecordingBackend, SparseMemory, UploadPolicy};
use gfx::vertex::{PrimitiveType, VertexDataType, VertexRegister, fvf};

const DEV: u32 = 0x0040_4FE0;

fn rec() -> (Device<RecordingBackend>, SparseMemory) {
    create(RecordingBackend::new(), UploadPolicy::Tracked)
}

fn push_words(m: &mut SparseMemory, words: &[u32]) {
    let mut put = m.read_u32(DEV).unwrap();
    for w in words {
        m.write_u32(put, *w).unwrap();
        put += 4;
    }
    m.write_u32(DEV, put).unwrap();
}

fn header(m: u32, count: u32) -> u32 {
    (count << 18) | m
}

/// What `CDevice_SetStateVB` pushes for a vertex: all 16 formats in one
/// incrementing command (`0x4010b2`), then one single-word offset command
/// per enabled register (`0x401134`).
fn set_state_vb(m: &mut SparseMemory, formats: &[(usize, u32)], stride: u32) {
    let mut words = vec![header(hw::VERTEX_DATA_ARRAY_FORMAT, 16)];
    let mut regs = [d3d::hw::VERTEX_FORMAT_DISABLED; 16];
    for &(r, fmt) in formats {
        regs[r] = stride << 8 | fmt;
    }
    words.extend(regs);
    for &(r, _) in formats {
        words.push(header(hw::VERTEX_DATA_ARRAY_OFFSET + 4 * r as u32, 1));
        words.push(VB_DATA + 0x10 * r as u32);
    }
    push_words(m, &words);
}

/// What `0x4b220` pushes after BeginPush: `BEGIN_END`, one non-incrementing
/// `INLINE_ARRAY` with the vertex bytes, `BEGIN_END` 0.
fn inline_array(m: &mut SparseMemory, primitive: u32, bytes: &[u8]) {
    assert!(bytes.len().is_multiple_of(4));
    let n = (bytes.len() / 4) as u32;
    let mut words = vec![header(method::BEGIN_END, 1), primitive];
    words.push(0x4000_0000 | header(method::INLINE_ARRAY, n));
    words.extend(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c)),
    );
    words.extend([header(method::BEGIN_END, 1), 0]);
    push_words(m, &words);
}

fn kick(d: &mut Device<RecordingBackend>, m: &mut SparseMemory) -> Result<(), D3dError> {
    d.kick_off(m)
}

// Inline arrays ---------------------------------------------------------------

#[test]
fn set_state_vb_formats_reach_the_register_state() {
    let (mut d, mut m) = rec();
    set_state_vb(&mut m, &[(0, 0x42), (3, 0x40)], 20);
    kick(&mut d, &mut m).unwrap();
    let hw = d.hw();
    assert_eq!(hw.vertex_format[0], 20 << 8 | 0x42);
    assert_eq!(hw.vertex_format[3], 20 << 8 | 0x40);
    assert_eq!(hw.vertex_format[1], d3d::hw::VERTEX_FORMAT_DISABLED);
    assert_eq!(hw.vertex_offset[3], VB_DATA + 0x30);
}

#[test]
fn inline_array_draws_with_the_pushed_formats() {
    let (mut d, mut m) = rec();
    set_states(&mut d, &mut m);
    d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE)
        .unwrap();
    set_state_vb(&mut m, &[(0, 0x42), (3, 0x40)], 20);
    let mut bytes = Vec::new();
    for (x, y) in [(0.0, 0.0), (64.0, 0.0), (0.0, 64.0), (64.0, 64.0)] {
        bytes.extend(vertex(x, y, 0xFF11_2233));
    }
    inline_array(&mut m, TRIANGLESTRIP, &bytes);
    kick(&mut d, &mut m).unwrap();

    let draws = d.backend.draws();
    assert_eq!(draws.len(), 1);
    let draw = draws[0];
    assert_eq!(draw.primitive, PrimitiveType::TriangleStrip);
    assert_eq!(draw.vertex_count, 4);
    assert!(draw.layout.pretransformed);
    let s = &draw.layout.streams[0];
    assert_eq!(s.stride, 20);
    let attrs: Vec<_> = s
        .attributes
        .iter()
        .map(|a| (a.register, a.data_type, a.offset))
        .collect();
    assert_eq!(
        attrs,
        vec![
            (VertexRegister(0), VertexDataType::Float4, 0),
            (VertexRegister(3), VertexDataType::D3dColor, 16),
        ]
    );
    assert_eq!(draw.streams[0].data, bytes);
    assert_eq!(d.stats().inline_array_draws, 1);
}

#[test]
fn inline_array_packing_aligns_each_register_to_its_component() {
    let (mut d, mut m) = rec();
    set_states(&mut d, &mut m);
    d.set_vertex_shader(&mut m, fvf::XYZRHW).unwrap();
    // float4 (16 bytes), short1 (2, padded to 4), float1 (4): 24 bytes.
    // The stride field (99) is not used.
    set_state_vb(&mut m, &[(0, 0x42), (1, 0x15), (2, 0x12)], 99);
    inline_array(&mut m, TRIANGLESTRIP, &[0u8; 24 * 3]);
    kick(&mut d, &mut m).unwrap();
    let draws = d.backend.draws();
    let s = &draws[0].layout.streams[0];
    assert_eq!(s.stride, 24);
    let offsets: Vec<_> = s.attributes.iter().map(|a| a.offset).collect();
    assert_eq!(offsets, vec![0, 16, 20]);
    assert_eq!(draws[0].vertex_count, 3);
}

#[test]
fn inline_array_drops_a_trailing_partial_vertex() {
    let (mut d, mut m) = rec();
    set_states(&mut d, &mut m);
    d.set_vertex_shader(&mut m, fvf::XYZRHW | fvf::DIFFUSE)
        .unwrap();
    set_state_vb(&mut m, &[(0, 0x42), (3, 0x40)], 20);
    // 16 words: three whole 20-byte vertices and 4 bytes over.
    let mut bytes = Vec::new();
    for (x, y) in [(0.0, 0.0), (64.0, 0.0), (0.0, 64.0)] {
        bytes.extend(vertex(x, y, 0xFF00_FF00));
    }
    bytes.extend([0xEE; 4]);
    inline_array(&mut m, TRIANGLESTRIP, &bytes);
    // Fewer bytes than one vertex: nothing to draw.
    inline_array(&mut m, TRIANGLESTRIP, &[0u8; 12]);
    kick(&mut d, &mut m).unwrap();
    let draws = d.backend.draws();
    assert_eq!(draws.len(), 1);
    assert_eq!(draws[0].vertex_count, 3);
    assert_eq!(draws[0].streams[0].data, bytes[..60]);
    assert!(d.fault().is_none());
}

#[test]
fn inline_array_with_every_format_disabled_stops() {
    let (mut d, mut m) = rec();
    d.set_vertex_shader(&mut m, fvf::XYZRHW).unwrap();
    set_state_vb(&mut m, &[], 0);
    inline_array(&mut m, TRIANGLESTRIP, &[0u8; 48]);
    let err = kick(&mut d, &mut m).unwrap_err().to_string();
    assert!(err.contains("every vertex array format disabled"), "{err}");
    assert!(d.fault().is_some(), "the device latches the fault");
}

/// A vertex program selected the way Halo does it (SelectVertexShaderDirect
/// with an attribute format at `vaf`), so draws run as programs.
fn program(d: &mut Device<RecordingBackend>, m: &mut SparseMemory) {
    let vaf = MISC + 0x400;
    for reg in 0..16u32 {
        m.write_u32(vaf + 16 * reg + 8, 0x02).unwrap(); // D3DVSDT_NONE
    }
    d.select_vertex_shader_direct(m, vaf, 0).unwrap();
}

#[test]
fn inline_arrays_leave_the_last_vertex_in_the_attribute_registers() {
    let (mut d, mut m) = rec();
    set_states(&mut d, &mut m);
    program(&mut d, &mut m);
    // Position float4, diffuse D3DCOLOR in v3, a packed normal in v2.
    set_state_vb(&mut m, &[(0, 0x42), (2, 0x16), (3, 0x40)], 24);
    let normal = |x: i32, y: i32, z: i32| {
        (x as u32 & 0x7FF) | ((y as u32 & 0x7FF) << 11) | ((z as u32 & 0x3FF) << 22)
    };
    let mut bytes = Vec::new();
    for (i, colour) in [0xFF00_0000u32, 0x8040_4040].iter().enumerate() {
        for f in [i as f32, 0.0, 0.5, 1.0] {
            bytes.extend(f.to_le_bytes());
        }
        bytes.extend(normal(1023, -1023, 511).to_le_bytes());
        bytes.extend(colour.to_le_bytes());
    }
    bytes.extend([0u8; 24]); // a third vertex, so a strip has a triangle
    inline_array(&mut m, TRIANGLESTRIP, &bytes);
    kick(&mut d, &mut m).unwrap();
    {
        let draws = d.backend.draws();
        let s = &draws[0].layout.streams[0];
        let attrs: Vec<_> = s
            .attributes
            .iter()
            .map(|a| (a.register.0, a.data_type, a.offset))
            .collect();
        assert_eq!(
            attrs,
            vec![
                (0, VertexDataType::Float4, 0),
                (2, VertexDataType::NormPacked3, 16),
                (3, VertexDataType::D3dColor, 20),
            ]
        );
        assert_eq!(s.stride, 24);
    }
    // Same again with the last vertex grey and the normal set, then a draw
    // with v2 and v3 disabled: their values carry over.
    let mut bytes = Vec::new();
    for colour in [0u32, 0, 0x8040_4040] {
        for f in [0.0f32, 0.0, 0.5, 1.0] {
            bytes.extend(f.to_le_bytes());
        }
        bytes.extend(normal(1023, -1023, -512).to_le_bytes());
        bytes.extend(colour.to_le_bytes());
    }
    inline_array(&mut m, TRIANGLESTRIP, &bytes);
    set_state_vb(&mut m, &[(0, 0x42)], 16);
    inline_array(&mut m, TRIANGLESTRIP, &[0u8; 48]);
    kick(&mut d, &mut m).unwrap();
    let draws = d.backend.draws();
    assert_eq!(draws.len(), 3);
    let defaults = draws[2].shader.vertex_defaults;
    let g = 0x40 as f32 / 255.0;
    assert_eq!(defaults[3], [g, g, g, 0x80 as f32 / 255.0]);
    assert_eq!(defaults[2], [1.0, -1.0, -1.0, 1.0]);
    // Enabled registers take the draw's own last vertex (all zero here,
    // w included: a float4 replaces all four components).
    assert_eq!(defaults[0], [0.0; 4]);
}

#[test]
fn inline_array_with_an_unknown_format_stops() {
    let (mut d, mut m) = rec();
    d.set_vertex_shader(&mut m, fvf::XYZRHW).unwrap();
    // Type 3 is not a vertex type.
    set_state_vb(&mut m, &[(0, 0x43)], 16);
    inline_array(&mut m, TRIANGLESTRIP, &[0u8; 48]);
    let err = kick(&mut d, &mut m).unwrap_err().to_string();
    assert!(err.contains("array format 0x43"), "{err}");
}

// SetRenderTarget, same size ----------------------------------------------------

/// A 64x64 linear target the size of the test back buffer.
fn same_size_target(m: &mut SparseMemory) -> u32 {
    let surf = MISC + 0x7300;
    m.map(0x8000_0000 | 0x0030_0000, 64 * 64 * 4);
    d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, W, H, W * 4, 0x0030_0000)
        .write(m, surf)
        .unwrap();
    surf
}

#[test]
fn same_size_set_render_target_keeps_the_viewport() {
    let (mut d, mut m) = rec();
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let depth = m.read_u32(DEV + f::DEPTH_STENCIL).unwrap();
    let surf = same_size_target(&mut m);
    // A smaller viewport the original would leave alone.
    let vp = MISC + 0x300;
    for (i, w) in [8u32, 4, 16, 16, 0.0f32.to_bits(), 1.0f32.to_bits()]
        .iter()
        .enumerate()
    {
        m.write_u32(vp + 4 * i as u32, *w).unwrap();
    }
    d.set_viewport(&mut m, vp).unwrap();
    let before = d.viewport();
    let flags = m.read_u32(DEV + f::FLAGS).unwrap();

    // [esp+4] = target, EBX = depth (none), ret 4.
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    assert_eq!(m.read_u32(DEV + f::RENDER_TARGET).unwrap(), surf);
    assert_eq!(m.read_u32(DEV + f::DEPTH_STENCIL).unwrap(), 0);
    assert_eq!(d.viewport(), before);
    assert_eq!(
        before,
        Viewport {
            x: 8,
            y: 4,
            width: 16,
            height: 16,
            min_z: 0.0,
            max_z: 1.0
        }
    );

    // Back to the back buffer with its depth buffer; flag 0x100 is cleared
    // there (0x3f7c7d), and only there.
    m.write_u32(DEV + f::FLAGS, flags | 0x100).unwrap();
    common::call(
        &mut d,
        &mut m,
        0x3f7b30,
        Regs {
            ebx: depth,
            ..Regs::default()
        },
        &[back],
    )
    .unwrap();
    assert_eq!(m.read_u32(DEV + f::RENDER_TARGET).unwrap(), back);
    assert_eq!(m.read_u32(DEV + f::DEPTH_STENCIL).unwrap(), depth);
    assert_eq!(m.read_u32(DEV + f::FLAGS).unwrap() & 0x100, 0);
    assert_eq!(d.viewport(), before);
}

#[test]
fn same_size_set_render_target_rejects_a_null_target() {
    let (mut d, mut m) = rec();
    let err = common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[0])
        .unwrap_err()
        .to_string();
    assert!(err.contains("null render target"), "{err}");
}

#[test]
fn same_size_set_render_target_writes_a_texture_target_back() {
    let (mut d, mut m) = rec();
    texture(&mut m, 0);
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(&mut m, surf).unwrap();
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    d.clear(&mut m, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[back]).unwrap();
    assert_eq!(d.stats().rtt_writebacks, 1);
}

#[test]
fn same_size_targets_hold_an_internal_reference_while_bound() {
    use d3d::resource::common::INTREFCOUNT_ONE as ONE;
    let (mut d, mut m) = rec();
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let surf = same_size_target(&mut m);
    let common = m.read_u32(surf).unwrap();
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    assert_eq!(m.read_u32(surf).unwrap(), common + ONE);
    // The title lets go of its reference while the surface is bound: the
    // binding keeps it alive.
    let n = common::call(&mut d, &mut m, 0x3fb640, Regs::default(), &[surf]);
    assert_eq!(n.unwrap().eax, Some(0));
    // Unbinding the last reference would destroy it, which stops.
    let err = common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[back])
        .unwrap_err()
        .to_string();
    assert!(err.contains("D3D_DestroyResource"), "{err}");
}

#[test]
fn texture_level_targets_reference_their_parent_once() {
    use d3d::resource::common::INTREFCOUNT_ONE as ONE;
    let (mut d, mut m) = rec();
    texture(&mut m, 0);
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let depth = m.read_u32(DEV + f::DEPTH_STENCIL).unwrap();
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(&mut m, surf).unwrap();
    let (s0, p0) = (m.read_u32(surf).unwrap(), m.read_u32(TEX_HEADER).unwrap());
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    // Again while bound: the surface gains a reference, the parent not.
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    assert_eq!(m.read_u32(surf).unwrap(), s0 + ONE);
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p0 + ONE);
    // As a depth buffer too (EBX), through the full SetRenderTarget.
    m.write_u32(DEV + f::GPU_TIME, 7).unwrap();
    common::call(
        &mut d,
        &mut m,
        0x3f7b30,
        Regs {
            ebx: depth,
            ..Regs::default()
        },
        &[back],
    )
    .unwrap();
    assert_eq!(m.read_u32(surf).unwrap(), s0);
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p0);
    // Lock of the parent (the level's resource) is stamped with the time.
    assert_eq!(m.read_u32(TEX_HEADER + 8).unwrap(), 7);
    d.set_render_target(&mut m, surf, 0).unwrap();
    assert_eq!(m.read_u32(surf).unwrap(), s0 + ONE);
    d.set_render_target(&mut m, back, depth).unwrap();
    assert_eq!(m.read_u32(surf).unwrap(), s0);
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p0);
}

/// A 16x4 level of the test texture, with the texture as its parent.
fn texture_level(m: &mut SparseMemory) -> u32 {
    texture(m, 0);
    let surf = MISC + 0x7200;
    let mut h =
        d3d::resource::SurfaceHeader::linear(gfx::D3DFormat::LinA8R8G8B8, 16, 4, 64, TEX_DATA);
    h.parent = TEX_HEADER;
    h.write(m, surf).unwrap();
    surf
}

#[test]
fn teardown_releases_the_bound_title_targets() {
    // A level (parent accounting) and a plain surface, each still bound
    // when the device goes; the caller keeps its own reference.
    for level in [true, false] {
        let (mut d, mut m) = rec();
        let surf = if level {
            texture_level(&mut m)
        } else {
            same_size_target(&mut m)
        };
        let (s0, p0) = (m.read_u32(surf).unwrap(), m.read_u32(TEX_HEADER).unwrap());
        common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
        m.write_u32(DEV + f::GPU_TIME, 9).unwrap();
        d.release_device(&mut m, DEV).unwrap();
        assert_eq!(m.read_u32(surf).unwrap(), s0, "level {level}");
        assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p0, "level {level}");
        let stamped = if level { TEX_HEADER } else { surf };
        assert_eq!(m.read_u32(stamped + 8).unwrap(), 9, "level {level}");
        assert_eq!(m.read_u32(DEV + f::RENDER_TARGET).unwrap(), 0);
        assert_eq!(m.read_u32(DEV + f::DEPTH_STENCIL).unwrap(), 0);
    }
}

#[test]
fn a_teardown_that_stops_changes_no_reference() {
    let (mut d, mut m) = rec();
    let surf = texture_level(&mut m);
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    common::call(&mut d, &mut m, 0x3fb640, Regs::default(), &[surf]).unwrap();
    let (s, p) = (m.read_u32(surf).unwrap(), m.read_u32(TEX_HEADER).unwrap());
    let err = d.release_device(&mut m, DEV).unwrap_err().to_string();
    assert!(err.contains("D3D_DestroyResource"), "{err}");
    assert_eq!(m.read_u32(surf).unwrap(), s);
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p);
}

#[test]
fn an_unbind_that_stops_changes_nothing_and_can_be_retried() {
    use d3d::resource::common::INTREFCOUNT_ONE as ONE;
    let (mut d, mut m) = rec();
    let back = m.read_u32(DEV + f::RENDER_TARGET).unwrap();
    let surf = texture_level(&mut m);
    let (s0, p0) = (m.read_u32(surf).unwrap(), m.read_u32(TEX_HEADER).unwrap());
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[surf]).unwrap();
    // The title drops its reference; only the binding holds the level.
    common::call(&mut d, &mut m, 0x3fb640, Regs::default(), &[surf]).unwrap();
    let bound = (m.read_u32(surf).unwrap(), m.read_u32(TEX_HEADER).unwrap());
    assert_eq!(bound, (s0 - 1 + ONE, p0 + ONE));
    for _ in 0..2 {
        let err = common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[back])
            .unwrap_err()
            .to_string();
        assert!(err.contains("D3D_DestroyResource"), "{err}");
        assert_eq!(
            (m.read_u32(surf).unwrap(), m.read_u32(TEX_HEADER).unwrap()),
            bound
        );
        assert_eq!(m.read_u32(DEV + f::RENDER_TARGET).unwrap(), surf);
    }
    // With a caller reference back, the same switch goes through.
    let c = m.read_u32(surf).unwrap();
    m.write_u32(surf, c + 1).unwrap();
    common::call(&mut d, &mut m, 0x3f7b30, Regs::default(), &[back]).unwrap();
    assert_eq!(m.read_u32(surf).unwrap(), s0);
    assert_eq!(m.read_u32(TEX_HEADER).unwrap(), p0);
    assert_eq!(m.read_u32(DEV + f::RENDER_TARGET).unwrap(), back);
}

#[test]
fn commit_keeps_the_bits_set_state_vb_consumes() {
    let (mut d, mut m) = rec();
    let l = d3d::GuestLayout::HALO2;
    m.write_u32(l.dirty_flags, 0xFFFF_FFFF).unwrap();
    common::call(&mut d, &mut m, 0x400fa0, Regs::default(), &[]).unwrap();
    assert_eq!(m.read_u32(l.dirty_flags).unwrap(), 0xC000_0070);
    // Shader selection sets the vertex bits; a commit before BeginPush
    // leaves them for the native SetStateVB.
    m.write_u32(l.dirty_flags, 0).unwrap();
    program(&mut d, &mut m);
    common::call(&mut d, &mut m, 0x400fa0, Regs::default(), &[]).unwrap();
    assert_eq!(m.read_u32(l.dirty_flags).unwrap() & 0x70, 0x70);
}
