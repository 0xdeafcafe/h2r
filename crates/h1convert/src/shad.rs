//! Halo 2 `shad` tags written from their definition (`tags::defs`,
//! `crates/tags/src/defs/shad.txt`) and Halo 1's shader, with no byte of
//! a Halo 2 shader instance copied.
//!
//! **The program.** A `shad` names a `stem`, the engine's shader program,
//! and fills the instance parameters the stem binds: bitmaps, colours and
//! vectors (postprocess `+0x04`, `+0x0c`, `+0x14`). Each Halo 1 shader
//! type draws with a retail stem chosen for it ([`Layout`]); every stem
//! used here but the three sky ones is in `shared.map`, so every base map
//! has it. We write no shader programs.
//!
//! **The parameters.** A [`Layout`] says, per instance parameter of its
//! stem, where the value comes from: a role of the Halo 1 shader (its base,
//! detail or bump map, a sky picture, mask or tile) with that map's tiling,
//! or a stem constant. D (`tests/shad_rebuild.rs`, every retail `shad`
//! of the stem on the Halo 2 disc): the roles are the stages the stem's
//! passes sample (`tags::material`), tiling is the vector bound to
//! `c[18 + stage]` ([`crate::textures::stage_vector`]) or, on the
//! transparent stems, a u row and a v row; a constant is a value every
//! retail instance of the stem holds (the default bitmaps, the
//! `linear_corner_fade` ramp, the unit light vectors), or for a
//! per-instance tint, the untinted value (white).
//!
//! **What else the tag holds**, from shad.txt:
//!
//! - the root parameter block (one element) and the postprocess mirrors
//!   of it (`parameter_bitmaps`, `parameter_colors`, `parameter_scalars`)
//!   kept consistent: the base map (or self-illumination map) names the
//!   bitmap the stem draws as the picture, the mirror entry indexes it;
//! - `bitmap_images`, one entry per non-null postprocess bitmap in order,
//!   its image the number of earlier entries naming the same bitmap (V);
//! - `slit_ref_34` the `slit` group with no tag, as on every retail tag;
//! - each bitmap's `lod_offset` from its size ([`lod_offset`]);
//! - no animation: the LOD/layer/pass/implementation tree and the
//!   animated properties are empty, which the engine reads as "not
//!   animated" (`0x000174d0` bounds-checks each level). Halo 1's map
//!   animation (scrolling clouds, water) is not converted yet.

use std::collections::HashMap;
use std::io::{Read, Seek};

use map::build::Block;
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, Target, Val};
use tags::shader::SHAD;
use tags::{DecodedTag, TagData};

use crate::{Error, Result};

/// What a Halo 1 shader supplies to a stem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// The picture: a surface's base map, a transparent shader's or a sky
    /// layer's map.
    Base,
    /// A surface's detail map; with [`Role::Secondary`], Halo 1's primary
    /// detail, shown where the base map's alpha is 0.
    Detail,
    /// Halo 1's secondary detail map, shown where the base map's alpha is
    /// 1 (`senv` type blended).
    Secondary,
    /// Halo 1's micro detail map, multiplied over the blend.
    Micro,
    Bump,
    /// A reflection cube map (Halo 1 `senv +0x324`).
    Cube,
    /// A cloud layer's mask (`two_alpha_clouds` stage 1).
    Mask,
    /// A cloud layer's tiled texture (stage 2).
    Tile,
}

/// A default bitmap of the base (every Halo 2 map has these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fixed {
    /// A flat bump map.
    DefaultVector,
    /// A plain (mid grey) detail map.
    DefaultDetail,
    ColorWhite,
    DefaultAdditive,
    DefaultMultiplicative,
    /// A black cube map: no reflection.
    DefaultCube,
    /// The ramp the opaque stems' fade passes draw.
    CornerFade,
    /// The rasterizer's plain 2D texture (`water_pool`'s bitmap 2, which
    /// its pass doesn't bind).
    Default2d,
    /// The water's facing-to-grazing ramp (`water_pool`'s stage 3; its
    /// alpha weighs the reflection against the refracted scene).
    Fresnel,
    /// White with alpha 0 (`water_pool`'s bitmap 4, unbound).
    ColorWhiteAlphaBlack,
}

impl Fixed {
    pub const ALL: [Fixed; 10] = [
        Fixed::DefaultVector,
        Fixed::DefaultDetail,
        Fixed::ColorWhite,
        Fixed::DefaultAdditive,
        Fixed::DefaultMultiplicative,
        Fixed::DefaultCube,
        Fixed::CornerFade,
        Fixed::Default2d,
        Fixed::Fresnel,
        Fixed::ColorWhiteAlphaBlack,
    ];

    /// The bitmap's tag path: on every Halo 2 map (V: all 30 disc maps),
    /// except the water's three, which are on the maps with a
    /// `water_pool` (Delta Approach, Beaver Creek and Coagulation among
    /// them; [`Library::has`] falls back where they're missing).
    pub fn path(self) -> &'static str {
        match self {
            Fixed::DefaultVector => "shaders\\default_bitmaps\\bitmaps\\default_vector",
            Fixed::DefaultDetail => "shaders\\default_bitmaps\\bitmaps\\default_detail",
            Fixed::ColorWhite => "shaders\\default_bitmaps\\bitmaps\\color_white",
            Fixed::DefaultAdditive => "shaders\\default_bitmaps\\bitmaps\\default_additive",
            Fixed::DefaultMultiplicative => {
                "shaders\\default_bitmaps\\bitmaps\\default_multiplicative"
            }
            Fixed::DefaultCube => "shaders\\default_bitmaps\\bitmaps\\default_additive_cube_map",
            Fixed::CornerFade => "rasterizer\\linear_corner_fade",
            Fixed::Default2d => "rasterizer\\default_2d",
            Fixed::Fresnel => "scenarios\\bitmaps\\water\\fresnel",
            Fixed::ColorWhiteAlphaBlack => {
                "shaders\\default_bitmaps\\bitmaps\\color_white_alpha_black"
            }
        }
    }
}

/// Where one instance bitmap comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bm {
    /// The Halo 1 shader's map in this role, else the default.
    Role(Role, Fixed),
    Fixed(Fixed),
    Null,
}

/// Where one instance vector comes from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Vector {
    Const([f32; 4]),
    /// `[u, v, 0, 0]`: the role's tiling, bound to `c[18 + stage]`.
    Tile(Role),
    /// `[u, 0, 0, 0]` and `[0, v, 0, 0]`: the u and v rows of a
    /// transparent stem's texture transform.
    URow(Role),
    VRow(Role),
    /// `[0, 0, u, v]`: the role's tiling in a vector's second half
    /// (`tex_bump_detail_blend_detail` binds `c[20]` from vector 1's
    /// first half and vector 2's second).
    Zw(Role),
    /// Colour `k`'s red, green and blue as floats (the opaque stems bind
    /// a colour to a pixel constant and its float copy to a vertex
    /// default; V every retail `tex_bump` instance has vector 7 = colour 0
    /// and vector 8 = colour 1).
    ColorOf(usize),
}

/// Which root parameter names the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    /// `base_map` (opaque stems).
    BaseMap,
    /// `self_illum_map` with colour `color_10` white and `scalar_1c` 1
    /// (the additive and self-illuminated stems; V `one_add_illum`).
    SelfIllum,
    /// Every root parameter null (the sky stems).
    None,
}

/// A stem and the source of each of its instance parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub stem: &'static str,
    /// `shad +0x14`: 1 on the opaque stems, 5 on the transparent (V).
    pub word_14: u16,
    /// `shad +0x3e`, `+0x40`, the engine integer inputs 4 and 5 that pick
    /// combiner variants: the commonest retail pair (D).
    pub ints: [u16; 2],
    pub bitmaps: &'static [Bm],
    pub colors: &'static [u32],
    pub vectors: &'static [Vector],
    pub root: Root,
    /// The instance animation tree an animated instance of the stem has,
    /// if this converts any animation onto it.
    pub animation: Option<&'static AnimTree>,
}

/// An animated stem's instance tree (postprocess `+0x1c` to `+0x54`,
/// docs/formats/shader-instances.md): one LOD whose mask names the
/// animated layer, every layer's pass range, one implementation's five
/// animation ranges, and the bindings, all reading property range 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnimTree {
    /// The template's layer count (the LOD's layer range).
    pub layers: u16,
    /// The one layer with a pass (and animation).
    pub animated_layer: u16,
    /// Bitmap, render-state, stage-state, pixel- and vertex-constant
    /// ranges (start, count) into [`Self::bindings`].
    pub implementation: [(u16, u16); 5],
    /// (target, param 1, param 2, property range).
    pub bindings: &'static [[u8; 4]],
}

/// `two_alpha_clouds`' animated tree. V (all 14 animated retail
/// instances): layer 15 of 25 animated; the tile's texture rows `c[22]`
/// and `c[23]` (vertex targets 212 and 245, parameter 5, a texture
/// transform) and bitmap target 1, each from the ten transform
/// components ([`TextureScroll`]).
pub const CLOUDS_TREE: AnimTree = AnimTree {
    layers: 25,
    animated_layer: 15,
    implementation: [(2, 1), (2, 0), (2, 0), (0, 0), (0, 2)],
    bindings: &[[212, 5, 0, 0], [245, 5, 0, 0], [1, 0, 0, 0]],
};

/// `water_pool`'s animated tree. V (all 14 retail instances,
/// `tests/shad_rebuild.rs`, `water_pool_instances_share_one_tree`): layer
/// 15 of 25 animated, like the clouds; one vertex binding (target 178,
/// parameter 1: `c[20]` whole, the ripple map's transform) and one bitmap
/// binding (target 0, the ripple map's frame), both reading the ten
/// components [`TextureScroll`] writes. Retail: component 0 the frame, an
/// identity over 0..17 every 0.5 to 0.75 s (`water_mip` has 17 frames); 1
/// and 2 the ripple's u and v scale (vector 0's x and y; periodic around
/// them on two); 3 one; 4 and 5 its offsets (vector 0's z and w); 6 to 9
/// zero. E `0x00017960` (vertex-constant animations): the binding word
/// `0x01b2` is register 2 of bank 1 (`c[20]`), copy mode 13 (xyzw), and
/// `0x0001b790` writes it as (u scale, v scale, u offset, v offset) when
/// the transform's rotation is 0.
pub const WATER_TREE: AnimTree = AnimTree {
    layers: 25,
    animated_layer: 15,
    implementation: [(1, 1), (1, 0), (1, 0), (0, 0), (0, 1)],
    bindings: &[[178, 1, 0, 0], [0, 0, 0, 0]],
};

/// A texture transform's scroll on one axis: the offset runs linearly
/// from `from` to `to` every `period` seconds of game time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slide {
    pub period: f32,
    pub from: f32,
    pub to: f32,
}

/// An animated texture transform: the ten components a [`CLOUDS_TREE`]
/// binding reads. V (every animated retail instance of the five
/// transparent stems: components 1 and 2 hold the map's u and v scale,
/// equal to its rows' (`sky_shq_trenchcloud`: 1 and 2); 4 and 5 are
/// the time-driven ones on the scrolling clouds and waterfalls; 0 and 3
/// are 1, 6 to 9 are 0, on all of them): scale and u/v offsets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextureScroll {
    pub scale: [f32; 2],
    pub offset: [Option<Slide>; 2],
}

