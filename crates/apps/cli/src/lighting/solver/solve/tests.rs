use super::*;

// A floor eight units across, under a roof whose edge cuts a texel in
// half. The roof exists only in the scenery arm. The fixture computes the
// retail bake from the exact area of exposed floor, independently of the
// rasterizer, its subpixel pattern and the ray query.
const K: Rgb = [112, 64, 32];
const ROOF_EDGE: f32 = 3.5;

fn floor() -> Vec<Triangle> {
    let corners = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
    [[0usize, 1, 2], [2, 3, 0]]
        .map(|indices| {
            let uv = indices.map(|i| corners[i]);
            Triangle {
                position: uv.map(|[u, v]| [8. * u, 4. * v, 0.]),
                uv,
                chart: 11,
            }
        })
        .to_vec()
}

fn area_visible(x: usize, width: usize) -> f32 {
    let dx = 8.0 / width as f32;
    (((x + 1) as f32 * dx - ROOF_EDGE) / dx).clamp(0.0, 1.0)
}

fn forward(indirect: Rgb, visible: f32) -> Rgb {
    from_fn(|c| indirect[c] + (K[c] as f32 * visible).round() as u8)
}

fn sun() -> Source {
    Source::sky([0., 0., 1.], 0.0).unwrap()
}

#[test]
fn recovers_fitted_light_and_shadow_at_each_stored_mip() {
    let roof = crate::split::scene::test_roof(ROOF_EDGE, 2.0);
    let mut fits = FitSamples::default();
    let mut pages = Vec::new();
    for (width, height, fill) in [(8, 4, [22, 13, 45]), (4, 2, [9, 41, 32])] {
        let trace = trace(width, height, &floor(), &sun(), &roof).unwrap();
        let retail: Vec<_> = (0..width * height)
            .map(|i| forward(fill, area_visible(i % width, width)))
            .collect();
        fits.add(&trace, &retail);
        pages.push((trace, retail, fill));
    }
    assert!(
        fits.fit(0).is_none(),
        "the scenery-free scene has no shadow edge"
    );
    let fit = fits.fit(1).unwrap();
    assert_eq!(fit.k, K.map(f32::from));
    assert_eq!(fit.median_error, 0.0);
    for (trace, retail, fill) in pages {
        let out = split_page(&trace, &retail, fit.k, 1);
        assert_eq!(out.stats.covered, retail.len());
        assert!(out.stats.edge > 0);
        assert_eq!(out.stats.clamped, 0);
        assert!(out.stats.max_play_error <= 1);
        for (i, p) in out.texels.iter().enumerate() {
            assert_eq!(
                p.visibility,
                byte(255.0 * area_visible(i % trace.width, trace.width))
            );
            assert_eq!(p.direct, K);
            assert_eq!(p.lamp_off, fill);
            assert_eq!(p.closure_i, fill);
            assert_eq!(p.lamp_on, retail[i]);
        }
        // A sample between four texels uses the same linear weights on both
        // stored closure pages. Check it across the penumbra in each mip.
        let i = if trace.width == 8 { 2 } else { 0 };
        let neighbours = [i, i + 1, i + trace.width, i + trace.width + 1];
        let weights = [0.12, 0.18, 0.28, 0.42];
        for c in 0..3 {
            let mut original = 0.0_f32;
            let mut indirect = 0.0;
            let mut baked = 0.0;
            for (j, w) in neighbours.into_iter().zip(weights) {
                original += w * retail[j][c] as f32;
                indirect += w * out.texels[j].closure_i[c] as f32;
                baked += w * out.texels[j].baked[c] as f32;
            }
            assert!((indirect + baked - original).abs() <= 1.0);
        }
    }
}

