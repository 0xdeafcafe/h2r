//! Synthetic uniform blocks exercise the serialized GPU ABI, including
//! NaN payloads, signed zero and integer stipple rows.
use shaders::psh::PixelUniforms;
use shaders::vsh::VertexUniforms;

fn vector(seed: u32) -> [f32; 4] {
    [
        f32::from_bits(0x7fc0_0000 | seed),
        -0.0,
        seed as f32,
        -(seed as f32),
    ]
}

fn bytes(vectors: &[[f32; 4]]) -> Vec<u8> {
    vectors
        .iter()
        .flatten()
        .flat_map(|f| f.to_le_bytes())
        .collect()
}

#[test]
fn vertex_uniform_bytes_match_the_gpu_layout_on_reuse() {
    let mut u = VertexUniforms::default();
    for (i, v) in u.constants.iter_mut().chain(&mut u.defaults).enumerate() {
        *v = vector(i as u32 + 1);
    }
    u.surface = [-0.0, 1024.0];
    u.z_max = 65535.0;
    let mut out = [0xa5; VertexUniforms::SIZE];
    for clip in [None, Some([-0.0, 100.0]), None] {
        for clamp in [false, true] {
            u.w_clip = clip;
            u.w_clamp = clamp;
            let mut expected = Vec::new();
            expected.extend_from_slice(&u.constants);
            expected.extend_from_slice(&u.defaults);
            expected.push([u.surface[0], u.surface[1], u.z_max, 0.0]);
            expected.push(match clip {
                Some([lo, hi]) => [lo, hi, 1.0, if clamp { 1.0 } else { 0.0 }],
                None => [0.0; 4],
            });
            u.write_bytes(&mut out);
            assert_eq!(out.as_slice(), bytes(&expected));
            assert_eq!(out.as_slice(), u.to_bytes());
        }
    }
}

#[test]
fn pixel_uniform_bytes_match_the_gpu_layout_on_reuse() {
    let mut u = PixelUniforms::default();
    let mut out = [0xa5; PixelUniforms::SIZE];
    for seed in [1, 100, 0] {
        let mut next = seed;
        let mut set = |v: &mut [f32; 4]| {
            *v = vector(next);
            next += 1;
        };
        for v in &mut u.c0 {
            set(v);
        }
        for v in &mut u.c1 {
            set(v);
        }
        set(&mut u.fog_color);
        set(&mut u.fog);
        for v in &mut u.bump_mat {
            set(v);
        }
        for v in &mut u.bump_lum {
            set(v);
        }
        for v in &mut u.tex_scale {
            set(v);
        }
        set(&mut u.eye);
        set(&mut u.alpha_ref);
        for v in &mut u.border {
            set(v);
        }
        set(&mut u.lod_bias);
        set(&mut u.depth);
        let [lo, hi, _, _] = vector(next);
        u.depth_range = [lo, hi];
        u.stipple_scale = [2.0 + seed as f32, 3.0 + seed as f32];
        u.stipple = std::array::from_fn(|i| 0xff80_0000 | ((i as u32 + seed) * 333));
        // Independent field order, matching the WGSL layout.
        let mut expected = Vec::new();
        expected.extend_from_slice(&u.c0);
        expected.extend_from_slice(&u.c1);
        expected.push(u.fog_color);
        expected.push(u.fog);
        expected.extend_from_slice(&u.bump_mat);
        expected.extend_from_slice(&u.bump_lum);
        expected.extend_from_slice(&u.tex_scale);
        expected.push(u.eye);
        expected.push(u.alpha_ref);
        expected.extend_from_slice(&u.border);
        expected.push(u.lod_bias);
        for row in u.stipple.as_chunks::<4>().0 {
            expected.push(row.map(f32::from_bits));
        }
        expected.push(u.depth);
        expected.push([lo, hi, u.stipple_scale[0], u.stipple_scale[1]]);
        u.write_bytes(&mut out);
        assert_eq!(out.as_slice(), bytes(&expected));
        assert_eq!(out.as_slice(), u.to_bytes());
    }
}