impl TextureScroll {
    /// The ten properties, component `k` from property `k`: each
    /// `(period, function blob)`; a constant `c` is a constant function
    /// (type 1) whose range starts at `c`, a slide a linear transition
    /// (type 2, shape 0, from 0 to 1) mapped onto `[from, to]`, as the
    /// retail clouds have them.
    pub fn properties(&self) -> Vec<(f32, Vec<u8>)> {
        use tags::effects::function::{Function, FunctionType};
        let f = |kind, range: [f32; 2], payload: &[f32]| {
            Function {
                kind,
                flags: 0,
                sub_types: [0, 0],
                range,
                unknown_0c: [0; 8],
                payload: payload.iter().flat_map(|v| v.to_le_bytes()).collect(),
            }
            .to_bytes()
        };
        let constant = |c: f32| (1.0, f(FunctionType::Constant, [c, 1.0], &[1.0, 1.0]));
        let offset = |s: Option<Slide>| match s {
            Some(s) => (
                s.period,
                f(FunctionType::Transition, [s.from, s.to], &[0.0, 1.0, 0.0, 1.0]),
            ),
            None => constant(0.0),
        };
        vec![
            constant(1.0),
            constant(self.scale[0]),
            constant(self.scale[1]),
            constant(1.0),
            offset(self.offset[0]),
            offset(self.offset[1]),
            constant(0.0),
            constant(0.0),
            constant(0.0),
            constant(0.0),
        ]
    }
}

const WHITE: u32 = 0x00ff_ffff;
const OPAQUE_ALPHA: u32 = 0xff00_0000;
const W1: [f32; 4] = [1.0, 1.0, 1.0, 0.0];
const A1: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const U1: [f32; 4] = [1.0, 0.0, 0.0, 0.0];
const V1: [f32; 4] = [0.0, 1.0, 0.0, 0.0];

/// Opaque bumped, lightmapped surfaces: stage 0 base (`c[18]` vector 1),
/// stage 1 detail (`c[19]` vector 2), stage 2 bump (`c[20]` vector 0),
/// stage 3 the lightmap. Vectors 3 to 6 are the same in all 498 retail
/// instances (V). Colours 0 and 1 (vectors 7 and 8) are the specular
/// colours, which the base map's alpha masks: black, as on 39 of 498
/// retail instances, since a Halo 1 base alpha is a blend or cut-out
/// mask, not a specular one (white lit Blood Gulch's ground flat).
const OPAQUE_BITMAPS: [Bm; 5] = [
    Bm::Role(Role::Bump, Fixed::DefaultVector),
    Bm::Null,
    Bm::Role(Role::Base, Fixed::ColorWhite),
    Bm::Role(Role::Detail, Fixed::DefaultDetail),
    Bm::Fixed(Fixed::CornerFade),
];
const OPAQUE_VECTORS: [Vector; 9] = [
    Vector::Tile(Role::Bump),
    Vector::Tile(Role::Base),
    Vector::Tile(Role::Detail),
    Vector::Const([0.39, 0.0, 0.93, 0.0]),
    Vector::Const([-0.276, 0.0, 0.0, 0.0]),
    Vector::Const([0.0, -0.276, 0.0, 0.0]),
    Vector::Const([0.0, 0.0, -0.93, 0.0]),
    Vector::ColorOf(0),
    Vector::ColorOf(1),
];

pub const TEX_BUMP: Layout = Layout {
    stem: "shaders\\shader_templates\\opaque\\tex_bump",
    word_14: 1,
    ints: [1, 0],
    bitmaps: &OPAQUE_BITMAPS,
    colors: &[0, 0],
    vectors: &OPAQUE_VECTORS,
    root: Root::BaseMap,
    animation: None,
};

/// Opaque bumped surfaces with Halo 1's detail blend: stage 0 the base
/// (bitmap 1, `c[18]` vector 3), stage 1 the detail shown where the base
/// alpha is 0 (bitmap 2, `c[19]` vector 0; Halo 1's primary), stage 2
/// the one shown where it is 1 (bitmap 3, `c[20].xy` vector 1; the
/// secondary), stage 3 a micro detail
/// (bitmap 4, `c[20].zw` vector 2); layer 1 is `lerp(t1, t2, t0.a) ×
/// t0 × 2 × t3 × 2`, Halo 1's `senv` blend with biased multiplies
/// (`tests/shad_rebuild.rs`). The bump is bitmap 0 (`c[20]` vector 4 in
/// the lighting layer). Colours 0 and 1 white and black (the final
/// combiner; all 27 retail instances), 2 and 3 the specular colours,
/// vectors 7 and 8 their copies; 3 of 27 retail instances (Delta
/// Temple's stone) have them black, the value written: Halo 1's
/// specular is not carried.
pub const TEX_BUMP_DETAIL_BLEND_DETAIL: Layout = Layout {
    stem: "shaders\\shader_templates\\opaque\\tex_bump_detail_blend_detail",
    word_14: 1,
    ints: [0, 1],
    bitmaps: &[
        Bm::Role(Role::Bump, Fixed::DefaultVector),
        Bm::Role(Role::Base, Fixed::ColorWhite),
        Bm::Role(Role::Detail, Fixed::DefaultDetail),
        Bm::Role(Role::Secondary, Fixed::DefaultDetail),
        Bm::Role(Role::Micro, Fixed::DefaultDetail),
        Bm::Fixed(Fixed::CornerFade),
        Bm::Fixed(Fixed::DefaultDetail),
    ],
    colors: &[WHITE, 0, 0, 0],
    vectors: &[
        Vector::Tile(Role::Detail),
        Vector::Tile(Role::Secondary),
        Vector::Zw(Role::Micro),
        Vector::Tile(Role::Base),
        Vector::Tile(Role::Bump),
        Vector::Const([0.276, 0.276, 0.93, 0.0]),
        Vector::Const([0.39, 0.0, 0.93, 0.0]),
        Vector::ColorOf(2),
        Vector::ColorOf(3),
        Vector::Const([1.0, 1.0, 0.0, 0.0]),
    ],
    root: Root::BaseMap,
    animation: None,
};

/// Opaque bumped surfaces with a reflection: stage 0 the base (bitmap 2,
/// `c[18]` vector 1), stage 1 the detail (bitmap 3, `c[19]` vector 0);
/// the lighting layer reads the bump (bitmap 0, `c[20]` vector 2) and the
/// cube map (bitmap 4). There colours 1 and 2 are the reflection's tint
/// facing the eye and at a grazing angle, lerped by the bump normal's dot
/// with the eye vector, and colours 3 and 4's alphas its brightness the
/// same way; the reflection is masked by the colour pass's alpha (base
/// × detail). Colours 5 and 6 (vectors 5 and 6 their copies) tint a
/// lightmap highlight weighted by the cube map's alpha: black, Halo 1
/// has none (`tests/shad_rebuild.rs`, `env_reflection_is_halo1s_rule`).
pub const TEX_BUMP_ENV: Layout = Layout {
    stem: "shaders\\shader_templates\\opaque\\tex_bump_env",
    word_14: 1,
    ints: [1, 0],
    bitmaps: &[
        Bm::Role(Role::Bump, Fixed::DefaultVector),
        Bm::Fixed(Fixed::DefaultVector),
        Bm::Role(Role::Base, Fixed::ColorWhite),
        Bm::Role(Role::Detail, Fixed::DefaultDetail),
        Bm::Role(Role::Cube, Fixed::DefaultCube),
        Bm::Fixed(Fixed::CornerFade),
    ],
    colors: &[WHITE, WHITE, WHITE, OPAQUE_ALPHA, OPAQUE_ALPHA, 0, 0],
    vectors: &[
        Vector::Tile(Role::Detail),
        Vector::Tile(Role::Base),
        Vector::Tile(Role::Bump),
        Vector::Const([0.276, 0.276, 0.93, 0.0]),
        Vector::Const([0.39, 0.0, 0.93, 0.0]),
        Vector::ColorOf(5),
        Vector::ColorOf(6),
    ],
    root: Root::BaseMap,
    animation: None,
};

/// [`TEX_BUMP`]'s layout with the base map's alpha cutting the surface
/// out; bitmap 1 is `color_white` in 34 of 47 retail instances.
pub const TEX_BUMP_ALPHA_TEST: Layout = Layout {
    stem: "shaders\\shader_templates\\opaque\\tex_bump_alpha_test_single_pass",
    word_14: 1,
    ints: [1, 0],
    bitmaps: &[
        Bm::Role(Role::Bump, Fixed::DefaultVector),
        Bm::Fixed(Fixed::ColorWhite),
        Bm::Role(Role::Base, Fixed::ColorWhite),
        Bm::Role(Role::Detail, Fixed::DefaultDetail),
        Bm::Fixed(Fixed::CornerFade),
    ],
    colors: &[0, 0],
    vectors: &OPAQUE_VECTORS,
    root: Root::BaseMap,
    animation: None,
};

/// Self-illuminated: one pass drawing bitmap 0 (`c[18]` vector 0).
pub const ILLUM: Layout = Layout {
    stem: "shaders\\shader_templates\\opaque\\illum",
    word_14: 1,
    ints: [0, 0],
    bitmaps: &[Bm::Role(Role::Base, Fixed::ColorWhite), Bm::Null],
    colors: &[WHITE],
    vectors: &[
        Vector::Tile(Role::Base),
        Vector::Const(U1),
        Vector::Const(V1),
        Vector::Const([0.0; 4]),
        Vector::Const([0.0; 4]),
    ],
    root: Root::SelfIllum,
    animation: None,
};

/// Additive: stage 1 bitmap 0, its rows vectors 0 and 1, white tints.
pub const ONE_ADD_ILLUM: Layout = Layout {
    stem: "shaders\\shader_templates\\transparent\\one_add_illum",
    word_14: 5,
    ints: [0, 0],
    bitmaps: &[Bm::Role(Role::Base, Fixed::ColorWhite)],
    colors: &[WHITE],
    vectors: &[
        Vector::URow(Role::Base),
        Vector::VRow(Role::Base),
        Vector::Const(W1),
        Vector::Const(W1),
        Vector::Const(A1),
        Vector::Const(A1),
    ],
    root: Root::SelfIllum,
    animation: None,
};

/// Alpha blended: stage 0 a reflection cube (black: none; a Halo 1 cube
/// map when the shader's map is one), stage 1 a multiplier (neutral),
/// stages 2 and 3 the picture (bitmap 2) and the additive default (bitmap
/// 4); the picture's rows vectors 4 and 5. V: bitmap 0 is a cube on every
/// retail instance, 1 to 4 2D (`every_rebuilt_bitmap_is_its_slots_retail_kind`).
pub const ONE_ALPHA_ENV: Layout = Layout {
    stem: "shaders\\shader_templates\\transparent\\one_alpha_env",
    word_14: 5,
    ints: [0, 0],
    bitmaps: &[
        Bm::Role(Role::Cube, Fixed::DefaultCube),
        Bm::Fixed(Fixed::DefaultMultiplicative),
        Bm::Role(Role::Base, Fixed::DefaultAdditive),
        Bm::Fixed(Fixed::ColorWhite),
        Bm::Fixed(Fixed::DefaultAdditive),
    ],
    colors: &[WHITE, OPAQUE_ALPHA, WHITE, WHITE],
    vectors: &[
        Vector::Const(W1),
        Vector::Const(W1),
        Vector::Const(A1),
        Vector::Const(A1),
        Vector::URow(Role::Base),
        Vector::VRow(Role::Base),
        Vector::Const(U1),
        Vector::Const(V1),
        Vector::Const(U1),
        Vector::Const(V1),
    ],
    root: Root::None,
    animation: None,
};