#[test]
fn fitted_slope_resists_an_unrelated_edge_and_play_fills_from_both_sides() {
    let roof = crate::split::scene::test_roof(ROOF_EDGE, 2.0);
    let trace = trace(8, 4, &floor(), &sun(), &roof).unwrap();
    let fill = [12, 36, 7];
    let mut retail: Vec<_> = (0..32)
        .map(|i| forward(fill, area_visible(i % 8, 8)))
        .collect();
    // This disagreement at the traced edge must stay in the closure set,
    // but should disappear from the lamp-off page an owner reviews.
    let edge_index = 11;
    retail[edge_index][0] += 13;
    let mut samples = FitSamples::default();
    samples.add(&trace, &retail);
    let k = samples.fit(1).unwrap().k;
    assert_eq!(k, K.map(f32::from));
    let out = split_page(&trace, &retail, k, 1);
    assert_eq!(out.stats.unfilled, 0);
    assert_eq!(out.stats.max_closure_error, 0);
    assert_eq!(out.texels[edge_index].closure_i[0], fill[0] + 13);
    assert_eq!(out.texels[edge_index].lamp_off, fill);
    assert_eq!(out.stats.max_play_error, 13);

    // A sloping fill between differently coloured anchors demonstrates
    // that the interpolation actually uses both sides.
    for y in 0..4 {
        retail[y * 8 + 1][1] += 20;
        retail[y * 8 + 5][1] += 4;
    }
    let out = split_page(&trace, &retail, k, 1);
    assert_eq!(out.texels[edge_index].lamp_off[1], fill[1] + 12);
}

#[test]
fn unmodelled_dark_patch_clamps_only_the_baked_closure_contribution() {
    let roof = crate::split::scene::test_roof(ROOF_EDGE, 2.0);
    let trace = trace(8, 1, &floor(), &sun(), &roof).unwrap();
    let fill = [12, 34, 5];
    let mut retail: Vec<_> = (0..8).map(|i| forward(fill, area_visible(i, 8))).collect();
    retail[7] = [1, 2, 3];
    let out = split_page(&trace, &retail, K.map(f32::from), 1);
    assert_eq!(out.stats.clamped, 1);
    assert_eq!(out.stats.max_closure_error, 0);
    assert_eq!(out.texels[7].baked, retail[7]);
    assert_eq!(out.texels[7].closure_i, [0; 3]);
    assert_eq!(out.texels[7].direct, K);
    assert_eq!(out.texels[7].lamp_on, K);
}

#[test]
fn vertex_split_traces_the_sun_disc_and_preserves_unselected_records() {
    let roof = crate::split::scene::test_roof(ROOF_EDGE, 2.0);
    let sun = Source::sky([0., 0., 1.], 0.08).unwrap();
    let fill = [17, 27, 13];
    // Two rows well inside/outside the shadow with a receiver exactly
    // below the roof rim. Half the sampled disc is visible from the rim.
    let xs = [0.0, 1.0, ROOF_EDGE, 6.0, 7.0, 8.0];
    let expected_v = [0.0, 0.0, 0.5, 1.0, 1.0, 0.0];
    let vertices = from_fn::<_, 6, _>(|i| Vertex {
        position: [xs[i], 0., 0.],
        normal: if i == 5 { [0.; 3] } else { [0., 0., 1.] },
        retail: forward(fill, expected_v[i]),
    });
    let split = split_vertices(
        &vertices,
        &[[0, 1], [1, 2], [2, 3], [3, 4]],
        &sun,
        &roof,
        K.map(f32::from),
        1,
    )
    .unwrap();
    assert_eq!(split.stats.covered, 5);
    assert_eq!(split.stats.unfilled, 0);
    for (i, p) in split.texels.iter().enumerate() {
        assert_eq!(p.visibility, byte(255. * expected_v[i]));
        assert_eq!(p.lamp_off, fill);
        assert_eq!(p.lamp_on, vertices[i].retail);
        assert_eq!(p.direct, if i == 5 { [0; 3] } else { K });
    }
}

