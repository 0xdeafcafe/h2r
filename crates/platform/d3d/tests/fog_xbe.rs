//! Checks the fog model in `d3d::nv2a` against the retail XBE: the three
//! pass-through programs `0x3f6f40` loads write the fog coordinate
//! `PassthroughFog` says, and the constants D3D's fog commit
//! (`0x00400000`) multiplies by are within tolerance of the closed forms
//! `d3d_fog` uses. Ignored by
//! default (the image is not in the repository, and nothing is copied
//! from it). Run with the path to a `default.xbe`, read-only:
//!
//! `H2_XBE=/abs/path/default.xbe cargo test -p d3d --test fog_xbe -- --ignored`

use d3d::GuestLayout;
use d3d::GuestMemory;
use d3d::nv2a::{PassthroughFog, exp_fog_scale, exp2_fog_scale, pretransformed_program};

/// NV2A Kelvin methods (nouveau's `nv20_3d` names).
const SET_FOG_MODE: u32 = 0x029C;
const SET_FOG_GEN_MODE: u32 = 0x02A0;
const SET_FOG_PARAMS: u32 = 0x09C0;
use d3d::runtime_glue::GuestAdapter;
use runtime::load_xbe;
use shaders::vsh::{self, OutReg, ProgramKind};

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn passthrough_fog_and_fog_constants_match_the_retail_image() {
    let Some(path) = std::env::var_os("H2_XBE") else {
        panic!("set H2_XBE to a retail default.xbe");
    };
    let image = load_xbe(&std::fs::read(path).unwrap()).unwrap();
    let mut guest = image.runtime.memory();
    let mem = GuestAdapter::new(&mut guest);

    // 0x3f6f40: table mode 0, then affine, then perspective.
    let choices = [
        PassthroughFog::select(0, true),
        PassthroughFog::select(3, true),
        PassthroughFog::select(3, false),
    ];
    assert_eq!(
        choices,
        [
            PassthroughFog::SpecularAlpha,
            PassthroughFog::Z,
            PassthroughFog::W
        ]
    );
    for ((addr, n), fog) in GuestLayout::HALO2
        .passthrough_programs
        .into_iter()
        .zip(choices)
    {
        let words = mem.read_words(addr, n as usize).unwrap();
        let guest_program = vsh::Program::from_words(ProgramKind::Normal, &words).unwrap();
        let ours = pretransformed_program([9, 10, 11, 12], fog);
        // rhw = 0 included: the guest's rcp gives infinity, and so must ours.
        for v0 in [
            [10.0f32, 20.0, 30.0, 0.5],
            [1.0, 2.0, 1234.5, 0.125],
            [3.0, 4.0, 5.0, 0.0],
        ] {
            let mut v = [[0.0f32, 0.0, 0.0, 1.0]; 16];
            v[0] = v0;
            v[4] = [0.1, 0.2, 0.3, 0.375];
            // The guest program scales by c[0] and offsets by c[1]; the fog
            // write reads neither.
            let mut c = [[0.0f32; 4]; 192];
            c[0] = [1.0, 1.0, 1.0, 1.0];
            let want = vsh::interp::run(&guest_program, &v, &mut c.clone()).get(OutReg::Fog)[0];
            c[0] = [0.5, 0.5, 0.0, 0.0];
            c[1] = [0.0; 4];
            let got = vsh::interp::run(&ours, &v, &mut c).get(OutReg::Fog)[0];
            assert_eq!(got, want, "{addr:#x} ({fog:?}) with v0 = {v0:?}");
        }
    }

    // The fog commit's constants, read from the image: 1.0 (0x45DBC0)
    // and 8192 for start == end (0x45DFF4) exactly, and the exp and exp2
    // density scales within a few parts per million of the closed forms
    // the model uses (D3D's own approximations).
    let f = |a: u32| f32::from_bits(mem.read_u32(a).unwrap());
    assert_eq!(f(0x0045_DBC0), 1.0);
    assert_eq!(f(0x0045_DFF4), 8192.0);
    let rel = |a: f32, b: f32| ((a as f64 - b as f64) / b as f64).abs();
    for (at, ours) in [
        (0x0045_E378, exp_fog_scale()),
        (0x0045_E374, exp2_fog_scale()),
    ] {
        let image = f(at);
        assert!(
            rel(image, ours) < 1e-5,
            "{at:#x}: image {image}, closed form {ours}"
        );
    }
    // And the push the commit builds at 0x4000f8: SET_FOG_GEN_MODE and
    // SET_FOG_ENABLE (two words), SET_FOG_MODE (one) and SET_FOG_PARAMS
    // (three), as NV2A method headers (count << 18 | method).
    let header = |count: u32, method: u32| count << 18 | method;
    let imm = |a: u32| mem.read_u32(a).unwrap();
    assert_eq!(imm(0x0040_00fa), header(2, SET_FOG_GEN_MODE));
    assert_eq!(imm(0x0040_010b), header(1, SET_FOG_MODE));
    assert_eq!(imm(0x0040_0115), header(3, SET_FOG_PARAMS));
}

/// The closed-form scales give D3D's fog functions through the hardware
/// formulas: `2^(16·k·d) = e^(−d)` and `2^(−32·(k·d)²) = e^(−d²)`.
#[test]
fn exp_fog_scales_give_e_powers() {
    for d in [0.0f64, 0.5, 1.0, 3.0] {
        let e = (16.0 * exp_fog_scale() as f64 * d).exp2();
        assert!((e - (-d).exp()).abs() < 1e-6, "exp at {d}");
        let s = exp2_fog_scale() as f64 * d;
        let e2 = (-32.0 * s * s).exp2();
        assert!((e2 - (-d * d).exp()).abs() < 1e-6, "exp2 at {d}");
    }
}