/// The sky picture (`crate::sky_shaders`): stage 3 bitmap 2, every row
/// pair at the picture's scale.
pub const SKY_PICTURE: Layout = Layout {
    stem: "shaders\\shader_templates\\transparent\\sky_one_alpha_env",
    word_14: 5,
    ints: [0, 0],
    bitmaps: &[
        Bm::Fixed(Fixed::DefaultCube),
        Bm::Fixed(Fixed::DefaultMultiplicative),
        Bm::Role(Role::Base, Fixed::DefaultAdditive),
        Bm::Fixed(Fixed::DefaultAdditive),
    ],
    colors: &[WHITE, OPAQUE_ALPHA, WHITE, WHITE],
    vectors: &[
        Vector::URow(Role::Base),
        Vector::VRow(Role::Base),
        Vector::URow(Role::Base),
        Vector::VRow(Role::Base),
        Vector::URow(Role::Base),
        Vector::VRow(Role::Base),
    ],
    root: Root::None,
    animation: None,
};

/// A cloud layer: stage 2 the tile (bitmap 0, rows 0 and 1), stage 1 the
/// mask (bitmap 1, rows 2 and 3).
pub const SKY_CLOUDS: Layout = Layout {
    stem: "shaders\\shader_templates\\transparent\\two_alpha_clouds",
    word_14: 5,
    ints: [0, 0],
    bitmaps: &[
        Bm::Role(Role::Tile, Fixed::ColorWhite),
        Bm::Role(Role::Mask, Fixed::ColorWhite),
        Bm::Fixed(Fixed::ColorWhite),
    ],
    colors: &[],
    vectors: &[
        Vector::URow(Role::Tile),
        Vector::VRow(Role::Tile),
        Vector::URow(Role::Mask),
        Vector::VRow(Role::Mask),
    ],
    root: Root::None,
    animation: Some(&CLOUDS_TREE),
};

/// An additive sky layer: stage 1 the picture (bitmap 0) times stage 2
/// white (bitmap 1) times colour 0, doubled: colour 0 half grey gives the
/// picture back; vector 4 the tint, white.
pub const SKY_ADDITIVE: Layout = Layout {
    stem: "shaders\\shader_templates\\transparent\\one_add_illum_detail",
    word_14: 5,
    ints: [0, 0],
    bitmaps: &[
        Bm::Role(Role::Base, Fixed::ColorWhite),
        Bm::Fixed(Fixed::ColorWhite),
    ],
    colors: &[0x0080_8080],
    vectors: &[
        Vector::URow(Role::Base),
        Vector::VRow(Role::Base),
        Vector::Const(U1),
        Vector::Const(V1),
        Vector::Const(W1),
        Vector::Const(W1),
        Vector::Const(A1),
        Vector::Const([0.0; 4]),
    ],
    root: Root::None,
    animation: None,
};

/// Water (`swat`): stage 0 the ripple map (bitmap 0) at `c[20]`'s
/// transform (vector 0: its u and v scale), stages 1 and 2 the reflection
/// (bitmap 1) and the scene behind the water, each offset by the ripple
/// (bump environment), stage 3 the fresnel ramp, whose alpha weighs the
/// reflection against the scene (`h2 shader` on Coagulation's
/// `coag_water_pool`). Bitmaps 2 and 4 aren't bound by the pass; they
/// hold retail's commonest. V (all 14 retail instances): integer inputs
/// (1, 0) on 12, bitmap 1 a cube on 5 and 2D on 9, no colours, one
/// vector.
pub const WATER_POOL: Layout = Layout {
    stem: "shaders\\shader_templates\\water\\water_pool",
    word_14: 5,
    ints: [1, 0],
    bitmaps: &[
        Bm::Role(Role::Bump, Fixed::DefaultVector),
        Bm::Role(Role::Cube, Fixed::DefaultCube),
        Bm::Fixed(Fixed::Default2d),
        Bm::Fixed(Fixed::Fresnel),
        Bm::Fixed(Fixed::ColorWhiteAlphaBlack),
    ],
    colors: &[],
    vectors: &[Vector::Tile(Role::Bump)],
    root: Root::None,
    animation: Some(&WATER_TREE),
};

/// Every layout.
pub const LAYOUTS: [&Layout; 11] = [
    &TEX_BUMP,
    &TEX_BUMP_ENV,
    &TEX_BUMP_DETAIL_BLEND_DETAIL,
    &TEX_BUMP_ALPHA_TEST,
    &ILLUM,
    &ONE_ADD_ILLUM,
    &ONE_ALPHA_ENV,
    &SKY_PICTURE,
    &SKY_CLOUDS,
    &SKY_ADDITIVE,
    &WATER_POOL,
];

/// A bitmap's `lod_offset` (postprocess bitmap `+8`) from its first
/// image's size: `0.960906 × log2(width × height) / 4`. D: on every
/// retail instance of the stems above whose bitmap is square, the offset
/// is this of its size (`tests/shad_rebuild.rs`); 0 for an empty size.
pub fn lod_offset(width: u16, height: u16) -> f32 {
    let area = f32::from(width) * f32::from(height);
    if area < 1.0 {
        return 0.0;
    }
    LOD_UNIT * area.log2() / 4.0
}

/// The retail step of [`lod_offset`] per doubling of a side.
pub const LOD_UNIT: f32 = 0.960_906;

/// One bound map: its datum in the set, tiling (u, v) and LOD offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bound {
    pub id: TagId,
    pub scale: [f32; 2],
    pub lod_offset: f32,
}

/// Values a shader sets over its layout's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extras {
    /// (colour index, ARGB) replacing the layout's colour.
    pub colors: Vec<(usize, u32)>,
    /// The scrolling of the layout's animated map
    /// ([`Layout::animation`]); `None`: not animated.
    pub scroll: Option<[Option<Slide>; 2]>,
}

impl Extras {
    /// `layout`'s colours with these over them.
    pub fn colors_of(&self, layout: &Layout) -> Vec<u32> {
        let mut c = layout.colors.to_vec();
        for &(k, v) in &self.colors {
            if let Some(x) = c.get_mut(k) {
                *x = v;
            }
        }
        c
    }
}

/// The base's stems and default bitmaps by name.
#[derive(Debug, Clone, Default)]
pub struct Library {
    stems: HashMap<&'static str, TagId>,
    fixed: HashMap<Fixed, Bound>,
}

impl Library {
    /// Finds every [`LAYOUTS`] stem and [`Fixed`] bitmap on `base`.
    pub fn read<R: Read + Seek>(base: &mut Map<R>) -> Self {
        let mut out = Self::default();
        let find = |base: &Map<R>, group: &[u8; 4], name: &str| {
            base.tags()
                .iter()
                .find(|t| {
                    t.group == FourCC(*group)
                        && t.location.is_some()
                        && t.name.as_deref() == Some(name)
                })
                .map(|t| t.id)
        };
        for l in LAYOUTS {
            if let Some(id) = find(base, b"stem", l.stem) {
                out.stems.insert(l.stem, id);
            }
        }
        for f in Fixed::ALL {
            let Some(id) = find(base, b"bitm", f.path()) else {
                continue;
            };
            let lod = match DecodedTag::read(base, id).map(|d| d.data) {
                Ok(TagData::Bitmap(b)) => b
                    .images
                    .first()
                    .map_or(0.0, |i| lod_offset(i.dimensions[0], i.dimensions[1])),
                _ => 0.0,
            };
            out.fixed.insert(
                f,
                Bound {
                    id,
                    scale: [1.0, 1.0],
                    lod_offset: lod,
                },
            );
        }
        out
    }