#[test]
fn adjacent_uv_charts_do_not_create_a_shadow_pair_or_fill_across_a_seam() {
    let roof = crate::split::scene::test_roof(ROOF_EDGE, 2.0);
    let mut left = floor();
    for tri in &mut left {
        for uv in &mut tri.uv {
            uv[0] *= 0.5;
        }
        for p in &mut tri.position {
            p[0] *= 0.25;
        }
    }
    let mut right = left.clone();
    for tri in &mut right {
        tri.chart = 12;
        for uv in &mut tri.uv {
            uv[0] += 0.5;
        }
        for p in &mut tri.position {
            p[0] += 6.;
        }
    }
    left.extend(right);
    let trace = trace(8, 1, &left, &sun(), &roof).unwrap();
    let retail = [
        [24; 3], [24; 3], [24; 3], [24; 3], [160; 3], [160; 3], [160; 3], [160; 3],
    ];
    let mut samples = FitSamples::default();
    samples.add(&trace, &retail);
    assert!(samples.fit(1).is_none());
    assert_eq!(
        split_page(&trace, &retail, K.map(f32::from), 1).stats.edge,
        0
    );
}

#[test]
fn point_lamp_fit_recovers_the_floor_pool_up_to_channel_quantization() {
    let position = [3.5, 2., 100.];
    let radius = 800.;
    let source = Source::Placed(PlacedLamp {
        name: "invented ceiling lamp".into(),
        position,
        radius,
    });
    // Roof halfway between floor and lamp has its edge directly underneath
    // the emitter. Ray convergence therefore leaves the same half-lit
    // footprint as in the parallel-ray fixture. The ceiling beyond the lamp
    // is never an occluder.
    let visibility = |origin: V3, dir: V3, max: f32| {
        let at_roof = (50. - origin[2]) / dir[2];
        let ceiling = (120. - origin[2]) / dir[2];
        assert!(at_roof < max && ceiling > max);
        [true, origin[0] + at_roof * dir[0] > position[0]]
    };
    let trace = trace(8, 4, &floor(), &source, &visibility).unwrap();
    let fill = [11, 31, 55];
    let mut expected_direct = Vec::new();
    let retail: Vec<_> = (0..32)
        .map(|i| {
            // At this distance the change in irradiance across one texel is
            // well under a channel step. Evaluate the reference at its centre.
            let x = (i % 8) as f32 + 0.5 - position[0];
            let y = (i / 8) as f32 + 0.5 - position[1];
            let r = (x * x + y * y + position[2] * position[2]).sqrt();
            let gain = (1.0 - r / radius) * position[2] / r;
            let v = area_visible(i % 8, 8);
            expected_direct.push(K.map(|channel| byte(channel as f32 * gain)));
            from_fn(|channel| fill[channel] + byte(K[channel] as f32 * gain * v))
        })
        .collect();
    let mut samples = FitSamples::default();
    samples.add(&trace, &retail);
    let k = samples.fit(1).unwrap().k;
    assert!(
        k.into_iter()
            .zip(K)
            .all(|(value, target)| (value - target as f32).abs() <= 1.5)
    );
    let split = split_page(&trace, &retail, k, 1);
    for (i, texel) in split.texels.iter().enumerate() {
        assert_eq!(texel.visibility, byte(area_visible(i % 8, 8) * 255.));
        for channel in 0..3 {
            assert!(texel.direct[channel].abs_diff(expected_direct[i][channel]) <= 1);
            assert!(texel.lamp_off[channel].abs_diff(fill[channel]) <= 1);
            assert!(texel.lamp_on[channel].abs_diff(retail[i][channel]) <= 1);
        }
    }
}

#[test]
fn strong_light_on_a_grazing_surface_has_a_scale_above_one() {
    // Every observed D fits in the stored page range, although the
    // per-light scale before the surface's 0.25 cosine does not.
    let cell = |lit| Cell {
        mask: u16::MAX,
        gain: 0.25,
        lit: [lit; 2],
        ..Cell::default()
    };
    let trace = Trace {
        width: 2,
        cells: vec![cell(0), cell(16)],
    };
    let mut fit = FitSamples::default();
    fit.add(&trace, &[[16; 3], [112; 3]]);
    assert_eq!(fit.fit(1).unwrap().k, [384.; 3]);
}