    /// A library of the given stems and default bitmaps (tests).
    pub fn new(stems: &[(&'static str, TagId)], fixed: &[(Fixed, Bound)]) -> Self {
        Self {
            stems: stems.iter().copied().collect(),
            fixed: fixed.iter().copied().collect(),
        }
    }

    /// The stem `layout` draws with on the base.
    pub fn stem(&self, layout: &Layout) -> Option<TagId> {
        self.stems.get(layout.stem).copied()
    }

    /// Whether the base has `layout`'s stem and every default bitmap it
    /// needs.
    pub fn has(&self, layout: &Layout) -> bool {
        self.stem(layout).is_some()
            && layout.bitmaps.iter().all(|b| match b {
                Bm::Role(_, f) | Bm::Fixed(f) => self.fixed.contains_key(f),
                Bm::Null => true,
            })
    }

    /// The `shad` record for `layout` drawing `maps`.
    pub fn record(&self, layout: &Layout, maps: &HashMap<Role, Bound>) -> Result<Record> {
        self.record_with(layout, maps, &Extras::default())
    }

    /// [`Self::record`] with per-shader values over the layout's.
    pub fn record_with(
        &self,
        layout: &Layout,
        maps: &HashMap<Role, Bound>,
        extras: &Extras,
    ) -> Result<Record> {
        let colors = extras.colors_of(layout);
        let stem = self.stem(layout).ok_or(Error::Invalid("the base lacks the stem"))?;
        let fixed = |f: &Fixed| {
            self.fixed
                .get(f)
                .copied()
                .ok_or(Error::Invalid("the base lacks a default bitmap"))
        };
        let mut bitmaps: Vec<Option<Bound>> = Vec::new();
        for b in layout.bitmaps {
            bitmaps.push(match b {
                Bm::Role(r, f) => Some(match maps.get(r) {
                    Some(m) => *m,
                    None => fixed(f)?,
                }),
                Bm::Fixed(f) => Some(fixed(f)?),
                Bm::Null => None,
            });
        }
        let scale = |r: &Role| maps.get(r).map_or([1.0, 1.0], |m| m.scale);
        let vectors: Vec<[f32; 4]> = layout
            .vectors
            .iter()
            .map(|v| match v {
                Vector::Const(c) => *c,
                Vector::Tile(r) => {
                    let [u, v] = scale(r);
                    [u, v, 0.0, 0.0]
                }
                Vector::URow(r) => [scale(r)[0], 0.0, 0.0, 0.0],
                Vector::VRow(r) => [0.0, scale(r)[1], 0.0, 0.0],
                Vector::Zw(r) => {
                    let [u, v] = scale(r);
                    [0.0, 0.0, u, v]
                }
                Vector::ColorOf(k) => {
                    let c = colors.get(*k).copied().unwrap_or(0);
                    let ch = |s: u32| ((c >> s) & 0xff) as f32 / 255.0;
                    [ch(16), ch(8), ch(0), 0.0]
                }
            })
            .collect();
        let picture = match layout.root {
            Root::None => None,
            _ => layout
                .bitmaps
                .iter()
                .position(|b| matches!(b, Bm::Role(Role::Base, _))),
        };
        let scroll = match (layout.animation, extras.scroll) {
            (Some(_), Some(offset)) if offset.iter().any(Option::is_some) => {
                let r = layout.bitmaps.iter().find_map(|b| match b {
                    Bm::Role(r, _) if *r != Role::Mask && *r != Role::Cube => Some(*r),
                    _ => None,
                });
                Some(TextureScroll {
                    scale: r.map_or([1.0, 1.0], |r| scale(&r)),
                    offset,
                })
            }
            _ => None,
        };
        let mut rec = build_record(layout, stem, &bitmaps, &colors, &vectors, picture)?;
        if let (Some(tree), Some(scroll)) = (layout.animation, scroll) {
            animate(&mut rec, tree, &scroll)?;
        }
        Ok(rec)
    }

    /// The `shad` tree for `layout` drawing `maps`.
    pub fn build(&self, layout: &Layout, maps: &HashMap<Role, Bound>) -> Result<Block> {
        self.build_with(layout, maps, &Extras::default())
    }

    /// [`Self::build`] with per-shader values over the layout's.
    pub fn build_with(
        &self,
        layout: &Layout,
        maps: &HashMap<Role, Bound>,
        extras: &Extras,
    ) -> Result<Block> {
        let rec = self.record_with(layout, maps, extras)?;
        defs::to_built(shad_def(), &rec, &mut |_| None)
            .map_err(|e| Error::Textures(format!("shad: {e}")))
    }
}

/// A u16 range: start in the low 9 bits, count in the high 7.
fn range16(start: u16, count: u16) -> Val {
    Val::Int(i64::from(start | (count << 9)))
}

/// Writes `tree` with `scroll`'s properties into the record's
/// postprocess.
fn animate(rec: &mut Record, tree: &AnimTree, scroll: &TextureScroll) -> Result<()> {
    let def = shad_def();
    let ppdef = child(def, "postprocess")?;
    let Some(Val::Block(pps)) = rec.at("postprocess").cloned() else {
        return Err(Error::Invalid("shad record lacks its postprocess"));
    };
    let mut pp = pps.into_iter().next().ok_or(Error::Invalid("no postprocess"))?;
    let mk = |path: &str, fields: &[(&str, Val)]| -> Result<Record> {
        let mut r = defs::default_record(child(ppdef, path)?);
        for (k, v) in fields {
            set(&mut r, k, v.clone())?;
        }
        Ok(r)
    };
    let lod = mk(
        "lods",
        &[
            ("layer_mask", Val::Int(1i64 << tree.animated_layer)),
            ("layers", range16(0, tree.layers)),
        ],
    )?;
    set(&mut pp, "lods", Val::Block(vec![lod]))?;
    let layers = (0..tree.layers)
        .map(|k| {
            let v = match k.cmp(&tree.animated_layer) {
                std::cmp::Ordering::Less => range16(0, 0),
                std::cmp::Ordering::Equal => range16(0, 1),
                std::cmp::Ordering::Greater => range16(1, 0),
            };
            mk("layers", &[("passes", v)])
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut pp, "layers", Val::Block(layers))?;
    set(
        &mut pp,
        "passes",
        Val::Block(vec![mk("passes", &[("implementations", range16(0, 1))])?]),
    )?;
    let names = [
        "bitmap_animations",
        "render_state_animations",
        "stage_state_animations",
        "pixel_constant_animations",
        "vertex_constant_animations",
    ];
    let fields: Vec<(&str, Val)> = names
        .iter()
        .zip(tree.implementation)
        .map(|(n, (a, b))| (*n, range16(a, b)))
        .collect();
    set(&mut pp, "implementations", Val::Block(vec![mk("implementations", &fields)?]))?;
    let props = scroll.properties();
    let precs = props
        .iter()
        .map(|(period, blob)| {
            mk(
                "properties",
                &[("input_b", Val::Float(*period)), ("function", Val::Data(blob.clone()))],
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let n = props.len() as u16;
    set(&mut pp, "properties", Val::Block(precs))?;
    let refs = (0..n)
        .map(|k| {
            mk(
                "property_refs",
                &[("property", Val::Int(i64::from(k))), ("component", Val::Int(i64::from(k)))],
            )
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut pp, "property_refs", Val::Block(refs))?;
    set(
        &mut pp,
        "property_ranges",
        Val::Block(vec![mk("property_ranges", &[("property_refs", range16(0, n))])?]),
    )?;
    let binds = tree
        .bindings
        .iter()
        .map(|b| {
            mk(
                "animations",
                &[
                    ("target", Val::Int(i64::from(b[0]))),
                    ("param_1", Val::Int(i64::from(b[1]))),
                    ("param_2", Val::Int(i64::from(b[2]))),
                    ("property_range", Val::Int(i64::from(b[3]))),
                ],
            )
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut pp, "animations", Val::Block(binds))?;
    set(rec, "postprocess", Val::Block(vec![pp]))
}

/// The `shad` definition.
pub fn shad_def() -> &'static tags::defs::StructDef {
    Defs::builtin().group(SHAD).expect("shad is defined")
}

fn datum(id: Option<TagId>) -> Val {
    Val::Datum(match id {
        Some(id) if id != TagId::NULL => Target::Id(id),
        _ => Target::Null,
    })
}

fn set(r: &mut Record, path: &str, v: Val) -> Result<()> {
    r.set(path, v)
        .map_err(|e| Error::Textures(format!("shad record: {e}")))
}

fn child<'d>(def: &'d tags::defs::StructDef, path: &str) -> Result<&'d tags::defs::StructDef> {
    def.child(path)
        .ok_or(Error::Invalid("shad definition lacks a block"))
}

fn build_record(
    layout: &Layout,
    stem: TagId,
    bitmaps: &[Option<Bound>],
    colors_in: &[u32],
    vectors: &[[f32; 4]],
    picture: Option<usize>,
) -> Result<Record> {
    let def = shad_def();
    let mut root = defs::default_record(def);
    set(&mut root, "template", Val::tag(FourCC(*b"stem"), stem))?;

    // The root parameters: the picture as base or self-illumination map.
    let pdef = child(def, "parameters")?;
    let mut params = defs::default_record(pdef);
    let picture_id = picture.and_then(|k| bitmaps[k]).map(|b| b.id);
    let mut colors = [[0.0f32; 3]; 2];
    let mut scalars = [0.0f32; 6];
    if let Some(id) = picture_id {
        match layout.root {
            Root::BaseMap => set(&mut params, "base_map", Val::tag(FourCC(*b"bitm"), id))?,
            Root::SelfIllum => {
                set(&mut params, "self_illum_map", Val::tag(FourCC(*b"bitm"), id))?;
                set(&mut params, "color_10", Val::floats(&[1.0; 3]))?;
                set(&mut params, "scalar_1c", Val::Float(1.0))?;
                colors[0] = [1.0; 3];
                scalars[0] = 1.0;
            }
            Root::None => {}
        }
    }
    set(&mut root, "parameters", Val::Block(vec![params]))?;
    set(
        &mut root,
        "unknown_14",
        Val::Bytes(layout.word_14.to_le_bytes().to_vec()),
    )?;

    // The compiled postprocess.
    let ppdef = child(def, "postprocess")?;
    let mut pp = defs::default_record(ppdef);
    set(&mut pp, "template", datum(Some(stem)))?;
    let bdef = child(ppdef, "bitmaps")?;
    let mut brecs = Vec::new();
    for b in bitmaps {
        let mut r = defs::default_record(bdef);
        set(&mut r, "bitmap", datum(b.map(|b| b.id)))?;
        set(&mut r, "lod_offset", Val::Float(b.map_or(0.0, |b| b.lod_offset)))?;
        brecs.push(r);
    }
    set(&mut pp, "bitmaps", Val::Block(brecs))?;
    let cdef = child(ppdef, "colors")?;
    let crecs = colors_in
        .iter()
        .map(|c| {
            let mut r = defs::default_record(cdef);
            set(&mut r, "color", Val::Int(i64::from(*c)))?;
            Ok(r)
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut pp, "colors", Val::Block(crecs))?;
    let vdef = child(ppdef, "vectors")?;
    let vrecs = vectors
        .iter()
        .map(|v| {
            let mut r = defs::default_record(vdef);
            set(&mut r, "vector", Val::floats(v))?;
            Ok(r)
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut pp, "vectors", Val::Block(vrecs))?;
    // Mirrors of the root parameters: bitmap k (base, self-illum, +0x2c,
    // specular, and a fifth with no root parameter) indexes the
    // postprocess bitmap holding its datum, else -1.
    let pbdef = child(ppdef, "parameter_bitmaps")?;
    let root_maps: [Option<TagId>; 5] = match layout.root {
        Root::BaseMap => [picture_id, None, None, None, None],
        Root::SelfIllum => [None, picture_id, None, None, None],
        Root::None => [None; 5],
    };
    let mut pbs = Vec::new();
    for m in root_maps {
        let mut r = defs::default_record(pbdef);
        let index = m
            .and_then(|id| bitmaps.iter().position(|b| b.map(|b| b.id) == Some(id)))
            .map_or(-1, |k| k as i64);
        set(&mut r, "bitmap_index", Val::Int(index))?;
        set(&mut r, "unknown_2", Val::Bytes(vec![0xff, 0xff]))?;
        pbs.push(r);
    }
    set(&mut pp, "parameter_bitmaps", Val::Block(pbs))?;
    let pcdef = child(ppdef, "parameter_colors")?;
    let pcs = colors
        .iter()
        .map(|c| {
            let mut r = defs::default_record(pcdef);
            set(&mut r, "color", Val::floats(c))?;
            Ok(r)
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut pp, "parameter_colors", Val::Block(pcs))?;
    let psdef = child(ppdef, "parameter_scalars")?;
    let pss = scalars
        .iter()
        .map(|s| {
            let mut r = defs::default_record(psdef);
            set(&mut r, "value", Val::Float(*s))?;
            Ok(r)
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut pp, "parameter_scalars", Val::Block(pss))?;
    set(&mut root, "postprocess", Val::Block(vec![pp]))?;

    // One image entry per non-null bitmap, numbering repeats.
    let idef = child(def, "bitmap_images")?;
    let mut imgs = Vec::new();
    let mut seen: HashMap<TagId, i64> = HashMap::new();
    for b in bitmaps.iter().flatten() {
        let n = seen.entry(b.id).or_insert(0);
        let mut r = defs::default_record(idef);
        set(&mut r, "image", Val::Int(*n))?;
        set(&mut r, "bitmap", datum(Some(b.id)))?;
        imgs.push(r);
        *n += 1;
    }
    set(&mut root, "bitmap_images", Val::Block(imgs))?;
    set(
        &mut root,
        "slit_ref_34",
        Val::Ref {
            group: FourCC(*b"slit").as_u32(),
            target: Target::Null,
        },
    )?;
    set(&mut root, "int_input_4", Val::Int(i64::from(layout.ints[0])))?;
    set(&mut root, "int_input_5", Val::Int(i64::from(layout.ints[1])))?;
    Ok(root)
}

/// Halo 1 shader groups this converts.
pub const H1_GROUPS: [&str; 9] = [
    "senv", "soso", "schi", "scex", "sotr", "swat", "sgla", "smet", "spla",
];

/// One map of a Halo 1 shader in a role.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct H1Bound {
    pub bitmap: h1map::TagId,
    pub scale: [f32; 2],
}

/// A Halo 1 shader read for the rebuild: the layout its type and flags
/// choose, and its maps by role.
#[derive(Debug, Clone, PartialEq)]
pub struct H1Shader {
    pub name: String,
    pub group: String,
    pub layout: &'static Layout,
    pub maps: Vec<(Role, H1Bound)>,
    /// Values over the layout's (tints).
    pub extras: Extras,
    /// What to draw with when the base lacks [`Self::layout`]'s stem.
    pub fallback: Option<Box<H1Shader>>,
}

impl H1Shader {
    /// The first of this and its fallbacks whose stem `library` holds.
    pub fn choose(&self, library: &Library) -> Option<&H1Shader> {
        if library.has(self.layout) {
            return Some(self);
        }
        self.fallback.as_deref().and_then(|f| f.choose(library))
    }

    /// Puts each map where its texture kind goes, in this and every
    /// fallback: a cube map in the layout's cube slot (a Halo 1 glass or
    /// water whose one map is a reflection cube), and any map whose kind
    /// its slot doesn't sample (a cube or volume in a 2D slot) dropped,
    /// so the slot keeps its default. The stems sample each slot as one
    /// kind; a cube where a 2D texture is sampled stops the draw.
    pub fn fit_kinds(&mut self, h1: &h1map::H1Map) {
        use h1map::bitmap::ImageKind;
        let has_cube = self
            .layout
            .bitmaps
            .iter()
            .any(|b| matches!(b, Bm::Role(Role::Cube, _)));
        let mut out: Vec<(Role, H1Bound)> = Vec::new();
        for (role, m) in std::mem::take(&mut self.maps) {
            let role = match (role, h1_texture_kind(h1, m.bitmap)) {
                (Role::Cube, Some(ImageKind::Cube)) => Role::Cube,
                (Role::Cube, _) => continue,
                (r, Some(ImageKind::D2)) => r,
                (Role::Base, Some(ImageKind::Cube)) if has_cube => Role::Cube,
                _ => continue,
            };
            if !out.iter().any(|(r, _)| *r == role) {
                out.push((role, m));
            }
        }
        self.maps = out;
        if let Some(f) = self.fallback.as_mut() {
            f.fit_kinds(h1);
        }
    }

    /// This and its fallbacks, first choice first.
    pub fn chain(&self) -> Vec<&H1Shader> {
        let mut out = vec![self];
        while let Some(f) = out.last().unwrap().fallback.as_deref() {
            out.push(f);
        }
        out
    }
}

/// `senv` flags (`+0x28`), bit 0 alpha-tested (`editor_core::halo1_art`).
pub const SENV_FLAGS: usize = 0x28;
/// `soso` flags (`+0x28`, as [`SENV_FLAGS`]): bit 1 two-sided, bit 2
/// not alpha-tested, bit 3 alpha-blended decal (S, the field's order in
/// Halo 1's model shader). V (Blood Gulch's 106 `soso`): 0, 1, 2, 3 or
/// 8; bit 2 on none, so every model shader is alpha-tested on its base
/// map's alpha. The plants' and trees' leaves (two-sided, DXT3 bases
/// three quarters clear) and the weapon and ammunition decals (bit 3,
/// DXT3) cut out where their base is clear.
pub const SOSO_NOT_ALPHA_TESTED: u16 = 1 << 2;

/// Whether a `soso` with `flags` ([`SOSO_NOT_ALPHA_TESTED`]) draws with
/// an alpha test: Halo 1 tests a model shader's base alpha unless the
/// flag says not to. Only a base that stores an alpha (`base_has_alpha`,
/// [`h1_has_alpha`]) has anything to cut; an opaque one keeps the plain
/// stem. Before converter version 84 every `soso` drew opaque: Blood
/// Gulch's leaves are solid dark green cards in a live frame, the clear
/// part of their texture (62 % of the broadleaf's) drawn too. A cut-out
/// live is not shown yet: pointing the broadleaf's leaf materials at an
/// alpha-tested shader drawing its leaf texture (ours, or retail's
/// `br_ammo_bag` with the texture swapped in) left the cards whole, so
/// the object path's alpha test (or the edit reaching it) needs a look of
/// its own.
pub fn soso_alpha_tested(flags: u16, base_has_alpha: bool) -> bool {
    flags & SOSO_NOT_ALPHA_TESTED == 0 && base_has_alpha
}

/// Whether the Halo 1 bitmap `id`'s first image stores an alpha channel
/// (DXT3, DXT5, A8R8G8B8, A1R5G5B5 or A4R4G4B4). DXT1 counts as opaque:
/// Halo 1's DXT1 model bases (every weapon and vehicle on Blood Gulch)
/// have none to test.
pub fn h1_has_alpha(h1: &h1map::H1Map, id: h1map::TagId) -> bool {
    use h1map::bitmap::PixelFormat as F;
    h1.tag(id)
        .ok()
        .and_then(|t| h1map::bitmap::Bitmap::read(h1, t).ok())
        .and_then(|b| {
            b.images.first().map(|i| {
                matches!(i.format, F::Dxt3 | F::Dxt5 | F::A8R8G8B8 | F::A1R5G5B5 | F::A4R4G4B4)
            })
        })
        .unwrap_or(false)
}

/// `senv` type (`+0x2a`): 0 normal, 1 blended (the base map's alpha
/// blends the primary and secondary detail maps), 2 blended with the
/// base specular. V (395 `senv` on the disc): 0 or 1. Blood Gulch's
/// ground (primary sand, secondary grass) is 1: its base alpha is 1 on
/// 80% of the texels, the grass, and 0 on 16%, the sand paths, so the
/// secondary detail shows where the alpha is 1 (a real run with the
/// primary there drew the canyon floor as sand).
pub const SENV_TYPE: usize = 0x2a;
/// `senv` primary detail map (`+0xb8`, its scale the f32 before it).
pub const SENV_PRIMARY: usize = 0xb8;
/// `senv` secondary detail map (`+0xcc`, scale before it). V: set on 29.
pub const SENV_SECONDARY: usize = 0xcc;
/// `senv` micro detail map (`+0xfc`, scale before it).
pub const SENV_MICRO: usize = 0xfc;
/// `spla` base map (`+0xd4`, `editor_core::halo1_art`).
pub const SPLA_MAP: usize = 0xd4;

/// Reads the Halo 1 shader `tag` (any of [`H1_GROUPS`]). `None` for
/// another group or one with no map this reads. Each map goes where its
/// texture kind is sampled ([`H1Shader::fit_kinds`]).
///
/// - `senv` not alpha-tested, with a detail map:
///   [`TEX_BUMP_DETAIL_BLEND_DETAIL`] (`senv_blend`), falling back to the
///   single-detail [`TEX_BUMP`] on a base without its stem.
/// - `senv`: [`TEX_BUMP`], or [`TEX_BUMP_ALPHA_TEST`] with flags bit 0;
///   base, detail and bump maps with their scales
///   ([`crate::textures::h1_maps_of`], [`crate::textures::h1_scales_of`]);
///   a bump map is kept only when it is palettized bump (format 17).
/// - `soso`: [`TEX_BUMP_ENV`] with its base and detail maps and its
///   reflection (`soso_reflection`) when it has one, else [`TEX_BUMP`],
///   or [`TEX_BUMP_ALPHA_TEST`] when its base is cut out
///   ([`soso_alpha_tested`], which wins over the reflection); the detail
///   tiled by its own scale
///   ([`crate::textures::scales_from`]).
/// - `swat` with a ripple map: [`WATER_POOL`] (`swat_water`), falling
///   back to [`ONE_ALPHA_ENV`] with the base map, as without one.
/// - `sgla`: [`ONE_ALPHA_ENV`] with the base map.
/// - `smet`: [`ILLUM`] with its map.
/// - `spla`: [`ONE_ADD_ILLUM`] with its map.
/// - `schi`, `scex`, `sotr`: their least tiled map
///   ([`crate::sky_shaders::recipe`]), [`ONE_ADD_ILLUM`] when the
///   framebuffer blend is additive, else [`ONE_ALPHA_ENV`].
pub fn read_h1(h1: &h1map::H1Map, tag: &h1map::Tag) -> Option<H1Shader> {
    let mut s = read_h1_maps(h1, tag)?;
    s.fit_kinds(h1);
    if s.maps.is_empty() { None } else { Some(s) }
}

/// The texture kind of the Halo 1 bitmap `id` (its first image); Halo 1's
/// "white" placeholder is 2D.
pub fn h1_texture_kind(h1: &h1map::H1Map, id: h1map::TagId) -> Option<h1map::bitmap::ImageKind> {
    use h1map::bitmap::ImageKind;
    let t = h1.tag(id).ok()?;
    let b = h1map::bitmap::Bitmap::read(h1, t).ok()?;
    Some(match b.images.first()?.kind {
        ImageKind::White => ImageKind::D2,
        k => k,
    })
}

fn read_h1_maps(h1: &h1map::H1Map, tag: &h1map::Tag) -> Option<H1Shader> {
    let group = tag.group.to_string();
    let one = |layout: &'static Layout, id: Option<h1map::TagId>| {
        Some(H1Shader {
            name: tag.name.clone(),
            group: group.clone(),
            layout,
            maps: vec![(
                Role::Base,
                H1Bound {
                    bitmap: id?,
                    scale: [1.0, 1.0],
                },
            )],
            extras: Extras::default(),
            fallback: None,
        })
    };
    match group.as_str() {
        "senv" | "soso" => {
            let m = crate::textures::h1_maps_of(h1, tag)?;
            let s = crate::textures::h1_scales_of(h1, tag);
            let d = h1.tag_data(tag).ok()?;
            let flags = d
                .get(SENV_FLAGS..SENV_FLAGS + 2)
                .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
            let alpha_tested = match group.as_str() {
                "senv" => flags & 1 != 0,
                _ => soso_alpha_tested(flags, m.base.is_some_and(|b| h1_has_alpha(h1, b))),
            };
            let layout = if alpha_tested {
                &TEX_BUMP_ALPHA_TEST
            } else {
                &TEX_BUMP
            };
            let mut maps = vec![(
                Role::Base,
                H1Bound {
                    bitmap: m.base?,
                    scale: [1.0, 1.0],
                },
            )];
            if let Some(b) = m.detail {
                maps.push((
                    Role::Detail,
                    H1Bound {
                        bitmap: b,
                        scale: s.detail,
                    },
                ));
            }
            if let Some(b) = m.bump
                && h1.tag(b).ok().and_then(|t| h1map::bitmap::Bitmap::read(h1, t).ok()).is_some_and(
                    |bm| {
                        bm.images
                            .first()
                            .is_some_and(|i| i.format.raw() == crate::textures::P8_BUMP)
                    },
                )
            {
                maps.push((
                    Role::Bump,
                    H1Bound {
                        bitmap: b,
                        scale: [s.bump; 2],
                    },
                ));
            }
            let mk = |layout: &'static Layout, maps: Vec<(Role, H1Bound)>, extras: Extras| H1Shader {
                name: tag.name.clone(),
                group: group.clone(),
                layout,
                maps,
                extras,
                fallback: None,
            };
            // Last resort first: each choice falls back to the one before.
            let mut out = mk(layout, maps.clone(), Extras::default());
            if group == "senv" && !alpha_tested {
                if let Some(blend) = senv_blend(h1, d, &maps) {
                    let mut b = mk(&TEX_BUMP_DETAIL_BLEND_DETAIL, blend, Extras::default());
                    b.fallback = Some(Box::new(out));
                    out = b;
                }
                // A blended surface keeps its blend: no reflection stem
                // on every base blends two details.
                let blended = d
                    .get(SENV_TYPE..SENV_TYPE + 2)
                    .is_some_and(|b| u16::from_le_bytes([b[0], b[1]]) != 0);
                if !blended && let Some((cube, extras)) = senv_reflection(h1, d) {
                    let mut env = maps.clone();
                    env.push((Role::Cube, cube));
                    let mut e = mk(&TEX_BUMP_ENV, env, extras);
                    e.fallback = Some(Box::new(out));
                    out = e;
                }
            }
            // A cut-out `soso` keeps its cut-out over its reflection: no
            // layout here alpha-tests with a cube map (retail's
            // `tex_bump_env_alpha_test` binds seven maps), and the
            // reflection mask would take the base's alpha. Retail's ammo
            // bags are alpha-tested without a reflection too.
            if group == "soso"
                && !alpha_tested
                && let Some((cube, extras)) = soso_reflection(h1, d)
            {
                let mut env = maps.clone();
                env.push((Role::Cube, cube));
                let mut e = mk(&TEX_BUMP_ENV, env, extras);
                e.fallback = Some(Box::new(out));
                out = e;
            }
            Some(out)
        }
        "swat" => {
            let plain = one(&ONE_ALPHA_ENV, crate::textures::h1_maps_of(h1, tag)?.base);
            let Some((maps, extras)) = swat_water(h1.tag_data(tag).ok()?) else {
                return plain;
            };
            Some(H1Shader {
                name: tag.name.clone(),
                group,
                layout: &WATER_POOL,
                maps,
                extras,
                fallback: plain.map(Box::new),
            })
        }
        "sgla" => one(&ONE_ALPHA_ENV, crate::textures::h1_maps_of(h1, tag)?.base),
        "smet" => one(&ILLUM, crate::textures::h1_maps_of(h1, tag)?.base),
        "spla" => {
            let d = h1.tag_data(tag).ok()?;
            let r = h1map::TagRef::read(d, SPLA_MAP)?;
            let id = (r.group == h1map::Group::new(b"bitm") && !r.id.is_none()).then_some(r.id);
            one(&ONE_ADD_ILLUM, id)
        }
        "schi" | "scex" | "sotr" => {
            let s = crate::sky_shaders::read_h1(h1, tag.id)?;
            let k = match crate::sky_shaders::recipe(&s)? {
                crate::sky_shaders::Recipe::Picture(k) | crate::sky_shaders::Recipe::Additive(k) => k,
                crate::sky_shaders::Recipe::Clouds { tile, .. } => tile,
            };
            let maps: Vec<(H1Bound, Option<h1map::bitmap::ImageKind>)> = s
                .maps
                .iter()
                .map(|m| {
                    (
                        H1Bound {
                            bitmap: m.bitmap,
                            scale: m.scale,
                        },
                        h1_texture_kind(h1, m.bitmap),
                    )
                })
                .collect();
            Some(H1Shader {
                name: tag.name.clone(),
                group,
                layout: if s.additive {
                    &ONE_ADD_ILLUM
                } else {
                    &ONE_ALPHA_ENV
                },
                maps: transparent_maps(&maps, k, s.additive),
                extras: Extras::default(),
                fallback: None,
            })
        }
        _ => None,
    }
}

/// The maps of a Halo 1 transparent shader (`schi`, `scex`, `sotr`;
/// `maps`, each with its texture kind) for [`ONE_ALPHA_ENV`] or, when
/// `additive`, [`ONE_ADD_ILLUM`]; `picture` is the map
/// [`crate::sky_shaders::recipe`] chose, the least tiled.
///
/// The picture is a 2D map: when the chosen one is a cube (a glass's
/// reflection, often its first map and at the same 1×1 scale as the
/// glass's own texture), the least tiled 2D map stands in. An alpha
/// blended shader's cube map is its reflection, in [`ONE_ALPHA_ENV`]'s
/// cube slot. D: a10's `levels\a10\shaders\cryo glass` (blend 7) has the
/// cube `cube map glass` first and the 2D `cryo glass` (DXT3, alpha about
/// 10%) second, both 1×1. With the cube as the picture the base slot kept
/// its default and the glass drew as a flat opaque pane: the grey slab
/// over the view from inside a10's cryo tube.
pub fn transparent_maps(
    maps: &[(H1Bound, Option<h1map::bitmap::ImageKind>)],
    picture: usize,
    additive: bool,
) -> Vec<(Role, H1Bound)> {
    use h1map::bitmap::ImageKind;
    let flat = |k: usize| maps[k].1 == Some(ImageKind::D2);
    let area = |k: usize| (maps[k].0.scale[0] * maps[k].0.scale[1]).abs();
    let base = if picture < maps.len() && flat(picture) {
        Some(picture)
    } else {
        (0..maps.len())
            .filter(|&k| flat(k))
            .min_by(|&a, &b| area(a).total_cmp(&area(b)))
            .or((picture < maps.len()).then_some(picture))
    };
    let mut out: Vec<(Role, H1Bound)> = base.map(|k| (Role::Base, maps[k].0)).into_iter().collect();
    if !additive
        && let Some(c) =
            (0..maps.len()).find(|&k| Some(k) != base && maps[k].1 == Some(ImageKind::Cube))
    {
        out.push((Role::Cube, maps[c].0));
    }
    out
}

/// `swat`'s reflection map (D: a cube on each of the eight distinct
/// `swat` of the disc).
pub const SWAT_REFLECTION: usize = 0x9c;
/// `swat`'s ripple animation: f32 angle (radians), f32 velocity, f32
/// scale, then the ripple map. D (the eight `swat`): angles 0 to 3π/2,
/// velocities 0 to 0.05, scales 4 to 220; the map a single-image 2D
/// bump (P8 bump, or X8R8G8B8 on `c10`'s two); the four ripples of the
/// block at `+0x124` (stride 0x4c) each scroll it again at their own
/// angle and velocity, which `water_pool`'s one ripple sample can't.
pub const SWAT_RIPPLE_ANGLE: usize = 0xbc;
/// See [`SWAT_RIPPLE_ANGLE`].
pub const SWAT_RIPPLE_VELOCITY: usize = 0xc0;
/// See [`SWAT_RIPPLE_ANGLE`].
pub const SWAT_RIPPLE_SCALE: usize = 0xc4;
/// See [`SWAT_RIPPLE_ANGLE`].
pub const SWAT_RIPPLE_MAP: usize = 0xc8;

/// A `swat`'s ([`SWAT_RIPPLE_ANGLE`]; `d`, its data) maps for
/// [`WATER_POOL`]: the ripple map as the bump, tiled by the ripple scale
/// over the surface's texture coordinates, and the reflection as the
/// cube; the ripple scrolls at the ripple velocity along the ripple
/// angle (u by its cosine, v by its sine), one ripple tile per
/// `1 / speed` seconds. `None` without a ripple map.
fn swat_water(d: &[u8]) -> Option<(Vec<(Role, H1Bound)>, Extras)> {
    let bitmap = |o: usize| {
        let r = h1map::TagRef::read(d, o)?;
        (r.group == h1map::Group::new(b"bitm") && !r.id.is_none()).then_some(r.id)
    };
    let f32_at = |o: usize| Some(f32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?));
    let scale = f32_at(SWAT_RIPPLE_SCALE)
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0);
    let mut maps = vec![(
        Role::Bump,
        H1Bound {
            bitmap: bitmap(SWAT_RIPPLE_MAP)?,
            scale: [scale; 2],
        },
    )];
    if let Some(c) = bitmap(SWAT_REFLECTION) {
        maps.push((
            Role::Cube,
            H1Bound {
                bitmap: c,
                scale: [1.0, 1.0],
            },
        ));
    }
    let angle = f32_at(SWAT_RIPPLE_ANGLE).unwrap_or(0.0);
    let velocity = f32_at(SWAT_RIPPLE_VELOCITY).unwrap_or(0.0);
    let slide = |speed: f32| {
        (speed.is_finite() && speed.abs() > 1e-6).then(|| Slide {
            period: 1.0 / speed.abs(),
            from: 0.0,
            to: speed.signum(),
        })
    };
    let scroll = [slide(velocity * angle.cos()), slide(velocity * angle.sin())];
    let extras = Extras {
        colors: Vec::new(),
        scroll: scroll.iter().any(Option::is_some).then_some(scroll),
    };
    Some((maps, extras))
}

/// The maps of an `senv` (`d`, its data) for
/// [`TEX_BUMP_DETAIL_BLEND_DETAIL`], from `single`, its maps for
/// [`TEX_BUMP`] (base and bump): Halo 1's primary detail where the base
/// alpha is 0 ([`Role::Detail`]) and, for a blended type, its secondary
/// where it is 1 ([`Role::Secondary`]); a normal one (or one missing
/// either) shows the one it has everywhere. The micro detail is
/// [`Role::Micro`]. `None` when it names no detail map.
fn senv_blend(h1: &h1map::H1Map, d: &[u8], single: &[(Role, H1Bound)]) -> Option<Vec<(Role, H1Bound)>> {
    let at = |o: usize| {
        let r = h1map::TagRef::read(d, o)?;
        if r.group != h1map::Group::new(b"bitm") || r.id.is_none() || h1.tag(r.id).is_err() {
            return None;
        }
        let s = d
            .get(o - 4..o)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .filter(|s| s.is_finite() && *s > 0.0)
            .unwrap_or(1.0);
        Some(H1Bound {
            bitmap: r.id,
            scale: [s, s],
        })
    };
    let blended = d
        .get(SENV_TYPE..SENV_TYPE + 2)
        .is_some_and(|b| u16::from_le_bytes([b[0], b[1]]) != 0);
    let primary = at(SENV_PRIMARY);
    let secondary = if blended { at(SENV_SECONDARY) } else { None };
    let micro = at(SENV_MICRO);
    if primary.is_none() && secondary.is_none() && micro.is_none() {
        return None;
    }
    let mut maps: Vec<(Role, H1Bound)> = single
        .iter()
        .filter(|(r, _)| matches!(r, Role::Base | Role::Bump))
        .copied()
        .collect();
    if let Some(p) = primary.or(secondary) {
        maps.push((Role::Detail, p));
    }
    if let Some(s) = secondary.or(primary) {
        maps.push((Role::Secondary, s));
    }
    if let Some(m) = micro {
        maps.push((Role::Micro, m));
    }
    Some(maps)
}

/// `senv` specular brightness (`+0x290`) and its colours facing the eye
/// and at a grazing angle (`+0x2a8`, `+0x2b4`, RGB f32). V (395 `senv`):
/// brightness 0.125 to 1, colours in 0..1.
pub const SENV_SPECULAR: usize = 0x290;
pub const SENV_PERPENDICULAR_COLOR: usize = 0x2a8;
pub const SENV_PARALLEL_COLOR: usize = 0x2b4;
/// `senv` reflection brightness facing the eye and grazing (`+0x2f4`,
/// `+0x2f8`; V 0 to 1) and the reflection cube map (`+0x324`, V: a cube
/// bitmap on 292 of 395).
pub const SENV_PERPENDICULAR_BRIGHTNESS: usize = 0x2f4;
pub const SENV_PARALLEL_BRIGHTNESS: usize = 0x2f8;
pub const SENV_CUBE: usize = 0x324;

/// An `senv`'s reflection for [`TEX_BUMP_ENV`]: its cube map and the
/// colours 1 to 4 (the tint facing the eye and grazing, each Halo 1's
/// colour times its specular brightness; the brightnesses as alphas).
/// `None` without a cube map or with both brightnesses 0.
fn senv_reflection(h1: &h1map::H1Map, d: &[u8]) -> Option<(H1Bound, Extras)> {
    reflection(
        h1,
        d,
        &ReflectionAt {
            cube: SENV_CUBE,
            perpendicular_brightness: SENV_PERPENDICULAR_BRIGHTNESS,
            parallel_brightness: SENV_PARALLEL_BRIGHTNESS,
            perpendicular_color: SENV_PERPENDICULAR_COLOR,
            parallel_color: SENV_PARALLEL_COLOR,
            tint_brightness: Some(SENV_SPECULAR),
        },
    )
}

/// `soso` (Halo 1's model shader) reflection: brightness facing the eye
/// (`+0x144`, f32) and its tint (`+0x148`, RGB f32), brightness grazing
/// (`+0x154`) and its tint (`+0x158`), the cube map (`+0x164`). V (Blood
/// Gulch's 106 `soso`): a `bitm` reference at `+0x164` on exactly those
/// with a brightness set, brightnesses and tints in 0..1 on those read
/// (the assault rifle's and sniper rifle's metal 1 and white, the
/// cyborg's visor 1 and a blue grazing tint); the two f32 before them
/// (`+0x13c`, `+0x140`, 6 and 8 on the ammunition bag) are not read.
pub const SOSO_PERPENDICULAR_BRIGHTNESS: usize = 0x144;
pub const SOSO_PERPENDICULAR_COLOR: usize = 0x148;
pub const SOSO_PARALLEL_BRIGHTNESS: usize = 0x154;
pub const SOSO_PARALLEL_COLOR: usize = 0x158;
pub const SOSO_CUBE: usize = 0x164;

/// `soso` multipurpose map (`+0xbc`, a `bitm` reference). On the Xbox disc
/// its red channel masks the reflection (V: every weapon `soso` on Blood
/// Gulch names one; the sniper rifle's is red only, dark but for its
/// metal). [`TEX_BUMP_ENV`] masks the reflection with the colour pass's
/// alpha (the base map's times the detail's), so a converted `soso` on it
/// takes a base whose alpha is this map's red
/// (`h1bitmaps::mask::red_into_alpha`).
pub const SOSO_MULTIPURPOSE: usize = 0xbc;

/// The `soso` `tag`'s multipurpose map ([`SOSO_MULTIPURPOSE`]), if it names
/// one that is in the map; `None` for another group.
pub fn soso_multipurpose(h1: &h1map::H1Map, tag: &h1map::Tag) -> Option<h1map::TagId> {
    if tag.group != h1map::Group::new(b"soso") {
        return None;
    }
    let r = h1map::TagRef::read(h1.tag_data(tag).ok()?, SOSO_MULTIPURPOSE)?;
    (r.group == h1map::Group::new(b"bitm") && !r.id.is_none() && h1.tag(r.id).is_ok())
        .then_some(r.id)
}

/// A `soso`'s reflection for [`TEX_BUMP_ENV`], as [`senv_reflection`]'s:
/// the tints are Halo 1's colours as they are (a `soso` has no separate
/// specular brightness).
///
/// Without it a converted weapon drew its base map lit and nothing more:
/// Halo 1's weapon base maps are dark (the sniper rifle's averages 35 of
/// 255) and take their metal from this reflection, so the sniper rifle
/// was near black in first person (converter version 76).
fn soso_reflection(h1: &h1map::H1Map, d: &[u8]) -> Option<(H1Bound, Extras)> {
    reflection(
        h1,
        d,
        &ReflectionAt {
            cube: SOSO_CUBE,
            perpendicular_brightness: SOSO_PERPENDICULAR_BRIGHTNESS,
            parallel_brightness: SOSO_PARALLEL_BRIGHTNESS,
            perpendicular_color: SOSO_PERPENDICULAR_COLOR,
            parallel_color: SOSO_PARALLEL_COLOR,
            tint_brightness: None,
        },
    )
}

/// Where a Halo 1 shader keeps its reflection.
struct ReflectionAt {
    cube: usize,
    perpendicular_brightness: usize,
    parallel_brightness: usize,
    perpendicular_color: usize,
    parallel_color: usize,
    /// A brightness the tints are scaled by (`senv`'s specular), if any.
    tint_brightness: Option<usize>,
}

/// The cube map at `at.cube` and colours 1 to 4 of [`TEX_BUMP_ENV`]: the
/// tints facing the eye and grazing (RGB), the brightnesses (alphas).
/// `None` without a cube map or with both brightnesses 0.
fn reflection(h1: &h1map::H1Map, d: &[u8], at: &ReflectionAt) -> Option<(H1Bound, Extras)> {
    let r = h1map::TagRef::read(d, at.cube)?;
    if r.group != h1map::Group::new(b"bitm") || r.id.is_none() || h1.tag(r.id).is_err() {
        return None;
    }
    let colors = reflection_colors(d, at)?;
    Some((
        H1Bound {
            bitmap: r.id,
            scale: [1.0, 1.0],
        },
        Extras {
            colors,
            scroll: None,
        },
    ))
}

/// [`reflection`]'s colours from the shader's bytes `d` alone: `None` when
/// both brightnesses are 0.
fn reflection_colors(d: &[u8], at: &ReflectionAt) -> Option<Vec<(usize, u32)>> {
    let f = |o: usize| {
        d.get(o..o + 4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    };
    let (perp, par) = (f(at.perpendicular_brightness), f(at.parallel_brightness));
    if perp == 0.0 && par == 0.0 {
        return None;
    }
    let bright = at.tint_brightness.map_or(1.0, f);
    let byte = |v: f32| (v * 255.0).round() as u32;
    let rgb = |o: usize| (0..3).fold(0u32, |c, i| (c << 8) | byte(f(o + 4 * i) * bright));
    Some(vec![
        (1, rgb(at.perpendicular_color)),
        (2, rgb(at.parallel_color)),
        (3, byte(perp) << 24),
        (4, byte(par) << 24),
    ])
}

/// The [`lod_offset`] of the Halo 1 bitmap `id` (its first image), 0
/// when it doesn't read.
pub fn h1_lod_offset(h1: &h1map::H1Map, id: h1map::TagId) -> f32 {
    h1.tag(id)
        .ok()
        .and_then(|t| h1map::bitmap::Bitmap::read(h1, t).ok())
        .and_then(|b| b.images.first().map(|i| lod_offset(i.width, i.height)))
        .unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `soso` is alpha-tested on a base with an alpha unless its flags
    /// say not (bit 2); the other bits (two-sided, decal) don't change
    /// that, and an opaque base has nothing to test.
    #[test]
    fn a_model_shader_cuts_out_where_its_base_is_clear() {
        for flags in [0, 1, 2, 3, 8] {
            assert!(soso_alpha_tested(flags, true), "{flags}");
            assert!(!soso_alpha_tested(flags, false), "{flags}");
        }
        assert!(!soso_alpha_tested(SOSO_NOT_ALPHA_TESTED, true));
        assert!(!soso_alpha_tested(SOSO_NOT_ALPHA_TESTED | 2, true));
    }

    /// A `soso`'s reflection (invented values): the tints as they are,
    /// the brightnesses as the alphas of colours 3 and 4; none with both
    /// brightnesses 0.
    #[test]
    fn a_model_shader_reflects_with_its_own_tints() {
        let mut d = vec![0u8; 0x180];
        let put = |d: &mut Vec<u8>, at: usize, v: f32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let at = ReflectionAt {
            cube: SOSO_CUBE,
            perpendicular_brightness: SOSO_PERPENDICULAR_BRIGHTNESS,
            parallel_brightness: SOSO_PARALLEL_BRIGHTNESS,
            perpendicular_color: SOSO_PERPENDICULAR_COLOR,
            parallel_color: SOSO_PARALLEL_COLOR,
            tint_brightness: None,
        };
        assert_eq!(reflection_colors(&d, &at), None);
        put(&mut d, SOSO_PERPENDICULAR_BRIGHTNESS, 1.0);
        put(&mut d, SOSO_PARALLEL_BRIGHTNESS, 0.5);
        for i in 0..3 {
            put(&mut d, SOSO_PERPENDICULAR_COLOR + 4 * i, 1.0);
        }
        put(&mut d, SOSO_PARALLEL_COLOR, 0.0);
        put(&mut d, SOSO_PARALLEL_COLOR + 4, 0.5);
        put(&mut d, SOSO_PARALLEL_COLOR + 8, 2.0);
        assert_eq!(
            reflection_colors(&d, &at),
            Some(vec![
                (1, 0x00ff_ffff),
                (2, 0x0000_80ff),
                (3, 0xff00_0000),
                (4, 0x8000_0000),
            ])
        );
    }

    fn h1_bound(id: u32, scale: f32) -> H1Bound {
        H1Bound {
            bitmap: h1map::TagId(id),
            scale: [scale, scale],
        }
    }

    /// a10's cryo glass: a 1×1 reflection cube first, the 1×1 glass
    /// texture second. The glass is the picture, the cube its reflection.
    #[test]
    fn a_glass_whose_first_map_is_a_cube_draws_its_2d_map_and_reflects_the_cube() {
        use h1map::bitmap::ImageKind;
        let maps = [
            (h1_bound(1, 1.0), Some(ImageKind::Cube)),
            (h1_bound(2, 1.0), Some(ImageKind::D2)),
        ];
        assert_eq!(
            transparent_maps(&maps, 0, false),
            vec![
                (Role::Base, h1_bound(2, 1.0)),
                (Role::Cube, h1_bound(1, 1.0))
            ]
        );
        // Additive: no reflection slot; the 2D map still draws.
        assert_eq!(
            transparent_maps(&maps, 0, true),
            vec![(Role::Base, h1_bound(2, 1.0))]
        );
    }

    /// A 2D picture stays the picture; with several 2D maps and a cube
    /// chosen, the least tiled 2D map is taken.
    #[test]
    fn a_2d_picture_is_kept_and_a_cube_choice_falls_to_the_least_tiled_2d_map() {
        use h1map::bitmap::ImageKind;
        let maps = [
            (h1_bound(1, 8.0), Some(ImageKind::D2)),
            (h1_bound(2, 1.0), Some(ImageKind::D2)),
        ];
        assert_eq!(
            transparent_maps(&maps, 1, false),
            vec![(Role::Base, h1_bound(2, 1.0))]
        );
        let maps = [
            (h1_bound(1, 1.0), Some(ImageKind::Cube)),
            (h1_bound(2, 8.0), Some(ImageKind::D2)),
            (h1_bound(3, 4.0), Some(ImageKind::D2)),
        ];
        assert_eq!(
            transparent_maps(&maps, 0, false),
            vec![
                (Role::Base, h1_bound(3, 4.0)),
                (Role::Cube, h1_bound(1, 1.0))
            ]
        );
    }

    /// With no 2D map, the chosen map stays (fit_kinds then moves a cube
    /// to the cube slot), and an unreadable kind is not a reflection.
    #[test]
    fn with_no_2d_map_the_chosen_map_stays() {
        use h1map::bitmap::ImageKind;
        let maps = [
            (h1_bound(1, 1.0), Some(ImageKind::Cube)),
            (h1_bound(2, 1.0), None),
        ];
        assert_eq!(
            transparent_maps(&maps, 0, false),
            vec![(Role::Base, h1_bound(1, 1.0))]
        );
    }

    fn library() -> Library {
        let stems: Vec<(&'static str, TagId)> = LAYOUTS
            .iter()
            .enumerate()
            .map(|(k, l)| (l.stem, TagId(0xe100_0000 + k as u32)))
            .collect();
        let fixed: Vec<(Fixed, Bound)> = Fixed::ALL
            .iter()
            .enumerate()
            .map(|(k, &f)| {
                (
                    f,
                    Bound {
                        id: TagId(0xe200_0000 + k as u32),
                        scale: [1.0, 1.0],
                        lod_offset: 1.0,
                    },
                )
            })
            .collect();
        Library::new(&stems, &fixed)
    }

    #[test]
    fn a_swat_draws_on_water_pool_its_ripple_scrolling_along_the_ripple_angle() {
        let mut d = vec![0u8; 0x140];
        let put_ref = |d: &mut [u8], at: usize, id: u32| {
            d[at..at + 4].copy_from_slice(&h1map::Group::new(b"bitm").0.to_le_bytes());
            d[at + 12..at + 16].copy_from_slice(&id.to_le_bytes());
        };
        let put = |d: &mut [u8], at: usize, v: f32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put_ref(&mut d, SWAT_REFLECTION, 0xe400_0001);
        put_ref(&mut d, SWAT_RIPPLE_MAP, 0xe400_0002);
        // 3π/2 (as on four of the disc's): straight down v, backwards.
        put(&mut d, SWAT_RIPPLE_ANGLE, 3.0 * std::f32::consts::FRAC_PI_2);
        put(&mut d, SWAT_RIPPLE_VELOCITY, 0.05);
        put(&mut d, SWAT_RIPPLE_SCALE, 4.0);
        let (maps, extras) = swat_water(&d).unwrap();
        let bound = |id: u32, s: f32| H1Bound {
            bitmap: h1map::TagId(id),
            scale: [s, s],
        };
        assert_eq!(
            maps,
            vec![(Role::Bump, bound(0xe400_0002, 4.0)), (Role::Cube, bound(0xe400_0001, 1.0))]
        );
        let [u, v] = extras.scroll.unwrap();
        assert_eq!(u, None, "cos(3π/2) is no u motion");
        let v = v.unwrap();
        assert!((v.period - 20.0).abs() < 1e-3, "one tile per 1 / 0.05 s");
        assert_eq!((v.from, v.to), (0.0, -1.0));

        // On the stem: the ripple's tiling in vector 0, the tree animating
        // c[20] and the ripple's frame from the scroll's ten components.
        let lib = library();
        let b = |id: u32, s: f32| Bound {
            id: TagId(id),
            scale: [s, s],
            lod_offset: 1.0,
        };
        let on: HashMap<Role, Bound> =
            [(Role::Bump, b(0xe300_0001, 4.0)), (Role::Cube, b(0xe300_0002, 1.0))].into_iter().collect();
        let rec = lib.record_with(&WATER_POOL, &on, &extras).unwrap();
        assert_eq!(
            rec.at("postprocess[0].vectors[0].vector"),
            Some(&Val::floats(&[4.0, 4.0, 0.0, 0.0]))
        );
        assert_eq!(
            rec.at("postprocess[0].bitmaps[0].bitmap"),
            Some(&Val::Datum(Target::Id(TagId(0xe300_0001))))
        );
        assert_eq!(
            rec.at("postprocess[0].bitmaps[1].bitmap"),
            Some(&Val::Datum(Target::Id(TagId(0xe300_0002))))
        );
        assert_eq!(
            rec.at("postprocess[0].lods[0].layer_mask"),
            Some(&Val::Int(1 << 15))
        );
        let n = |p: &str| match rec.at(p) {
            Some(Val::Block(b)) => b.len(),
            _ => usize::MAX,
        };
        assert_eq!(n("postprocess[0].animations"), 2);
        assert_eq!(n("postprocess[0].property_refs"), 10);

        // Still water has no tree; no ripple map, no water.
        put(&mut d, SWAT_RIPPLE_VELOCITY, 0.0);
        assert_eq!(swat_water(&d).unwrap().1.scroll, None);
        d[SWAT_RIPPLE_MAP + 12..SWAT_RIPPLE_MAP + 16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(swat_water(&d).is_none());
    }

    #[test]
    fn retail_lod_offsets_follow_the_size() {
        // The values retail holds for 4², 8², 16² and 512² bitmaps.
        assert!((lod_offset(4, 4) - 0.960906).abs() < 1e-5);
        assert!((lod_offset(8, 8) - 1.441359).abs() < 1e-5);
        assert!((lod_offset(16, 16) - 1.921812).abs() < 1e-5);
        assert!((lod_offset(512, 512) - 4.324077).abs() < 1e-4);
        assert_eq!(lod_offset(0, 0), 0.0);
    }

    #[test]
    fn every_layout_builds_and_reads_back() {
        let lib = library();
        let maps: HashMap<Role, Bound> = [Role::Base, Role::Detail, Role::Bump, Role::Mask, Role::Tile]
            .iter()
            .enumerate()
            .map(|(k, &r)| {
                (
                    r,
                    Bound {
                        id: TagId(0xe300_0000 + k as u32),
                        scale: [k as f32 + 2.0, k as f32 + 3.0],
                        lod_offset: 2.0,
                    },
                )
            })
            .collect();
        for l in LAYOUTS {
            assert!(lib.has(l), "{}", l.stem);
            let tree = lib.build(l, &maps).unwrap();
            assert_eq!(tree.bytes.len(), 0x54);
            // A builder tree's reference reads back with group 0 (the
            // builder writes the target's); a second trip is exact.
            let back = defs::from_built(shad_def(), &tree).unwrap();
            let again = defs::to_built(shad_def(), &back, &mut |_| None).unwrap();
            let back2 = defs::from_built(shad_def(), &again).unwrap();
            assert_eq!(defs::diff(shad_def(), &back, &back2), Vec::new(), "{}", l.stem);
            assert_eq!(
                back.at("postprocess[0].template"),
                Some(&Val::Datum(Target::Id(lib.stem(l).unwrap())))
            );
            let n = |p: &str| match back.at(p) {
                Some(Val::Block(b)) => b.len(),
                _ => usize::MAX,
            };
            assert_eq!(n("postprocess[0].bitmaps"), l.bitmaps.len());
            assert_eq!(n("postprocess[0].colors"), l.colors.len());
            assert_eq!(n("postprocess[0].vectors"), l.vectors.len());
            assert_eq!(n("postprocess[0].parameter_bitmaps"), 5);
            assert_eq!(n("postprocess[0].parameter_colors"), 2);
            assert_eq!(n("postprocess[0].parameter_scalars"), 6);
            assert_eq!(n("postprocess[0].lods"), 0, "not animated");
        }
    }

    #[test]
    fn the_opaque_layout_tiles_detail_and_bump_and_mirrors_the_base() {
        let lib = library();
        let b = |id: u32, s: f32| Bound {
            id: TagId(id),
            scale: [s, s],
            lod_offset: 3.0,
        };
        let maps: HashMap<Role, Bound> = [
            (Role::Base, b(0xe3000001, 1.0)),
            (Role::Detail, b(0xe3000002, 16.0)),
            (Role::Bump, b(0xe3000003, 4.0)),
        ]
        .into_iter()
        .collect();
        let rec = lib.record(&TEX_BUMP, &maps).unwrap();
        let vec = |k: usize| match rec.at(&format!("postprocess[0].vectors[{k}].vector")) {
            Some(Val::List(v)) => v
                .iter()
                .map(|x| match x {
                    Val::Float(f) => *f,
                    _ => f32::NAN,
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        assert_eq!(vec(0), [4.0, 4.0, 0.0, 0.0], "bump");
        assert_eq!(vec(1), [1.0, 1.0, 0.0, 0.0], "base");
        assert_eq!(vec(2), [16.0, 16.0, 0.0, 0.0], "detail");
        assert_eq!(vec(7), [0.0, 0.0, 0.0, 0.0], "specular colour 0 (black) as floats");
        let bm = |k: usize| rec.at(&format!("postprocess[0].bitmaps[{k}].bitmap")).cloned();
        assert_eq!(bm(2), Some(Val::Datum(Target::Id(TagId(0xe3000001)))));
        assert_eq!(bm(1), Some(Val::Datum(Target::Null)));
        assert_eq!(
            rec.at("parameters[0].base_map"),
            Some(&Val::tag(FourCC(*b"bitm"), TagId(0xe3000001)))
        );
        assert_eq!(
            rec.at("postprocess[0].parameter_bitmaps[0].bitmap_index"),
            Some(&Val::Int(2))
        );
        assert_eq!(
            rec.at("postprocess[0].parameter_bitmaps[1].bitmap_index"),
            Some(&Val::Int(-1))
        );
        // Four non-null bitmaps, each listed once.
        match rec.at("bitmap_images") {
            Some(Val::Block(b)) => assert_eq!(b.len(), 4),
            other => panic!("{other:?}"),
        }
        // Without a detail map the default detail draws, untiled.
        let mut no_detail = maps.clone();
        no_detail.remove(&Role::Detail);
        let rec = lib.record(&TEX_BUMP, &no_detail).unwrap();
        assert_eq!(
            rec.at("postprocess[0].bitmaps[3].bitmap"),
            Some(&Val::Datum(Target::Id(lib.fixed[&Fixed::DefaultDetail].id)))
        );
    }

    #[test]
    fn the_detail_blend_layout_puts_each_detail_in_its_stage() {
        let lib = library();
        let b = |id: u32, s: f32| Bound {
            id: TagId(id),
            scale: [s, s],
            lod_offset: 3.0,
        };
        let maps: HashMap<Role, Bound> = [
            (Role::Base, b(0xe3000001, 1.0)),
            (Role::Detail, b(0xe3000002, 100.0)),
            (Role::Secondary, b(0xe3000003, 60.0)),
            (Role::Micro, b(0xe3000004, 12.0)),
        ]
        .into_iter()
        .collect();
        let rec = lib.record(&TEX_BUMP_DETAIL_BLEND_DETAIL, &maps).unwrap();
        let vec = |k: usize| match rec.at(&format!("postprocess[0].vectors[{k}].vector")) {
            Some(Val::List(v)) => v
                .iter()
                .map(|x| match x {
                    Val::Float(f) => *f,
                    _ => f32::NAN,
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        let bm = |k: usize| rec.at(&format!("postprocess[0].bitmaps[{k}].bitmap")).cloned();
        let id = |n: u32| Some(Val::Datum(Target::Id(TagId(n))));
        assert_eq!(bm(1), id(0xe3000001), "base, stage 0");
        assert_eq!(bm(2), id(0xe3000002), "primary, stage 1 (base alpha 0)");
        assert_eq!(bm(3), id(0xe3000003), "secondary, stage 2 (base alpha 1)");
        assert_eq!(bm(4), id(0xe3000004), "micro, stage 3");
        assert_eq!(vec(0), [100.0, 100.0, 0.0, 0.0]);
        assert_eq!(vec(1), [60.0, 60.0, 0.0, 0.0]);
        assert_eq!(vec(2), [0.0, 0.0, 12.0, 12.0], "c[20].zw");
        assert_eq!(vec(3), [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(vec(7), [0.0, 0.0, 0.0, 0.0], "no specular");
    }

    #[test]
    fn a_missing_stem_or_default_refuses() {
        let lib = Library::new(&[], &[]);
        assert!(!lib.has(&TEX_BUMP));
        assert!(lib.record(&TEX_BUMP, &HashMap::new()).is_err());
    }
}
