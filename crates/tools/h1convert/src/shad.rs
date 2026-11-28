//! Halo 2 `shad` tags written from their definition (`tags::defs`,
//! `crates/formats/tags/src/defs/shad.txt`) and Halo 1's shader, with no byte of
//! a Halo 2 shader instance copied.
//!
//! **The program.** A `shad` names a `stem`, the engine's shader program,
//! and fills the instance parameters the stem binds: bitmaps, colours and
//! vectors (postprocess `+0x04`, `+0x0c`, `+0x14`). Each Halo 1 shader
//! type draws with a retail stem chosen for it ([`Layout`]); every stem
//! used here but the three sky ones is in `shared.map`, so every base map
//! has it. The structure installer emits a colour pass for non-default
//! terrain detail modes, and the HUD scope has its own screen-effect pass.
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
//! - no animation unless the layout has an [`AnimTree`] and the shader
//!   something to drive it: the LOD/layer/pass/implementation tree and
//!   the animated properties are otherwise empty, which the engine reads
//!   as "not animated" (`0x000174d0` bounds-checks each level). Animated:
//!   a scrolling map ([`TextureScroll`]) and a weapon's ammunition
//!   counter digit ([`counter_properties`]).

use std::collections::HashMap;
use std::io::{Read, Seek};

use map::build::{Block, TagSet};
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, Str, Target, Val};
use tags::shader::SHAD;
use tags::{DecodedTag, TagData};

use crate::{Error, Result};

mod detail;
pub mod program;

pub use detail::DetailBlend;
/// The names `boot` answers, kept in `h1-contract`.
pub use h1_contract::shader::{
    COUNTDOWN_DIGITS, MODEL_REFLECTION_SIGNATURE, model_reflection_start,
};

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
    /// A change-colour map: each channel weighs one of the object's change
    /// colours (red 0 .. alpha 3; [`TEX_BUMP_ENV_FOUR_CHANGE_COLOR`]). Made
    /// from a `soso`'s multipurpose map ([`soso_change_colour`]).
    ChangeColour,
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
    /// `[0, 0, 0, a]`: colour `k`'s alpha as a float (the reflection
    /// brightnesses [`TEX_BUMP_ENV_FOUR_CHANGE_COLOR`] binds as vertex
    /// defaults).
    AlphaOf(usize),
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
    /// Where the [`Tint`] goes, on a stem drawn by
    /// `transparent_generic_reflection`: its vertex shader blends `v[10]`
    /// (perpendicular colour plus alpha, the surface facing the eye) into
    /// `v[11]` (parallel, grazing) by the view angle, and the pass
    /// multiplies its colour by the result's colour and alpha.
    pub tint: Option<TintAt>,
}

/// The vectors a [`Tint`] is written to: the perpendicular and parallel
/// colours (`[r, g, b, 0]`; `None` on a stem without them) and alphas
/// (`[0, 0, 0, a]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TintAt {
    pub colors: Option<[usize; 2]>,
    pub alphas: [usize; 2],
}

/// A transparent shader's colour and its view-angle fade, written into
/// the [`Layout::tint`] vectors.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tint {
    /// The colour the stem's output is multiplied by (both angles).
    pub color: [f32; 3],
    /// The alpha where the surface faces the eye.
    pub perpendicular: f32,
    /// The alpha at a grazing angle.
    pub parallel: f32,
}

impl Tint {
    /// `color` with Halo 1's framebuffer fade mode `fade` (`schi`, `scex`,
    /// `sotr` `+0x2e`: 0 none, 1 fade when perpendicular, 2 fade when
    /// parallel) as the two alphas: fading when perpendicular is alpha 0
    /// facing the eye and 1 grazing; when parallel, the reverse (a light
    /// cone's silhouette fades out). Another value fades nothing.
    pub fn faded(color: [f32; 3], fade: u16) -> Self {
        let (perpendicular, parallel) = match fade {
            FADE_PERPENDICULAR => (0.0, 1.0),
            FADE_PARALLEL => (1.0, 0.0),
            _ => (1.0, 1.0),
        };
        Self {
            color,
            perpendicular,
            parallel,
        }
    }
}

/// Halo 1's transparent framebuffer fade mode (`+0x2e`) "fade when
/// perpendicular".
pub const FADE_PERPENDICULAR: u16 = 1;
/// "Fade when parallel".
pub const FADE_PARALLEL: u16 = 2;

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

/// The ammunition counter's animated tree on `illum_detail`
/// ([`ILLUM_DETAIL`]): retail's battle rifle digits'. V (Coagulation's
/// `objects\weapons\rifle\battle_rifle\shaders\tens` and `ones`, the
/// stem's only instances there; `tests/shad_rebuild.rs`,
/// `ammunition_counter_instances_share_one_tree`): layer 7 of 25
/// animated, one vertex binding (target 177, parameter 1) and one bitmap
/// binding (target 0: bitmap 0's frame), both reading the ten components
/// [`counter_properties`] writes. Component 0, the frame, is the
/// weapon's digit; 1 to 3 are one and 4 to 9 zero (an identity texture
/// transform).
pub const COUNTER_TREE: AnimTree = AnimTree {
    layers: 25,
    animated_layer: 7,
    implementation: [(1, 1), (1, 0), (1, 0), (0, 0), (0, 1)],
    bindings: &[[177, 1, 0, 0], [0, 0, 0, 0]],
};

/// The Halo 2 weapon functions giving one digit of the rounds loaded in
/// its first magazine, by place (0 the ones, 1 the tens). E: the weapon's
/// function handler (`0x000ff600`..`0x000ffc57`) answers
/// `primary_ammunition_ones` (`0x1700056b`) with `(weapon +0x22c) mod 10`
/// and `primary_ammunition_tens` (`0x1700056c`) with `(weapon +0x22c / 10)
/// mod 10`, each times 0.1 (`[0x45dc68]`) and clamped to 0..1, when the
/// definition has a magazine (`weap +0x2c0`); `+0x22c` is magazine 0's
/// rounds loaded (`i16`, the value `primary_ammunition` divides by the
/// magazine's loaded maximum, `0x000ffadf`).
pub const AMMUNITION_DIGITS: [&str; 2] = ["primary_ammunition_ones", "primary_ammunition_tens"];

/// A counter digit's frames: a digit's bitmap holds one image per digit,
/// 0 to 9 in order (Halo 1's `numbers_plate` and retail's).
pub const COUNTER_FRAMES: f32 = 10.0;

/// One animated property of a `shad`: what drives it (an object function
/// by name, or game time with `None`), its period in seconds when time
/// drives it, and its function blob (`tags::effects::function`).
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub input: Option<&'static str>,
    pub period: f32,
    pub function: Vec<u8>,
}

/// The ten properties of a counter digit ([`COUNTER_TREE`]) driven by the
/// object function `input` ([`AMMUNITION_DIGITS`] or [`COUNTDOWN_DIGITS`]): component 0 is
/// the input mapped by an identity onto 0..[`COUNTER_FRAMES`], so the
/// handler's `digit × 0.1` picks frame `digit`; the rest an identity
/// transform. V: retail's `tens` and `ones` hold exactly these (property
/// 0 `primary_ammunition_tens` or `_ones`, period 1, an identity over 0
/// to 10; 1 to 3 the constant 1, 4 to 9 the constant 0).
pub fn counter_properties(input: &'static str) -> Vec<Property> {
    use tags::effects::function::{Function, FunctionType};
    let mut out: Vec<Property> = TextureScroll {
        scale: [1.0, 1.0],
        offset: [None, None],
    }
    .properties()
    .into_iter()
    .map(|(period, function)| Property {
        input: None,
        period,
        function,
    })
    .collect();
    out[0] = Property {
        input: Some(input),
        period: 1.0,
        function: Function {
            kind: FunctionType::Identity,
            flags: 0,
            sub_types: [0, 0],
            range: [0.0, COUNTER_FRAMES],
            unknown_0c: [0; 8],
            payload: Vec::new(),
        }
        .to_bytes(),
    };
    out
}

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
    tint: None,
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
    tint: None,
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
    tint: None,
};

/// [`TEX_BUMP_ENV`] with the object's change colours: Halo 2's player
/// armour stem (the Chief's `masterchief`, the Arbiter's first-person
/// arms). Its colour pass draws base × detail × 2 times, for each change
/// colour `k`, `lerp(1, colour k, cc.channel k)`, the change-colour map
/// (bitmap 4, sampled at the base's coordinates) weighing colour 0 by its
/// red, 1 by green, 2 by blue, 3 by alpha, each colour an engine
/// constant read from the drawn object (`h2 shader`: stages 1 to 4 of the
/// pass's `c0`). That is Halo 1's model shader rule for its one change
/// colour (the multipurpose map's mask tints the diffuse toward the
/// colour), so a `soso` with a change colour draws here with its mask in
/// the channel of the Halo 2 slot its colour goes to
/// ([`halo2_change_colour_slot`]); a `soso` without a reflection draws
/// here with the black default cube, as retail's `elite_fp_leather` does.
///
/// The rest is [`TEX_BUMP_ENV`]'s, in other places (D, the three retail
/// instances in `shared.map`): bitmap 0 the bump, 1 a second bump
/// (`default_vector` on two of three), 2 the base, 3 the detail, 5 the
/// cube, 6 the corner fade; vector 0 the detail's tiling, 1 the base's,
/// 3 the bump's, 2, 4 and 5 constants. The environment pass reads the
/// reflection tints facing the eye and grazing as `v[10]` and `v[11]`:
/// colours 4 and 5 (vectors 6 and 7 their copies) with the brightnesses
/// as colours 6 and 7's alphas (vectors 8 and 9). Colours 0 and 1
/// (vectors 10 and 11) are the lightmap highlight tints, black as on
/// [`TEX_BUMP_ENV`] (Halo 1 has none); colours 2 and 3, black (both on
/// `elite_fp_leather`).
pub const TEX_BUMP_ENV_FOUR_CHANGE_COLOR: Layout = Layout {
    stem: "shaders\\shader_templates\\opaque\\tex_bump_env_four_change_color",
    word_14: 1,
    ints: [1, 0],
    bitmaps: &[
        Bm::Role(Role::Bump, Fixed::DefaultVector),
        Bm::Fixed(Fixed::DefaultVector),
        Bm::Role(Role::Base, Fixed::ColorWhite),
        Bm::Role(Role::Detail, Fixed::DefaultDetail),
        Bm::Role(Role::ChangeColour, Fixed::DefaultAdditive),
        Bm::Role(Role::Cube, Fixed::DefaultCube),
        Bm::Fixed(Fixed::CornerFade),
    ],
    colors: &[0, 0, 0, 0, WHITE, WHITE, OPAQUE_ALPHA, OPAQUE_ALPHA],
    vectors: &[
        Vector::Tile(Role::Detail),
        Vector::Tile(Role::Base),
        Vector::Const([0.0, 0.0, 1.0, 1.0]),
        Vector::Tile(Role::Bump),
        Vector::Const([0.276, 0.276, 0.93, 0.0]),
        Vector::Const([0.39, 0.0, 0.93, 0.0]),
        Vector::ColorOf(4),
        Vector::ColorOf(5),
        Vector::AlphaOf(6),
        Vector::AlphaOf(7),
        Vector::ColorOf(0),
        Vector::ColorOf(1),
    ],
    root: Root::BaseMap,
    animation: None,
    tint: None,
};

/// [`TEX_BUMP_ENV`]'s reflection colours (1 to 4: tints facing the eye
/// and grazing, their brightnesses) where
/// [`TEX_BUMP_ENV_FOUR_CHANGE_COLOR`] keeps them (4 to 7).
pub fn four_change_color_extras(env: &Extras) -> Extras {
    Extras {
        colors: env
            .colors
            .iter()
            .filter(|(k, _)| (1..=4).contains(k))
            .map(|&(k, c)| (k + 3, c))
            .collect(),
        scroll: env.scroll,
        ..Extras::default()
    }
}

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
    tint: None,
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
    tint: None,
};

/// Self-illuminated over a detail, the ammunition counter's stem: `h2
/// shader` on retail's battle rifle `tens` draws `t0 × t1 × 2 × c0`
/// added (one/one) on layer 7, alpha-tested on its brightness, bitmap 0
/// the digits (`numbers_plate`, ten images) on stage 0 and bitmap 1 a
/// detail on stage 1; bitmap 2 null. Vector 0 the digits' tiling, 1 and 2
/// the unit rows, 3 the detail's tiling (retail 8×8 over its
/// `wacky_globs`). Here the detail is the plain grey default, so the
/// digit is drawn as Halo 1's additive `schi` draws it, and colour 0 is
/// white: Halo 1's digits carry their own colour (retail tints its white
/// ones cyan). [`COUNTER_TREE`] picks the digit. In `shared.map`.
pub const ILLUM_DETAIL: Layout = Layout {
    stem: "shaders\\shader_templates\\opaque\\illum_detail",
    word_14: 1,
    ints: [0, 0],
    bitmaps: &[
        Bm::Role(Role::Base, Fixed::ColorWhite),
        Bm::Role(Role::Detail, Fixed::DefaultDetail),
        Bm::Null,
    ],
    colors: &[WHITE],
    vectors: &[
        Vector::Tile(Role::Base),
        Vector::Const(U1),
        Vector::Const(V1),
        Vector::Tile(Role::Detail),
    ],
    root: Root::BaseMap,
    animation: Some(&COUNTER_TREE),
    tint: None,
};

/// Additive: stage 1 bitmap 0, its rows vectors 0 and 1; vectors 2 to 5
/// the [`Tint`] (white, no fade, by default). `h2 shader` on retail's
/// `teleporter_emitter_core`: `rgb = t1.rgb × c0.rgb × v0.rgb × v0.a`,
/// `v[10]` = vector 2 + vector 4, `v[11]` = vector 3 + vector 5.
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
    tint: Some(TintAt {
        colors: Some([2, 3]),
        alphas: [4, 5],
    }),
};

/// Additive, two maps multiplied: `rgb = t1.rgb × t2.rgb × 2 × v0.rgb ×
/// c0.a × v0.a`, blend one/one, drawn two-sided (`CULLMODE` 0 in the
/// pass). Bitmap 0 on stage 2, its rows vectors 0 and 1 (`c[22]`,
/// `c[23]`), the one [`CLOUDS_TREE`] scrolls; bitmap 1 on stage 1, rows
/// vectors 2 and 3 (`c[20]`, `c[21]`); vectors 4 to 7 the [`Tint`];
/// colour 0's alpha the `c0.a` (opaque). D (`h2 shader` over the disc's
/// 53 retail instances): retail's `elite_jetpack_exhaust` has its
/// exhaust as bitmap 0 on stage 2 and its mask as bitmap 1 on stage 1,
/// `vector[1]` in `c[23]`; `cr_stream` animates exactly the clouds tree
/// (bitmap target 1, vertex targets 212 and 245, layer 15) and
/// `cr_illumination` is unanimated with colour 0 `0xff000000`; an unused
/// bitmap 1 is `default_multiplicative`. In Coagulation, Beaver Creek and
/// 05a, not in `shared.map`.
pub const TWO_ADD_CLOUDS: Layout = Layout {
    stem: "shaders\\shader_templates\\transparent\\two_add_clouds",
    word_14: 5,
    ints: [0, 0],
    bitmaps: &[
        Bm::Role(Role::Tile, Fixed::ColorWhite),
        Bm::Role(Role::Mask, Fixed::DefaultMultiplicative),
    ],
    colors: &[OPAQUE_ALPHA],
    vectors: &[
        Vector::URow(Role::Tile),
        Vector::VRow(Role::Tile),
        Vector::URow(Role::Mask),
        Vector::VRow(Role::Mask),
        Vector::Const(W1),
        Vector::Const(W1),
        Vector::Const(A1),
        Vector::Const(A1),
    ],
    root: Root::None,
    animation: Some(&CLOUDS_TREE),
    tint: Some(TintAt {
        colors: Some([4, 5]),
        alphas: [6, 7],
    }),
};

/// Plasma, added: two noise maps whose difference, squared and
/// sharpened, lights thin moving bands (`h2 shader` on retail's
/// `teleporter_plasma`: `r0 = t1 − t2`, squared ×4, `bx2` squared twice),
/// coloured between colour 1 and colour 0 by the band's strength, times
/// the mask's alpha and the fade alpha (`rgb = t3.a × v0.a × band`, blend
/// one/one). Bitmap 0 on stage 1 (rows vectors 0 and 1, `c[20]`,
/// `c[21]`), bitmap 1 on stage 2 (rows 2 and 3, `c[22]`, `c[23]`, the
/// pair [`CLOUDS_TREE`] scrolls), bitmap 2 the mask on stage 3 (rows 4
/// and 5). Vector 7 is `v[10]` and vector 6 `v[11]`: the perpendicular
/// and parallel alphas (retail's `teleporter_plasma` and `grav_lift` 1
/// and 0, the receiver and `needler_ammo_glow` 1 and 1). Colour 2 black
/// on every retail instance; colour 3's alpha 0 leaves the fade alone.
/// D: `needler_ammo_glow` animates exactly the clouds tree (bitmap
/// target 1, vertex targets 212 and 245); retail's teleporter animates
/// bitmap 0's pair too, which this doesn't. In `shared.map`.
pub const PLASMA_ALPHA: Layout = Layout {
    stem: "shaders\\shader_templates\\transparent\\plasma_alpha",
    word_14: 5,
    ints: [1, 0],
    bitmaps: &[
        Bm::Role(Role::Base, Fixed::ColorWhite),
        Bm::Role(Role::Tile, Fixed::ColorWhite),
        Bm::Role(Role::Mask, Fixed::ColorWhite),
    ],
    colors: &[WHITE, WHITE, 0, 0],
    vectors: &[
        Vector::URow(Role::Base),
        Vector::VRow(Role::Base),
        Vector::URow(Role::Tile),
        Vector::VRow(Role::Tile),
        Vector::URow(Role::Mask),
        Vector::VRow(Role::Mask),
        Vector::Const(A1),
        Vector::Const(A1),
    ],
    root: Root::None,
    animation: Some(&CLOUDS_TREE),
    tint: Some(TintAt {
        colors: None,
        alphas: [7, 6],
    }),
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
    tint: None,
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
    tint: None,
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
    tint: None,
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
    tint: None,
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
    tint: None,
};

/// Every layout.
pub const LAYOUTS: [&Layout; 15] = [
    &TEX_BUMP,
    &TEX_BUMP_ENV,
    &TEX_BUMP_ENV_FOUR_CHANGE_COLOR,
    &TEX_BUMP_DETAIL_BLEND_DETAIL,
    &TEX_BUMP_ALPHA_TEST,
    &ILLUM,
    &ILLUM_DETAIL,
    &ONE_ADD_ILLUM,
    &TWO_ADD_CLOUDS,
    &PLASMA_ALPHA,
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
    /// Retain original reflection endpoints in a converted model shader so the
    /// cinematic renderer can release its override after a checkpoint or a cut.
    pub cinematic_reflection: bool,
    /// The colour and fade, on a layout with [`Layout::tint`] vectors;
    /// `None` keeps the layout's (white, no fade).
    pub tint: Option<Tint>,
    /// The [`Role::Mask`] map is to be bound as a derived bitmap: the
    /// Halo 1 picture with this gain times its brightest channel as alpha
    /// (`h1_assets::bitmaps::mask::convert_brightest`, [`plasma_shader`]); the
    /// caller converting the maps makes it. `None`: the map as it is.
    pub brightest_mask: Option<f32>,
    /// The weapon function picking the layout's animated map's frame, one
    /// of [`AMMUNITION_DIGITS`] ([`counter_properties`]); over
    /// [`Self::scroll`]. `None`: no counter.
    pub counter: Option<&'static str>,
    /// Separate Halo 1 operations after the details are blended, then
    /// after sampling the micro detail. The retail stem already does [0, 0].
    pub detail_blend: [DetailBlend; 2],
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
    detail: Option<detail::Program>,
    model_lighting: Option<std::sync::Arc<tags::shader::TemplatePostprocess>>,
    glass_vertex: Option<TagId>,
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
        out.glass_vertex = find(base, b"vrtx", program::glass::GLASS_VERTEX);
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
            detail: None,
            model_lighting: None,
            glass_vertex: None,
        }
    }

    /// The stem `layout` draws with on the base.
    pub fn stem(&self, layout: &Layout) -> Option<TagId> {
        self.stems.get(layout.stem).copied()
    }

    pub(crate) fn install_sky_clouds(&mut self, set: &mut TagSet) -> Result<()> {
        if self.stem(&SKY_CLOUDS).is_none() {
            let id = program::clouds::install(set)?;
            self.stems.insert(SKY_CLOUDS.stem, id);
        }
        Ok(())
    }

    /// Whether the base has `layout`'s stem and every default bitmap it
    /// needs.
    pub fn has(&self, layout: &Layout) -> bool {
        if layout.stem == program::glass::CLEAR.stem {
            return self.glass_vertex.is_some();
        }
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
        let stem = self.stem(layout).ok_or(Error::Invalid("the base lacks the stem"))?;
        self.record_on_stem(layout, maps, extras, stem)
    }

    fn record_on_stem(
        &self,
        layout: &Layout,
        maps: &HashMap<Role, Bound>,
        extras: &Extras,
        stem: TagId,
    ) -> Result<Record> {
        let colors = extras.colors_of(layout);
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
                Vector::AlphaOf(k) => {
                    let c = colors.get(*k).copied().unwrap_or(0);
                    [0.0, 0.0, 0.0, (c >> 24) as f32 / 255.0]
                }
            })
            .collect();
        let mut vectors = vectors;
        if let (Some(at), Some(t)) = (layout.tint, extras.tint) {
            let [r, g, b] = t.color;
            let [pa, qa] = at.alphas;
            let mut put = vec![
                (pa, [0.0, 0.0, 0.0, t.perpendicular]),
                (qa, [0.0, 0.0, 0.0, t.parallel]),
            ];
            if let Some([pc, qc]) = at.colors {
                put.push((pc, [r, g, b, 0.0]));
                put.push((qc, [r, g, b, 0.0]));
            }
            for (k, v) in put {
                if let Some(x) = vectors.get_mut(k) {
                    *x = v;
                }
            }
        }
        let picture = match layout.root {
            Root::None => None,
            _ => layout
                .bitmaps
                .iter()
                .position(|b| matches!(b, Bm::Role(Role::Base, _))),
        };
        let scroll = match (layout.animation, extras.scroll) {
            (Some(_), Some(offset)) if offset.iter().any(Option::is_some) => {
                // The tile is the scrolled map where the layout has one
                // (`PLASMA_ALPHA`'s is its second bitmap).
                let tile = layout
                    .bitmaps
                    .iter()
                    .any(|b| matches!(b, Bm::Role(Role::Tile, _)))
                    .then_some(Role::Tile);
                let r = tile.or_else(|| {
                    layout.bitmaps.iter().find_map(|b| match b {
                        Bm::Role(r, _) if *r != Role::Mask && *r != Role::Cube => Some(*r),
                        _ => None,
                    })
                });
                Some(TextureScroll {
                    scale: r.map_or([1.0, 1.0], |r| scale(&r)),
                    offset,
                })
            }
            _ => None,
        };
        let mut colors = colors;
        if extras.cinematic_reflection {
            if let Some(first) = model_reflection_start(colors.len()) {
                colors.extend_from_within(first..first + 4);
                colors.push(MODEL_REFLECTION_SIGNATURE);
            }
        }
        let mut rec = build_record(layout, stem, &bitmaps, &colors, &vectors, picture)?;
        if let Some(tree) = layout.animation {
            let props = match (extras.counter, scroll) {
                (Some(input), _) => Some(counter_properties(input)),
                (None, Some(scroll)) => Some(
                    scroll
                        .properties()
                        .into_iter()
                        .map(|(period, function)| Property {
                            input: None,
                            period,
                            function,
                        })
                        .collect(),
                ),
                (None, None) => None,
            };
            if let Some(props) = props {
                animate(&mut rec, tree, &props)?;
            }
        }
        Ok(rec)
    }

    /// Read the terrain's other layer connections once when a map uses
    /// a non-default detail program.
    pub(crate) fn load_detail_program<R: Read + Seek>(&mut self, base: &mut Map<R>) -> Result<()> {
        if self.detail.is_none() {
            let stem = self
                .stem(&TEX_BUMP_DETAIL_BLEND_DETAIL)
                .ok_or(Error::Invalid("the base lacks the detail-blend stem"))?;
            self.detail = Some(detail::Program::read(base, stem)?);
        }
        Ok(())
    }

    /// Rebuild a structure shader with the detail mode selected by its senv.
    pub(crate) fn build_surface(
        &self,
        set: &mut map::build::TagSet,
        layout: &Layout,
        maps: &HashMap<Role, Bound>,
        extras: &Extras,
    ) -> Result<Block> {
        if layout.stem == program::glass::CLEAR.stem {
            use map::build::TagPayload;
            use program::glass;
            let group = FourCC(*b"stem");
            let id = if let Some(id) = set.converted(group, layout.stem) {
                id
            } else {
                let vertex = self
                    .glass_vertex
                    .ok_or(Error::Invalid("missing glass vertex shader"))?;
                let pass_group = FourCC(*b"spas");
                set.add_group(pass_group, None, None);
                let pass = set.add_tag(
                    pass_group,
                    layout.stem,
                    TagPayload::Built(glass::clear_pass(vertex)?),
                )?;
                set.add_group(group, None, None);
                let id = set.add_tag(
                    group,
                    layout.stem,
                    TagPayload::Built(glass::clear_template(pass)?),
                )?;
                set.record_conversion(group, layout.stem, id);
                id
            };
            return glass::clear_instance(id);
        }
        if !uses_detail_program(layout, extras) {
            return self.build_with(layout, maps, extras);
        }
        let program = self
            .detail
            .as_ref()
            .ok_or(Error::Invalid("missing terrain detail program"))?;
        let stem = program.install(set, detail::active(extras.detail_blend, maps))?;
        let record = self.record_on_stem(layout, maps, extras, stem)?;
        defs::to_built(shad_def(), &record, &mut |_| None)
            .map_err(|e| Error::Textures(format!("shad: {e}")))
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

/// Writes `tree` with `props` (property `k` driving component `k`) into
/// the record's postprocess.
fn animate(rec: &mut Record, tree: &AnimTree, props: &[Property]) -> Result<()> {
    animate_ranges(rec, tree, props, props.len())
}

/// Each group supplies one property range. Texture transforms consume several
/// components together, while scalar register writes each consume one.
fn animate_ranges(rec: &mut Record, tree: &AnimTree, props: &[Property], width: usize) -> Result<()> {
    if !(1..=127).contains(&width) || props.len() > 512 || props.len() % width != 0 {
        return Err(Error::Invalid("invalid shader animation property range"));
    }
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
    let precs = props
        .iter()
        .map(|p| {
            let input = match p.input {
                Some(name) => Str::Text(name.to_owned()),
                None => Str::Null,
            };
            mk(
                "properties",
                &[
                    ("input", Val::String(input)),
                    ("input_b", Val::Float(p.period)),
                    ("function", Val::Data(p.function.clone())),
                ],
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let n = props.len() as u16;
    set(&mut pp, "properties", Val::Block(precs))?;
    let refs = (0..n)
        .map(|k| {
            mk(
                "property_refs",
                &[("property", Val::Int(i64::from(k))), ("component", Val::Int(i64::from(k % width as u16)))],
            )
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut pp, "property_refs", Val::Block(refs))?;
    set(
        &mut pp,
        "property_ranges",
        Val::Block((0..n).step_by(width).map(|start| {
            mk("property_ranges", &[("property_refs", range16(start, width as u16))])
        }).collect::<Result<Vec<_>>>()?),
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

/// Native draw dependencies for a player emblem, which has no Halo 1 shader.
pub const EMBLEM_STEM: &str = r"shaders\shader_templates\opaque\emblem_overlay_simple";
pub const EMBLEM_BITMAP: &str = r"ui\global_bitmaps\emblems\foreground";

/// Build a player-emblem instance. The native program supplies the team/member
/// colours. The instance binds the foreground frame, toggle and active inputs.
pub fn emblem(stem: TagId, foreground: TagId) -> Result<Block> {
    use tags::effects::function::{Function, FunctionType};

    const FRAMES: u16 = 64;
    let layout = Layout {
        stem: EMBLEM_STEM,
        word_14: 1,
        ints: [0, 0],
        bitmaps: &[],
        colors: &[],
        vectors: &[],
        root: Root::None,
        animation: None,
        tint: None,
    };
    let foreground = Bound {
        id: foreground,
        scale: [1.0, 1.0],
        lod_offset: 3.363171,
    };
    let mut root = build_record(
        &layout,
        stem,
        &[Some(foreground)],
        &[0; 2],
        &[[1.0, 1.0, 0.0, 0.0]],
        None,
    )?;
    set(&mut root, "scalar_input", Val::Float(0.0))?;
    let image_def = child(shad_def(), "bitmap_images")?;
    let images = (0..FRAMES)
        .map(|image| {
            let mut row = defs::default_record(image_def);
            set(&mut row, "image", Val::Int(i64::from(image)))?;
            set(&mut row, "bitmap", datum(Some(foreground.id)))?;
            Ok(row)
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut root, "bitmap_images", Val::Block(images))?;
    let ppdef = child(shad_def(), "postprocess")?;
    let Some(Val::Block(postprocess)) = root.at_mut("postprocess") else {
        return Err(Error::Invalid("emblem shader lacks its postprocess"));
    };
    let pp = &mut postprocess[0];
    let rows = |path: &str, values: Vec<Vec<(&str, Val)>>| -> Result<Val> {
        let def = child(ppdef, path)?;
        values
            .into_iter()
            .map(|fields| {
                let mut row = defs::default_record(def);
                for (field, value) in fields {
                    set(&mut row, field, value)?;
                }
                Ok(row)
            })
            .collect::<Result<Vec<_>>>()
            .map(Val::Block)
    };
    set(
        pp,
        "lods",
        rows(
            "lods",
            vec![vec![
                ("layer_mask", Val::Int(1 << 2)),
                ("layers", range16(0, 25)),
            ]],
        )?,
    )?;
    set(
        pp,
        "layers",
        rows(
            "layers",
            (0..25)
                .map(|layer| {
                    vec![(
                        "passes",
                        range16(u16::from(layer > 2), u16::from(layer == 2)),
                    )]
                })
                .collect(),
        )?,
    )?;
    set(
        pp,
        "passes",
        rows("passes", vec![vec![("implementations", range16(0, 1))]])?,
    )?;
    set(
        pp,
        "implementations",
        rows(
            "implementations",
            vec![vec![
                ("bitmap_animations", range16(3, 1)),
                ("render_state_animations", range16(3, 0)),
                ("stage_state_animations", range16(3, 0)),
                ("pixel_constant_animations", range16(0, 2)),
                ("vertex_constant_animations", range16(2, 1)),
            ]],
        )?,
    )?;

    // Two scalar properties bracket the ten texture-transform components.
    // Component 0 selects the frame; 1–3 are unity and 4–9 are zero.
    let mut properties = vec![Vec::new(); 12];
    for (i, row) in properties.iter_mut().enumerate() {
        let (input, maximum) = match i {
            0 => ("emblem_toggle_foreground", 1.0),
            1 => ("emblem_foreground_index", f32::from(FRAMES)),
            11 => ("emblem_active", 1.0),
            _ => ("", if i < 5 { 1.0 } else { 0.0 }),
        };
        let function = if input.is_empty() {
            Function {
                kind: FunctionType::Constant,
                flags: 0,
                sub_types: [0, 0],
                range: [maximum; 2],
                unknown_0c: [0; 8],
                payload: [1.0f32.to_le_bytes(); 2].concat(),
            }
        } else {
            row.push(("input", Val::text(input)));
            Function {
                kind: FunctionType::Identity,
                flags: 0,
                sub_types: [0, 0],
                range: [0.0, maximum],
                unknown_0c: [0; 8],
                payload: Vec::new(),
            }
        };
        row.extend([
            ("input_b", Val::Float(1.0)),
            ("function", Val::Data(function.to_bytes())),
        ]);
    }
    set(pp, "properties", rows("properties", properties)?)?;
    set(
        pp,
        "property_refs",
        rows(
            "property_refs",
            (0..12)
                .map(|i| {
                    vec![
                        ("property", Val::Int(i)),
                        ("component", Val::Int(if i == 11 { -1 } else { i - 1 })),
                    ]
                })
                .collect(),
        )?,
    )?;
    set(
        pp,
        "property_ranges",
        rows(
            "property_ranges",
            vec![
                vec![("property_refs", range16(0, 1))],
                vec![("property_refs", range16(1, 10))],
                vec![("property_refs", range16(11, 1))],
            ],
        )?,
    )?;
    set(
        pp,
        "animations",
        rows(
            "animations",
            [[128, 0, 3, 0], [0, 1, 3, 2], [176, 1, 0, 1], [0, 0, 0, 1]]
                .map(|b| {
                    vec![
                        ("target", Val::Int(b[0])),
                        ("param_1", Val::Int(b[1])),
                        ("param_2", Val::Int(b[2])),
                        ("property_range", Val::Int(b[3])),
                    ]
                })
                .to_vec(),
        )?,
    )?;
    defs::to_built(shad_def(), &root, &mut |_| None)
        .map_err(|e| Error::Textures(format!("emblem shader: {e}")))
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
/// part of their texture (62 % of the broadleaf's) drawn too. The stem
/// alone left the cards whole: its depth and shadow passes kill by its
/// bump map's alpha (slot 0), not the base's, so the object converter
/// binds a flat bump cut out where the base is
/// (`h1_assets::bitmaps::mask::cutout_bump`, converter version 92; shown live on
/// Blood Gulch's whitebark tree, docs/halo1/objects.md "Cut-out model
/// shaders").
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
///   A `soso` with a change colour ([`soso_change_colour`]) and no
///   cut-out draws on [`TEX_BUMP_ENV_FOUR_CHANGE_COLOR`] first, with its
///   reflection if any, its multipurpose map in the change-colour role,
///   falling back to the above.
/// - `swat` with a ripple map: [`WATER_POOL`] (`swat_water`), falling
///   back to [`ONE_ALPHA_ENV`] with the base map, as without one.
/// - `sgla`: [`ONE_ALPHA_ENV`] with the base map.
/// - `smet`: [`ILLUM`] with its map.
/// - `spla`: [`ONE_ADD_ILLUM`] with its map.
/// - `schi`, `scex` additive with maps multiplied together
///   ([`multiply_chain`]): [`TWO_ADD_CLOUDS`] (two maps, one scrolling,
///   the rest folded into the tint), falling back to [`ONE_ADD_ILLUM`].
/// - `sotr` additive and one of Halo 1's plasma programs ([`is_plasma`]):
///   [`PLASMA_ALPHA`] ([`plasma_shader`]), falling back to the next rule's.
/// - `schi`, `scex`, `sotr` otherwise: their least tiled map
///   ([`crate::sky_shaders::recipe`]), [`ONE_ADD_ILLUM`] when the
///   framebuffer blend is additive, else [`ONE_ALPHA_ENV`]. Additive ones
///   fade by the view angle as Halo 1's fade mode says ([`Tint::faded`]);
///   Other `sotr` stages (combiners) aren't read.
pub fn read_h1(h1: &h1map::H1Map, tag: &h1map::Tag) -> Option<H1Shader> {
    if let Some(layout) = textureless_layout(tag.group, h1.tag_data(tag).ok()?) {
        return Some(H1Shader {
            name: tag.name.clone(),
            group: tag.group.to_string(),
            layout,
            maps: vec![],
            extras: Extras::default(),
            fallback: None,
        });
    }
    let mut s = read_h1_maps(h1, tag)?;
    s.fit_kinds(h1);
    if s.maps.is_empty() { None } else { Some(s) }
}

/// Match the source draw, including its absent layers. Bad references or
/// additional sampled maps must still go through the full material reader.
pub fn textureless_layout(group: h1map::Group, bytes: &[u8]) -> Option<&'static Layout> {
    let null_maps = |offsets: &[usize]| {
        offsets.iter().all(|&offset| {
            h1map::TagRef::read(bytes, offset)
                .is_some_and(|r| r.group == h1map::Group::new(b"bitm") && r.id.is_none())
        })
    };
    let zero = |at: usize, len: usize| {
        bytes
            .get(at..at + len)
            .is_some_and(|b| b.iter().all(|v| *v == 0))
    };
    if group == h1map::Group::new(b"senv")
        && zero(0x28, 4)
        && null_maps(&[0x88, 0xb8, 0xcc, 0xfc, 0x128, 0x254, 0x324])
        && zero(0x290, 4)
    {
        // 0x774a2 / 0x77500 bind white and neutral detail respectively.
        return Some(&TEX_BUMP);
    }
    // A model shader with no map, no reflection and no change colour (the
    // `invisible` of the spray and waterfall emitters): its base and detail
    // bind white and neutral as a map-less `senv`'s do, the white lit
    // surface `docs/halo1/role-audit.md` gives the waterfall emitter.
    if group == h1map::Group::new(b"soso")
        && zero(0x28, 2)
        && zero(SOSO_CHANGE_COLOUR, 2)
        && null_maps(&[0xa4, SOSO_MULTIPURPOSE, 0xdc, SOSO_CUBE])
        && zero(SOSO_PERPENDICULAR_BRIGHTNESS, 4)
        && zero(SOSO_PARALLEL_BRIGHTNESS, 4)
    {
        return Some(&TEX_BUMP);
    }
    if group == h1map::Group::new(b"sgla")
        && zero(0x28, 2)
        && null_maps(&[0x64, 0xac, 0xc0, 0x158, 0x16c, 0x1a0, 0x1b4])
        && zero(0x8a, 2)
        && zero(0x8c, 4)
        && zero(0x9c, 4)
        && [0x54, 0x58, 0x5c]
            .iter()
            .all(|&at| bytes.get(at..at + 4) == Some(&1f32.to_le_bytes()[..]))
    {
        return Some(&program::glass::CLEAR);
    }
    None
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
            let s = crate::textures::h1_scales_of(h1, tag).ok()?;
            let d = h1.tag_data(tag).ok()?;
            if group == "senv" && m.base.is_none() {
                // Monitor glow has no diffuse texture. Its only sampled
                // image is the illumination map, following its UV scale.
                let light = h1map::TagRef::read(d, 0x254)?;
                if light.group == h1map::Group::new(b"bitm") && !light.id.is_none() {
                    let mut shader = one(&ILLUM, Some(light.id))?;
                    let scale = f32::from_le_bytes(d.get(0x250..0x254)?.try_into().ok()?);
                    shader.maps[0].1.scale = [scale; 2];
                    return Some(shader);
                }
            }
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
                let detail_blend = DetailBlend::read(d);
                if let Some(blend) = senv_blend(h1, d, &maps, detail_blend) {
                    let extras = Extras {
                        detail_blend,
                        ..Extras::default()
                    };
                    let mut b = mk(&TEX_BUMP_DETAIL_BLEND_DETAIL, blend, extras);
                    b.fallback = Some(Box::new(out));
                    out = b;
                }
                // A blended surface keeps its blend: no reflection stem
                // on every base blends two details.
                let blended = d
                    .get(SENV_TYPE..SENV_TYPE + 2)
                    .is_some_and(|b| u16::from_le_bytes([b[0], b[1]]) != 0);
                if !blended
                    && !uses_detail_program(out.layout, &out.extras)
                    && let Some((cube, extras)) = senv_reflection(h1, d)
                {
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
            let reflection = (group == "soso" && !alpha_tested)
                .then(|| soso_reflection(h1, d))
                .flatten();
            if let Some((cube, extras)) = &reflection {
                let mut env = maps.clone();
                env.push((Role::Cube, *cube));
                let mut e = mk(&TEX_BUMP_ENV, env, extras.clone());
                e.fallback = Some(Box::new(out));
                out = e;
            }
            // A change colour draws on the change-colour stem, with the
            // reflection when there is one (else the black cube); the
            // multipurpose map stands for the change-colour map, which
            // the object converter makes from its blue
            // (`h1_assets::bitmaps::mask::channel_into`). A cut-out `soso` keeps
            // its cut-out: no stem here alpha-tests with change colours.
            if !alpha_tested && let Some((_, multipurpose)) = soso_change_colour(h1, tag) {
                let mut cc = maps.clone();
                let mut extras = Extras::default();
                if let Some((cube, env)) = &reflection {
                    cc.push((Role::Cube, *cube));
                    extras = four_change_color_extras(env);
                }
                cc.push((
                    Role::ChangeColour,
                    H1Bound {
                        bitmap: multipurpose,
                        scale: [1.0, 1.0],
                    },
                ));
                let mut c = mk(&TEX_BUMP_ENV_FOUR_CHANGE_COLOR, cc, extras);
                c.fallback = Some(Box::new(out));
                out = c;
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
            let d = h1.tag_data(tag).ok()?;
            let fade = d
                .get(TRANSPARENT_FADE..TRANSPARENT_FADE + 2)
                .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
            if s.additive
                && let Some(chained) = chicago_maps(h1, d, s.chicago)
                && let Some(chain) = multiply_chain(&chained)
            {
                return Some(chain.shader(&tag.name, &group, &chained, fade));
            }
            let plasma = (s.additive && !s.chicago)
                .then(|| generic_maps(h1, d))
                .flatten()
                .filter(|(maps, stages)| is_plasma(maps, *stages));
            let mut out = H1Shader {
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
            };
            if s.additive {
                out.extras.tint = Some(Tint::faded([1.0; 3], fade));
            }
            if let Some((maps, _)) = plasma
                && let Some(hue) = h1_hue(h1, maps[3].bound.bitmap)
            {
                let light = PlasmaLight {
                    hue,
                    glow: plasma_glow(h1, d).unwrap_or([0.0; 3]),
                };
                return Some(plasma_shader(&tag.name, &maps, fade, light, out));
            }
            Some(out)
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

/// The framebuffer fade mode of `schi`, `scex` and `sotr` ([`Tint::faded`]).
pub const TRANSPARENT_FADE: usize = 0x2e;
/// `schi` / `scex` flags byte: bit 3 the first map is in screen space.
pub const CHICAGO_FLAGS: usize = 0x29;
pub const CHICAGO_SCREENSPACE: u8 = 1 << 3;
/// `schi` flags bit 7: numeric, the shader draws one digit of a number
/// (E, below).
pub const CHICAGO_NUMERIC: u8 = 1 << 7;
/// `schi` numeric counter limit (`u8`): the largest number counted.
pub const CHICAGO_COUNTER_LIMIT: usize = 0x28;
/// `schi` extra flags (`u32`); bit 1 takes the engine's own number
/// ([`H1Numeric::engine_number`]).
pub const CHICAGO_EXTRA_FLAGS: usize = 0x60;
pub const CHICAGO_ENGINE_NUMBER: u32 = 1 << 1;

/// A Halo 1 numeric `schi`: a counter digit. E, Halo 1's model draw
/// (`0x00067ad1`..`0x00067c32` in Halo 1's XBE): a `schi` whose flags
/// (`+0x29`) have bit 7 set and that has a map draws its first map's
/// bitmap at image `digit`, where `digit` is
/// `(n / base^p) mod base`: `base` the bitmap's image count (Halo 1
/// `bitm +0x60`), `p` the permutation the model's shader entry names
/// (`mode +0xdc` element `+0x10`) and `n` the number. `n` is the engine's
/// (`[0x2e34a0]`, through `0x00056aa0`) when extra flags bit 1 is set;
/// otherwise `floor(limit × out + 0.5)` clamped to 0..limit, `limit` the
/// counter limit (`+0x28`) and `out` the drawn object's A out, or its D
/// out when the bitmap has eight images. D (every `schi` on the disc's
/// Blood Gulch): the assault rifle's first-person `numbers` (limit 60, ten
/// digit images, named twice by its model: permutation 1 on the left
/// digit, 0 on the right) and `compass` (limit 8, eight images).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct H1Numeric {
    pub limit: u8,
    /// The first map's bitmap and its u and v scale.
    pub bitmap: h1map::TagId,
    pub scale: [f32; 2],
    /// The bitmap's image count: the digits' base.
    pub images: usize,
    /// Extra flags [`CHICAGO_ENGINE_NUMBER`].
    pub engine_number: bool,
}

impl H1Numeric {
    /// An engine-number shader reads the digit selected by the model's
    /// shader permutation through the Halo 1 routine at `0x56aa0`.
    pub fn countdown_digit(&self, permutation: i16) -> Option<&'static str> {
        if !self.engine_number || self.images != COUNTER_FRAMES as usize {
            return None;
        }
        COUNTDOWN_DIGITS.get(usize::try_from(permutation).ok()?).copied()
    }

    /// The Halo 2 weapon function ([`AMMUNITION_DIGITS`]) drawing digit
    /// `permutation` of this counter on a weapon whose A out is its
    /// rounds loaded over `loaded_maximum` (Halo 1's primary ammunition):
    /// Halo 1 counts `floor(limit × rounds / loaded_maximum + 0.5)`, the
    /// rounds themselves when the limit is the loaded maximum, in base 10
    /// when the bitmap has ten images. `None` for any other counter, or a
    /// place Halo 2 has no function for (hundreds and up).
    pub fn ammunition_digit(&self, permutation: i16, loaded_maximum: i16) -> Option<&'static str> {
        if self.engine_number
            || self.images != COUNTER_FRAMES as usize
            || i16::from(self.limit) != loaded_maximum
        {
            return None;
        }
        AMMUNITION_DIGITS
            .get(usize::try_from(permutation).ok()?)
            .copied()
    }
}

/// Reads the Halo 1 shader `tag` as a numeric `schi` ([`H1Numeric`]);
/// `None` for another group, a `schi` not numeric or one whose first map
/// names no bitmap that reads.
pub fn chicago_numeric(h1: &h1map::H1Map, tag: &h1map::Tag) -> Option<H1Numeric> {
    use crate::sky_shaders::{MAPS, SCHI_MAP};
    if tag.group != h1map::Group::new(b"schi") {
        return None;
    }
    let d = h1.tag_data(tag).ok()?;
    if d.get(CHICAGO_FLAGS)? & CHICAGO_NUMERIC == 0 {
        return None;
    }
    let (size, at, scale_at) = SCHI_MAP;
    let block = h1map::Block::read(d, MAPS)?;
    let e = h1.block(block, size).ok()?.get(..size)?;
    let r = h1map::TagRef::read(e, at)?;
    if r.group != h1map::Group::new(b"bitm") || r.id.is_none() {
        return None;
    }
    let f32_at = |o: usize| Some(f32::from_le_bytes(e.get(o..o + 4)?.try_into().ok()?));
    let bitmap = h1map::bitmap::Bitmap::read(h1, h1.tag(r.id).ok()?).ok()?;
    let extra = d
        .get(CHICAGO_EXTRA_FLAGS..CHICAGO_EXTRA_FLAGS + 4)
        .map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    Some(H1Numeric {
        limit: *d.get(CHICAGO_COUNTER_LIMIT)?,
        bitmap: r.id,
        scale: [f32_at(scale_at)?, f32_at(scale_at + 4)?],
        images: bitmap.images.len(),
        engine_number: extra & CHICAGO_ENGINE_NUMBER != 0,
    })
}

/// A counter digit of `n` drawn by the Halo 2 weapon function `input`
/// ([`H1Numeric::ammunition_digit`]): [`ILLUM_DETAIL`] with the digits'
/// bitmap, its frame picked by [`COUNTER_TREE`].
pub fn counter_shader(name: &str, n: &H1Numeric, input: &'static str) -> H1Shader {
    H1Shader {
        name: name.to_owned(),
        group: "schi".into(),
        layout: &ILLUM_DETAIL,
        maps: vec![(
            Role::Base,
            H1Bound {
                bitmap: n.bitmap,
                scale: n.scale,
            },
        )],
        extras: Extras {
            counter: Some(input),
            ..Extras::default()
        },
        fallback: None,
    }
}

/// `schi` / `scex` first map type (`u16`): 0 a 2D map (the others are
/// reflection cubes and object-centred maps).
pub const CHICAGO_FIRST_MAP_TYPE: usize = 0x2a;
/// A `schi` map's colour function (`+0x2c`, `u16`): how this map and the
/// next combine. D (`scenery\teleporter_base\shaders\teleporter_cone`):
/// maps 0 and 1 hold 2, map 2 (the last) 0.
pub const CHICAGO_MAP_FUNCTION: usize = 0x2c;
/// Colour function "multiply": this map times the next.
pub const CHICAGO_MULTIPLY: u16 = 2;
/// "Double multiply": twice the product.
pub const CHICAGO_DOUBLE_MULTIPLY: u16 = 3;

/// One map of a Halo 1 chicago shader as [`multiply_chain`] reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChainMap {
    pub bound: H1Bound,
    /// Its colour function ([`CHICAGO_MAP_FUNCTION`]).
    pub function: u16,
    /// Its u and v scrolling ([`crate::sky_shaders`]: a game-time slide).
    pub scroll: [Option<Slide>; 2],
    /// Its mean colour, 0 to 1 (image 0, the largest level).
    pub mean: [f32; 3],
}

/// An additive chicago shader whose maps multiply together, as the two
/// maps [`TWO_ADD_CLOUDS`] draws: `tile` (bitmap 0, the scrolling slot)
/// and `mask` (bitmap 1, unanimated), indices into the maps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chain {
    pub tile: usize,
    pub mask: usize,
    /// 2 per double multiply.
    pub factor: f32,
}

/// The [`Chain`] of `maps` when every map but the last multiplies into
/// the next ([`CHICAGO_MULTIPLY`], [`CHICAGO_DOUBLE_MULTIPLY`]): the
/// first scrolling map on the scrolling slot (else the first map), the
/// first unanimated other map on the other (else the next one). `None`
/// with fewer than two maps or another function in the chain.
///
/// D: Blood Gulch's teleporter cone (`teleporter_cone`, on every
/// teleporter receiver, additive, fade when parallel, two-sided): grey
/// `base volumetric dust` twice, scrolling at their own rates (u 6 s, v
/// −20 s; u −4 s, v 15 s), times the green vertical ramp
/// `teleporter_dust` (black at the top, 1×1.05). Before converter
/// version 90 the least tiled map alone drew, the first dust: a grey,
/// static cone at full brightness with hard edges and no green.
pub fn multiply_chain(maps: &[ChainMap]) -> Option<Chain> {
    let (_, chained) = maps.split_last()?;
    if chained.is_empty() {
        return None;
    }
    let mut factor = 1.0;
    for m in chained {
        match m.function {
            CHICAGO_MULTIPLY => {}
            CHICAGO_DOUBLE_MULTIPLY => factor *= 2.0,
            _ => return None,
        }
    }
    let scrolls = |k: usize| maps[k].scroll.iter().any(Option::is_some);
    let tile = (0..maps.len()).find(|&k| scrolls(k)).unwrap_or(0);
    let mask = (0..maps.len())
        .find(|&k| k != tile && !scrolls(k))
        .or_else(|| (0..maps.len()).find(|&k| k != tile))?;
    Some(Chain { tile, mask, factor })
}

impl Chain {
    /// The factor times the mean colour of every map not in `drawn`: a
    /// multiplied map the stem has no slot for, folded into the tint (a
    /// scrolling layer averages to its mean colour over its period).
    pub fn folded(&self, maps: &[ChainMap], drawn: &[usize]) -> [f32; 3] {
        let mut c = [self.factor; 3];
        for (k, m) in maps.iter().enumerate() {
            if !drawn.contains(&k) {
                for (x, v) in c.iter_mut().zip(m.mean) {
                    *x *= v;
                }
            }
        }
        c.map(|x| x.clamp(0.0, 1.0))
    }

    /// The shader: [`TWO_ADD_CLOUDS`] with the tile scrolling and the
    /// rest folded into the tint, halved for the stem's doubling; on a
    /// base without the stem, [`ONE_ADD_ILLUM`] drawing the mask with
    /// every other map folded in. Both fade as `fade` says
    /// ([`Tint::faded`]).
    pub fn shader(&self, name: &str, group: &str, maps: &[ChainMap], fade: u16) -> H1Shader {
        let plain = H1Shader {
            name: name.to_owned(),
            group: group.to_owned(),
            layout: &ONE_ADD_ILLUM,
            maps: vec![(Role::Base, maps[self.mask].bound)],
            extras: Extras {
                tint: Some(Tint::faded(self.folded(maps, &[self.mask]), fade)),
                ..Extras::default()
            },
            fallback: None,
        };
        let half = self.folded(maps, &[self.tile, self.mask]).map(|x| x * 0.5);
        H1Shader {
            name: name.to_owned(),
            group: group.to_owned(),
            layout: &TWO_ADD_CLOUDS,
            maps: vec![
                (Role::Tile, maps[self.tile].bound),
                (Role::Mask, maps[self.mask].bound),
            ],
            extras: Extras {
                tint: Some(Tint::faded(half, fade)),
                scroll: Some(maps[self.tile].scroll),
                ..Extras::default()
            },
            fallback: Some(Box::new(plain)),
        }
    }
}

/// The maps of a chicago shader (`d`, its data; `chicago` false for
/// `sotr`, whose stages this doesn't read: `None`) for
/// [`multiply_chain`]. `None` when its first map is in screen space or
/// not a 2D map, or any map lacks a 2D bitmap or its mean colour.
fn chicago_maps(h1: &h1map::H1Map, d: &[u8], chicago: bool) -> Option<Vec<ChainMap>> {
    use crate::sky_shaders::{MAPS, SCHI_ANIMATION, SCHI_MAP};
    if !chicago || d.get(CHICAGO_FLAGS)? & CHICAGO_SCREENSPACE != 0 {
        return None;
    }
    let u16_at =
        |b: &[u8], at: usize| Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?));
    let f32_at =
        |b: &[u8], at: usize| Some(f32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
    if u16_at(d, CHICAGO_FIRST_MAP_TYPE)? != 0 {
        return None;
    }
    let (size, bitmap, scale) = SCHI_MAP;
    let block = h1map::Block::read(d, MAPS)?;
    let bytes = h1.block(block, size).ok()?;
    bytes
        .chunks_exact(size)
        .map(|e| {
            let r = h1map::TagRef::read(e, bitmap)?;
            if r.group != h1map::Group::new(b"bitm")
                || r.id.is_none()
                || h1_texture_kind(h1, r.id) != Some(h1map::bitmap::ImageKind::D2)
            {
                return None;
            }
            Some(ChainMap {
                bound: H1Bound {
                    bitmap: r.id,
                    scale: [f32_at(e, scale)?, f32_at(e, scale + 4)?],
                },
                function: u16_at(e, CHICAGO_MAP_FUNCTION)?,
                scroll: [
                    crate::sky_shaders::slide(e, SCHI_ANIMATION),
                    crate::sky_shaders::slide(e, SCHI_ANIMATION + 0x10),
                ],
                mean: h1_mean_color(h1, r.id)?,
            })
        })
        .collect()
}

/// The mean colour (0 to 1) of the Halo 1 bitmap `id`'s first image at
/// its largest level.
pub fn h1_mean_color(h1: &h1map::H1Map, id: h1map::TagId) -> Option<[f32; 3]> {
    use h1_assets::bitmaps::BitmapExt;
    let t = h1.tag(id).ok()?;
    let b = h1map::bitmap::Bitmap::read(h1, t).ok()?;
    let img = b.decode(h1, 0, 0).ok()?;
    Some(mean_rgb(&img.rgba))
}

/// The mean of RGBA8 texels' red, green and blue, 0 to 1 (black for none).
pub fn mean_rgb(rgba: &[u8]) -> [f32; 3] {
    let n = rgba.len() / 4;
    if n == 0 {
        return [0.0; 3];
    }
    let mut s = [0u64; 3];
    for px in rgba.chunks_exact(4) {
        for (x, v) in s.iter_mut().zip(px) {
            *x += u64::from(*v);
        }
    }
    s.map(|x| (x as f64 / n as f64 / 255.0) as f32)
}

/// `sotr` stages block (count, address), `0x70` per stage: Halo 1's
/// transparent generic combiner program.
pub const SOTR_STAGES: usize = 0x60;
/// Stages of Halo 1's plasma programs ([`is_plasma`]).
pub const SOTR_PLASMA_STAGES: usize = 7;

/// A `sotr`'s maps (each as a [`ChainMap`], function 0) and its stage
/// count, `d` its data. `None` when a map lacks a 2D bitmap or its mean
/// colour.
fn generic_maps(h1: &h1map::H1Map, d: &[u8]) -> Option<(Vec<ChainMap>, usize)> {
    use crate::sky_shaders::{MAPS, SOTR_ANIMATION, SOTR_MAP};
    let f32_at =
        |b: &[u8], at: usize| Some(f32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?));
    let stages = h1map::Block::read(d, SOTR_STAGES)?.count as usize;
    let (size, bitmap, scale) = SOTR_MAP;
    let bytes = h1.block(h1map::Block::read(d, MAPS)?, size).ok()?;
    let maps = bytes
        .chunks_exact(size)
        .map(|e| {
            let r = h1map::TagRef::read(e, bitmap)?;
            if r.group != h1map::Group::new(b"bitm")
                || r.id.is_none()
                || h1_texture_kind(h1, r.id) != Some(h1map::bitmap::ImageKind::D2)
            {
                return None;
            }
            Some(ChainMap {
                bound: H1Bound {
                    bitmap: r.id,
                    scale: [f32_at(e, scale)?, f32_at(e, scale + 4)?],
                },
                function: 0,
                scroll: [
                    crate::sky_shaders::slide(e, SOTR_ANIMATION),
                    crate::sky_shaders::slide(e, SOTR_ANIMATION + 0x10),
                ],
                mean: h1_mean_color(h1, r.id)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some((maps, stages))
}

/// Whether an additive `sotr` is one of Halo 1's plasma programs: seven
/// stages over four maps, the first two the same noise bitmap scrolling
/// two ways, the last two still (an offset or energy mask, then the
/// picture whose alpha shapes it). D (every `sotr` a model or structure
/// draws on the disc): twelve, on four stage programs that differ only
/// in their constants' wiring: the teleporter shield (Blood Gulch and the
/// other multiplayer maps), the jackal's and the field generator's
/// shields, the overshield and active camouflage powerups, the dropship's
/// tractor beam, the needler's core and ammo glow, the gun turret's and
/// index platform's beam tendrils, a50's prison energy door and shuttle
/// bay doors (and b40's structure `b40_shaftenergy`). Every other `sotr`
/// fails one test (0 to 4 stages, or another map pattern).
pub fn is_plasma(maps: &[ChainMap], stages: usize) -> bool {
    let scrolls = |m: &ChainMap| m.scroll.iter().any(Option::is_some);
    stages == SOTR_PLASMA_STAGES
        && maps.len() == 4
        && maps[0].bound.bitmap == maps[1].bound.bitmap
        && scrolls(&maps[0])
        && scrolls(&maps[1])
        && !scrolls(&maps[2])
        && !scrolls(&maps[3])
}

/// An `[r, g, b]` (0 to 1) as a colour parameter, alpha 0.
pub fn rgb_color(c: [f32; 3]) -> u32 {
    let b = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    (b(c[0]) << 16) | (b(c[1]) << 8) | b(c[2])
}

/// The name of a plasma picture `name` converted with its brightness as
/// alpha at `gain` ([`Extras::brightest_mask`]): `<name>_brightness_<gain
/// × 1000>`.
pub fn brightness_mask_name(name: &str, gain: f32) -> String {
    format!("{name}_brightness_{}", (gain * 1000.0).round() as u32)
}

/// The name of the bump map cut out by the base map `name`'s alpha that
/// an alpha-tested model shader binds in its bump slot
/// (`h1_assets::bitmaps::mask::cutout_bump`): `<name>_alpha_test_bump`.
pub fn cutout_bump_name(name: &str) -> String {
    format!("{name}_alpha_test_bump")
}

/// One `sotr` stage's size, and its fields (`d`: the stage): `+0` flags
/// (bit 0 colour mux, bit 1 alpha mux, bit 2 [`STAGE_A_OUT_COLOR0`]),
/// `+0x0c` and `+0x1c` colour 0's lower and upper bounds (alpha, red,
/// green, blue), `+0x3c` the four colour inputs A to D (u16 input, u16
/// mapping each), `+0x54` the A×B + C×D output. D: the input numbering of
/// the teleporter shield's stages (0 zero, 1 one, 2 half, 3 −1, 4 −half,
/// 5 to 8 `t0` to `t3`, 9 and 10 `v0` and `v1`, 11 and 12 `r0` and `r1`,
/// 13 and 14 `c0` and `c1`, then the same 15 to 24 as alphas), mapping 0
/// unsigned, output 1 `r0`.
pub const SOTR_STAGE: usize = 0x70;
/// See [`SOTR_STAGE`]: colour 0 runs between its bounds by the object's A
/// output, not by the stage's own function. D (every `sotr` of five
/// levels): set only on Halo 1's plasma stages whose bounds differ, the
/// first and last stage of each plasma (the jackal's and the generator's
/// shields: the first and the fourth); the jackal's lower bound is its
/// shield's spent look. An object exporting no A output (the
/// teleporter's `teleporter_base`, which has no functions) holds it at
/// the lower bound.
pub const STAGE_A_OUT_COLOR0: u16 = 4;
const STAGE_C0_LOWER: usize = 0x0c;
const STAGE_C0_UPPER: usize = 0x1c;
const STAGE_INPUTS: usize = 0x3c;
const STAGE_SUM_OUT: usize = 0x54;
/// The inputs (input, mapping) of a plasma's last stage that adds the
/// picture's colour twice: `r0.a × t3 + c0 × t3`, into `r0`.
const PICTURE_TIMES_BAND_PLUS_C0: [(u16, u16); 4] = [(21, 0), (8, 0), (13, 0), (8, 0)];

/// The colour a plasma's last stage adds under its bands, as Halo 1
/// draws it on an object without an A output: Halo 1's last stage (`d`
/// the `sotr`'s data) `r0 = r0.a × picture + c0 × picture` adds the
/// picture times colour 0 wherever the bands are dark ([`PlasmaLight`]):
/// colour 0's lower bound when [`STAGE_A_OUT_COLOR0`] drives it (that
/// output 0), else the upper (the stage function "one", 0 in Halo 1's
/// periodic function list whose 6 is the slide [`crate::sky_shaders`]
/// reads). `None` when the last stage isn't that sum (the jackal's shield
/// adds `v1 × (1 − picture.a)` instead). V (the disc): the teleporter
/// shield's lower bound is (0, 0.329, 0.424).
pub fn plasma_glow(h1: &h1map::H1Map, d: &[u8]) -> Option<[f32; 3]> {
    let bytes = h1.block(h1map::Block::read(d, SOTR_STAGES)?, SOTR_STAGE).ok()?;
    let last = bytes.chunks_exact(SOTR_STAGE).last()?;
    let u16_at = |o: usize| u16::from_le_bytes([last[o], last[o + 1]]);
    let f32_at = |o: usize| f32::from_le_bytes(last[o..o + 4].try_into().unwrap());
    let inputs: [(u16, u16); 4] =
        std::array::from_fn(|i| (u16_at(STAGE_INPUTS + 4 * i), u16_at(STAGE_INPUTS + 4 * i + 2)));
    if inputs != PICTURE_TIMES_BAND_PLUS_C0 || u16_at(STAGE_SUM_OUT) != 1 {
        return None;
    }
    let at = if u16_at(0) & STAGE_A_OUT_COLOR0 != 0 {
        STAGE_C0_LOWER
    } else {
        STAGE_C0_UPPER
    };
    // Alpha, red, green, blue.
    Some(std::array::from_fn(|c| f32_at(at + 4 * (c + 1)).clamp(0.0, 1.0)))
}

/// The hue of a Halo 1 bitmap's first image ([`hue`]); `None` when it
/// doesn't decode or is black.
pub fn h1_hue(h1: &h1map::H1Map, id: h1map::TagId) -> Option<[f32; 3]> {
    use h1_assets::bitmaps::BitmapExt;
    let b = h1map::bitmap::Bitmap::read(h1, h1.tag(id).ok()?).ok()?;
    hue(&b.decode(h1, 0, 0).ok()?.rgba)
}

/// RGBA8 texels' colour with its brightest channel 1: each channel's sum
/// over the sum of every texel's brightest channel, then scaled so the
/// largest is 1, so the bright texels set it. `None` for black.
pub fn hue(rgba: &[u8]) -> Option<[f32; 3]> {
    let mut sum = [0u64; 3];
    let mut brightest = 0u64;
    for t in rgba.chunks_exact(4) {
        for c in 0..3 {
            sum[c] += u64::from(t[c]);
        }
        brightest += u64::from(t[0].max(t[1]).max(t[2]));
    }
    if brightest == 0 {
        return None;
    }
    let h = sum.map(|x| x as f64 / brightest as f64);
    let top = h.iter().copied().fold(0.0, f64::max);
    (top > 0.0).then(|| h.map(|x| (x / top) as f32))
}

/// How a Halo 1 plasma's light maps onto [`PLASMA_ALPHA`]. Halo 1 adds
/// `picture.rgb × (band + glow)` (the band 1 except along thin dark
/// lines: the two noises' squared difference `s`, expanded and squared
/// to `(2s − 1)²`, then `x(2 − x)`; the glow [`plasma_glow`]). Halo 2's
/// stem adds `mask.a × fade × (colour 0|1 × s + colour 2|3 × q)` (`h2
/// shader` on retail's `teleporter_plasma`: stage 6 `r0 = r1 × t1 + r0.a
/// × c0`, `t1 = s` from stage 2, `r1` between colours 0 and 1 at stage 3,
/// `q = r0.a` after stage 4: `s` expanded and squared three times, 1
/// except along thin dark lines; colours 2 and 3 are stage 5's, which
/// reads only alpha, and stage 6's, which reads only colour, so the same
/// colour with alpha 0 in both is right whichever is which). So the mask
/// is the picture with alpha `gain × max(r, g, b)` (its brightness;
/// [`Self::gain`] gives back the headroom the 8-bit colours lack),
/// colours 2 and 3 the band (`hue × (1 + glow) / gain`, Halo 1's lit
/// picture) and colours 0 and 1 the glow (`hue × glow / gain`, lighting
/// the dark lines by `s` as Halo 1's glow does). Exact at Halo 1's peak
/// in the hue's brightest channel, where the register combiners' clamp
/// to 1 meets Halo 1's; the other channels as the picture's average hue.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlasmaLight {
    /// [`hue`] of the picture.
    pub hue: [f32; 3],
    /// [`plasma_glow`], 0 without one.
    pub glow: [f32; 3],
}

impl PlasmaLight {
    /// The hue's brightest channel.
    fn top(&self) -> usize {
        (0..3).fold(0, |k, c| if self.hue[c] > self.hue[k] { c } else { k })
    }

    /// `1 + glow` in the hue's brightest channel: the mask's alpha gain.
    pub fn gain(&self) -> f32 {
        1.0 + self.glow[self.top()]
    }

    /// Colours 0 and 1, `hue × glow / gain`.
    pub fn glow_color(&self) -> [f32; 3] {
        let g = self.gain();
        std::array::from_fn(|c| self.hue[c] * self.glow[c] / g)
    }

    /// Colours 2 and 3, `hue × (1 + glow) / gain`: 1 in the brightest
    /// channel.
    pub fn band_color(&self) -> [f32; 3] {
        let g = self.gain();
        std::array::from_fn(|c| (self.hue[c] * (1.0 + self.glow[c]) / g).min(1.0))
    }
}

/// A Halo 1 plasma ([`is_plasma`]) on [`PLASMA_ALPHA`], as Halo 2's own
/// plasma shaders (its teleporters', the needler's) draw it: the first
/// noise map still on bitmap 0, the second scrolling on bitmap 1 (their
/// difference makes the moving bands), the picture on bitmap 2 with its
/// brightness as alpha ([`Extras::brightest_mask`]), the colours and gain
/// from `light` ([`PlasmaLight`]), and the fade from Halo 1's fade mode.
/// `plain` (the single-map rebuild) is the fallback. Lost: the offset or
/// energy mask (map 2), which only bends the bands; a glow an object's A
/// output animates (held at the bound [`plasma_glow`] picks); the first
/// noise's own scroll (the clouds tree animates one map).
pub fn plasma_shader(
    name: &str,
    maps: &[ChainMap],
    fade: u16,
    light: PlasmaLight,
    plain: H1Shader,
) -> H1Shader {
    let glow = rgb_color(light.glow_color());
    let band = rgb_color(light.band_color());
    H1Shader {
        name: name.to_owned(),
        group: plain.group.clone(),
        layout: &PLASMA_ALPHA,
        maps: vec![
            (Role::Base, maps[0].bound),
            (Role::Tile, maps[1].bound),
            (Role::Mask, maps[3].bound),
        ],
        extras: Extras {
            colors: vec![(0, glow), (1, glow), (2, band), (3, band)],
            scroll: Some(maps[1].scroll),
            tint: Some(Tint::faded([1.0; 3], fade)),
            brightest_mask: Some(light.gain()),
            ..Extras::default()
        },
        fallback: Some(Box::new(plain)),
    }
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
        scroll: scroll.iter().any(Option::is_some).then_some(scroll),
        ..Extras::default()
    };
    Some((maps, extras))
}

pub(crate) fn uses_detail_program(layout: &Layout, extras: &Extras) -> bool {
    layout.stem == TEX_BUMP_DETAIL_BLEND_DETAIL.stem
        && extras.detail_blend != [DetailBlend::default(); 2]
}

/// The maps of an `senv` (`d`, its data) for
/// [`TEX_BUMP_DETAIL_BLEND_DETAIL`], from `single`, its maps for
/// [`TEX_BUMP`] (base and bump): Halo 1's primary detail where the base
/// alpha is 0 ([`Role::Detail`]) and, for a blended type, its secondary
/// where it is 1 ([`Role::Secondary`]); a normal one (or one missing
/// either) shows the one it has everywhere. The micro detail is
/// [`Role::Micro`]. With the default modes and no detail map, the
/// simpler opaque layout is enough.
fn senv_blend(
    h1: &h1map::H1Map,
    d: &[u8],
    single: &[(Role, H1Bound)],
    detail_blend: [DetailBlend; 2],
) -> Option<Vec<(Role, H1Bound)>> {
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
    if primary.is_none()
        && secondary.is_none()
        && micro.is_none()
        && detail_blend == [DetailBlend::default(); 2]
    {
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
/// (`h1_assets::bitmaps::mask::red_into_alpha`).
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

/// `soso` change colour source (`+0x4c`, i16): 0 none, 1 to 4 the drawn
/// object's change colour A to D, which tints the diffuse where the
/// multipurpose map's blue says (on the Xbox disc; the cyborg's armour
/// plates are blue there, its undersuit not). V (every `soso` on the
/// disc, docs/halo1/objects.md "Change colours"): 1 on the scenery,
/// vehicles, Covenant and doors that have one, 2 on the marines' faces and
/// the Flood, 3 on the cyborg's armour, visor and first-person hands; no
/// object's model reads two of A and C.
pub const SOSO_CHANGE_COLOUR: usize = 0x4c;

/// The multipurpose channel holding the change-colour mask: blue.
pub const SOSO_CHANGE_COLOUR_CHANNEL: usize = 2;

/// The change colour source of a `soso` (its bytes `d`,
/// [`SOSO_CHANGE_COLOUR`]): 1 to 4 for A to D, `None` for none or a
/// value out of range.
pub fn soso_change_colour_source(d: &[u8]) -> Option<u16> {
    let v = u16::from_le_bytes(
        d.get(SOSO_CHANGE_COLOUR..SOSO_CHANGE_COLOUR + 2)?
            .try_into()
            .ok()?,
    );
    (1..=4).contains(&v).then_some(v)
}

/// The Halo 2 change colour (object `+0xac` element, shader channel) a
/// Halo 1 change colour source (1 to 4, A to D) goes to: A and C to 0, B
/// to 1, D to 2.
///
/// Halo 2's engine writes a player's colours into their unit's change
/// colours 0 to 3 (`0x0014bfc0`, through `0x000bacc0`, which skips a slot
/// the object has none of): 0 the armour's primary colour, or the team's
/// in a team game; 1 the secondary, or the player's own primary in a team
/// game (`0x0007f790`); 2 and 3 the emblem's colours. Halo 1 gives a
/// player's unit its colour as change colour C: the multiplayer cyborg
/// has no C of its own (two change colours, A and B, each driven by a
/// function) and its armour reads C, and the campaign cyborg's C is the
/// Chief's green. So C goes to 0, where Halo 2 puts the player's colour.
/// A, the object's own colour everywhere else (scenery, vehicles, the
/// Covenant), goes to 0 too: every Halo 1 object reads at most one of A
/// and C ([`crate::shad::SOSO_CHANGE_COLOUR`]), and the object's slot 0
/// takes the one its shaders read (`h1objects`). B (the marines' skin
/// tones beside a crewman's A) keeps a slot of its own.
pub fn halo2_change_colour_slot(source: u16) -> Option<usize> {
    match source {
        1 | 3 => Some(0),
        2 => Some(1),
        4 => Some(2),
        _ => None,
    }
}

/// A `soso`'s change colour: its Halo 2 slot
/// ([`halo2_change_colour_slot`]) and the multipurpose map whose blue
/// masks it. `None` without either, or for another group.
pub fn soso_change_colour(h1: &h1map::H1Map, tag: &h1map::Tag) -> Option<(usize, h1map::TagId)> {
    let slot = halo2_change_colour_slot(soso_change_colour_source(h1.tag_data(tag).ok()?)?)?;
    Some((slot, soso_multipurpose(h1, tag)?))
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
            ..Extras::default()
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

    #[test]
    fn rebuilt_emblem_binds_the_player_inputs_at_the_native_pass() {
        use tags::effects::function::Function;

        let stem = TagId(0xe001_0001);
        let atlas = TagId(0xe002_0002);
        let b = emblem(stem, atlas).unwrap();
        let r = defs::from_built(shad_def(), &b).unwrap();
        // The template is a reference fixup to the stem, its group word
        // written as `stem`. A builder tree reads a reference fixup back
        // with group 0 (`defs::from_built`): the map builder writes the
        // target tag's own group at emit.
        assert!(b.fixups.iter().any(|f| {
            f.offset == 0 && f.kind == map::build::FixupKind::TagRef(stem)
        }));
        assert_eq!(b.bytes[..4], FourCC(*b"stem").as_u32().to_le_bytes());
        assert_eq!(
            r.at("template"),
            Some(&Val::Ref {
                group: 0,
                target: Target::Id(stem),
            })
        );
        assert_eq!(
            r.at("postprocess[0].bitmaps[0].bitmap"),
            Some(&datum(Some(atlas)))
        );
        assert_eq!(
            r.at("postprocess[0].lods[0].layer_mask"),
            Some(&Val::Int(4))
        );
        assert_eq!(
            r.at("postprocess[0].implementations[0].bitmap_animations"),
            Some(&range16(3, 1))
        );
        assert_eq!(
            r.at("postprocess[0].animations[3].property_range"),
            Some(&Val::Int(1))
        );
        for (i, input, sample, expected) in [
            (0, "emblem_toggle_foreground", 1.0, 1.0),
            (1, "emblem_foreground_index", 17.0 / 64.0, 17.0),
            (11, "emblem_active", 0.0, 0.0),
        ] {
            let path = format!("postprocess[0].properties[{i}]");
            assert_eq!(r.at(&format!("{path}.input")), Some(&Val::text(input)));
            let Some(Val::Data(data)) = r.at(&format!("{path}.function")) else {
                panic!("missing emblem property function {input}");
            };
            let function = Function::parse(data).unwrap();
            let [lo, hi] = function.range;
            assert_eq!(lo + (hi - lo) * function.evaluate(sample, 0.0), expected);
        }
    }

    #[test]
    fn textureless_layout_requires_absent_layers_and_white_glass_tint() {
        for (group, slots, layout) in [
            (
                b"senv",
                &[0x88, 0xb8, 0xcc, 0xfc, 0x128, 0x254, 0x324][..],
                &TEX_BUMP,
            ),
            (
                b"soso",
                &[0xa4, SOSO_MULTIPURPOSE, 0xdc, SOSO_CUBE][..],
                &TEX_BUMP,
            ),
            (
                b"sgla",
                &[0x64, 0xac, 0xc0, 0x158, 0x16c, 0x1a0, 0x1b4][..],
                &program::glass::CLEAR,
            ),
        ] {
            let mut b = vec![0; 0x340];
            for &i in slots {
                b[i..i + 4].copy_from_slice(&h1map::Group::new(b"bitm").0.to_le_bytes());
                b[i + 12..i + 16].fill(0xff);
            }
            for i in [0x54, 0x58, 0x5c] {
                b[i..i + 4].copy_from_slice(&1f32.to_le_bytes());
            }
            let group = h1map::Group::new(group);
            assert_eq!(textureless_layout(group, &b), Some(layout));
            b[slots[0] + 12] = 1;
            assert!(textureless_layout(group, &b).is_none());
            b[slots[0] + 12] = 0xff;
            if group == h1map::Group::new(b"sgla") {
                b[0x54..0x58].fill(0);
                assert!(textureless_layout(group, &b).is_none());
            }
            if group == h1map::Group::new(b"soso") {
                // The test fills 0x54..0x5c with 1.0 for glass; a soso
                // reads none of it. A reflection or change colour is not
                // map-less.
                b[SOSO_PERPENDICULAR_BRIGHTNESS..SOSO_PERPENDICULAR_BRIGHTNESS + 4]
                    .copy_from_slice(&1f32.to_le_bytes());
                assert!(textureless_layout(group, &b).is_none());
                b[SOSO_PERPENDICULAR_BRIGHTNESS..SOSO_PERPENDICULAR_BRIGHTNESS + 4].fill(0);
                b[SOSO_CHANGE_COLOUR] = 1;
                assert!(textureless_layout(group, &b).is_none());
            }
        }
    }

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

    /// Halo 1's change colour sources (`soso +0x4c`) and the Halo 2
    /// slots they go to: C (the player's colour in Halo 1) and A to 0,
    /// where Halo 2 writes the player's armour or team colour; B to 1, D
    /// to 2; none and out-of-range values to nothing.
    #[test]
    fn halo1_change_colours_go_to_halo2_slots() {
        let mut d = vec![0u8; 0x60];
        let mut source = |v: i16| {
            d[SOSO_CHANGE_COLOUR..SOSO_CHANGE_COLOUR + 2].copy_from_slice(&v.to_le_bytes());
            soso_change_colour_source(&d)
        };
        assert_eq!(source(0), None);
        assert_eq!(source(1), Some(1));
        assert_eq!(source(3), Some(3));
        assert_eq!(source(4), Some(4));
        assert_eq!(source(5), None);
        assert_eq!(source(-1), None);
        assert_eq!(soso_change_colour_source(&[0u8; 0x4d]), None, "short");
        let slots: Vec<_> = (0..=5).map(halo2_change_colour_slot).collect();
        assert_eq!(slots, [None, Some(0), Some(1), Some(0), Some(2), None]);
    }

    /// The change-colour layout: the change-colour map in bitmap 4, the
    /// cube in 5; Halo 1's reflection tints (1 to 4 on `tex_bump_env`) in
    /// colours 4 to 7, their copies in vectors 6 to 9 (RGB for the tints,
    /// the alpha alone for the brightnesses); the highlight colours 0 and
    /// 1 black, so vectors 10 and 11 too; tiling as on `tex_bump_env`.
    #[test]
    fn the_change_colour_layout_puts_the_mask_and_the_reflection_where_the_stem_reads_them() {
        let lib = library();
        let b = |id: u32, s: f32| Bound {
            id: TagId(id),
            scale: [s, s],
            lod_offset: 3.0,
        };
        let maps: HashMap<Role, Bound> = [
            (Role::Base, b(0xe3000001, 1.0)),
            (Role::Detail, b(0xe3000002, 10.0)),
            (Role::ChangeColour, b(0xe3000004, 1.0)),
            (Role::Cube, b(0xe3000005, 1.0)),
        ]
        .into_iter()
        .collect();
        let env = Extras {
            colors: vec![
                (1, 0x00ff_8000),
                (2, 0x0000_80ff),
                (3, 0xcc00_0000),
                (4, 0x3300_0000),
            ],
            scroll: None,
            ..Extras::default()
        };
        let extras = four_change_color_extras(&env);
        assert_eq!(
            extras.colors,
            [
                (4, 0x00ff_8000),
                (5, 0x0000_80ff),
                (6, 0xcc00_0000),
                (7, 0x3300_0000)
            ]
        );
        let rec = lib
            .record_with(&TEX_BUMP_ENV_FOUR_CHANGE_COLOR, &maps, &extras)
            .unwrap();
        let bm = |k: usize| {
            rec.at(&format!("postprocess[0].bitmaps[{k}].bitmap"))
                .cloned()
        };
        assert_eq!(
            bm(2),
            Some(Val::Datum(Target::Id(TagId(0xe3000001)))),
            "base"
        );
        assert_eq!(
            bm(3),
            Some(Val::Datum(Target::Id(TagId(0xe3000002)))),
            "detail"
        );
        assert_eq!(
            bm(4),
            Some(Val::Datum(Target::Id(TagId(0xe3000004)))),
            "change colour"
        );
        assert_eq!(
            bm(5),
            Some(Val::Datum(Target::Id(TagId(0xe3000005)))),
            "cube"
        );
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
        assert_eq!(vec(0), [10.0, 10.0, 0.0, 0.0], "detail tiling");
        assert_eq!(vec(1), [1.0, 1.0, 0.0, 0.0], "base tiling");
        assert_eq!(vec(6), [1.0, 128.0 / 255.0, 0.0, 0.0], "facing tint");
        assert_eq!(vec(7), [0.0, 128.0 / 255.0, 1.0, 0.0], "grazing tint");
        assert_eq!(vec(8), [0.0, 0.0, 0.0, 0.8], "facing brightness");
        assert_eq!(vec(9), [0.0, 0.0, 0.0, 0.2], "grazing brightness");
        assert_eq!(vec(10), [0.0; 4]);
        assert_eq!(vec(11), [0.0; 4]);
        let colour = |k: usize| {
            rec.at(&format!("postprocess[0].colors[{k}].color"))
                .cloned()
        };
        assert_eq!(colour(0), Some(Val::Int(0)));
        assert_eq!(colour(4), Some(Val::Int(0x00ff_8000)));
        assert_eq!(colour(7), Some(Val::Int(0x3300_0000)));
        // Without a reflection: the black default cube, white tints.
        let mut plain = maps.clone();
        plain.remove(&Role::Cube);
        let rec = lib.record(&TEX_BUMP_ENV_FOUR_CHANGE_COLOR, &plain).unwrap();
        assert_eq!(
            rec.at("postprocess[0].bitmaps[5].bitmap"),
            Some(&Val::Datum(Target::Id(lib.fixed[&Fixed::DefaultCube].id)))
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

    fn chain_map(id: u32, function: u16, scroll: bool, mean: f32) -> ChainMap {
        let s = Slide {
            period: 6.0,
            from: 0.0,
            to: 1.0,
        };
        ChainMap {
            bound: h1_bound(id, 1.0),
            function,
            scroll: [scroll.then_some(s), None],
            mean: [mean, mean * 0.5, 0.0],
        }
    }

    /// Fade when parallel: opaque facing the eye, gone at a grazing angle;
    /// when perpendicular, the reverse; none (or an unknown mode) keeps
    /// both.
    #[test]
    fn the_fade_mode_sets_the_facing_and_grazing_alphas() {
        let f = |m| {
            let t = Tint::faded([1.0; 3], m);
            (t.perpendicular, t.parallel)
        };
        assert_eq!(f(0), (1.0, 1.0));
        assert_eq!(f(FADE_PERPENDICULAR), (0.0, 1.0));
        assert_eq!(f(FADE_PARALLEL), (1.0, 0.0));
        assert_eq!(f(7), (1.0, 1.0));
    }

    /// The teleporter cone's shape: two scrolling dust maps multiplied,
    /// then a static ramp. The first dust scrolls on the stem's animated
    /// slot, the ramp is the other map, the second dust folds into the
    /// tint as its mean colour.
    #[test]
    fn a_multiply_chain_draws_its_first_scrolling_map_and_first_still_one() {
        let maps = [
            chain_map(1, CHICAGO_MULTIPLY, true, 0.4),
            chain_map(2, CHICAGO_MULTIPLY, true, 0.5),
            chain_map(3, 0, false, 0.6),
        ];
        let c = multiply_chain(&maps).unwrap();
        assert_eq!((c.tile, c.mask, c.factor), (0, 2, 1.0));
        assert_eq!(c.folded(&maps, &[0, 2]), [0.5, 0.25, 0.0]);
        let s = c.shader("cone", "schi", &maps, FADE_PARALLEL);
        assert_eq!(s.layout, &TWO_ADD_CLOUDS);
        assert_eq!(
            s.maps,
            vec![
                (Role::Tile, h1_bound(1, 1.0)),
                (Role::Mask, h1_bound(3, 1.0))
            ]
        );
        let t = s.extras.tint.unwrap();
        assert_eq!(t.color, [0.25, 0.125, 0.0], "halved: the stem doubles");
        assert_eq!((t.perpendicular, t.parallel), (1.0, 0.0));
        assert_eq!(s.extras.scroll, Some(maps[0].scroll));
        // Without the stem: the ramp alone, both dusts folded in.
        let f = s.fallback.as_deref().unwrap();
        assert_eq!(f.layout, &ONE_ADD_ILLUM);
        assert_eq!(f.maps, vec![(Role::Base, h1_bound(3, 1.0))]);
        let t = f.extras.tint.unwrap();
        assert!((t.color[0] - 0.2).abs() < 1e-6 && (t.color[1] - 0.05).abs() < 1e-6);
        assert_eq!(f.extras.scroll, None);
    }

    /// Nothing scrolling: maps 0 and 1 draw. A double multiply doubles the
    /// fold (clamped to 1). Another function, or one map, is no chain.
    #[test]
    fn a_chain_needs_every_map_but_the_last_to_multiply() {
        let maps = [
            chain_map(1, CHICAGO_DOUBLE_MULTIPLY, false, 0.4),
            chain_map(2, CHICAGO_MULTIPLY, false, 0.5),
            chain_map(3, 4, false, 0.9),
        ];
        let c = multiply_chain(&maps).unwrap();
        assert_eq!((c.tile, c.mask, c.factor), (0, 1, 2.0));
        assert_eq!(
            c.folded(&maps, &[0, 1]),
            [1.0, 0.9, 0.0],
            "2 × 0.9, clamped"
        );
        let mut add = maps;
        add[1].function = 4;
        assert_eq!(multiply_chain(&add), None, "add is not a multiply");
        assert_eq!(multiply_chain(&maps[..1]), None);
        // Two scrolling maps: the second takes the still slot, unanimated.
        let both = [
            chain_map(1, CHICAGO_MULTIPLY, true, 0.4),
            chain_map(2, 0, true, 0.5),
        ];
        let c = multiply_chain(&both).unwrap();
        assert_eq!((c.tile, c.mask), (0, 1));
    }

    #[test]
    fn mean_rgb_averages_each_channel() {
        assert_eq!(mean_rgb(&[]), [0.0; 3]);
        let m = mean_rgb(&[255, 0, 51, 9, 0, 0, 51, 200]);
        assert!((m[0] - 0.5).abs() < 1e-6 && m[1] == 0.0 && (m[2] - 0.2).abs() < 1e-6);
    }

    /// On `two_add_clouds`: the tile on bitmap 0 (rows 0 and 1, the
    /// scrolled pair), the mask on bitmap 1 (rows 2 and 3), the tint in
    /// vectors 4 to 7, colour 0 opaque, and the clouds tree animating the
    /// tile. On `one_add_illum` the tint goes to vectors 2 to 5.
    #[test]
    fn the_tint_and_scroll_land_in_the_stems_vectors_and_tree() {
        let lib = library();
        let b = |id: u32, u: f32, v: f32| Bound {
            id: TagId(id),
            scale: [u, v],
            lod_offset: 1.0,
        };
        let maps: HashMap<Role, Bound> = [
            (Role::Tile, b(0xe300_0001, 1.0, 1.0)),
            (Role::Mask, b(0xe300_0002, 1.0, 1.05)),
        ]
        .into_iter()
        .collect();
        let extras = Extras {
            tint: Some(Tint {
                color: [0.25, 0.5, 0.75],
                perpendicular: 1.0,
                parallel: 0.0,
            }),
            scroll: Some([
                Some(Slide {
                    period: 6.0,
                    from: 0.0,
                    to: 1.0,
                }),
                None,
            ]),
            ..Extras::default()
        };
        let rec = lib.record_with(&TWO_ADD_CLOUDS, &maps, &extras).unwrap();
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
        let bm = |k: usize| {
            rec.at(&format!("postprocess[0].bitmaps[{k}].bitmap"))
                .cloned()
        };
        assert_eq!(bm(0), Some(Val::Datum(Target::Id(TagId(0xe300_0001)))));
        assert_eq!(bm(1), Some(Val::Datum(Target::Id(TagId(0xe300_0002)))));
        assert_eq!(vec(3), [0.0, 1.05, 0.0, 0.0], "the mask's v row");
        assert_eq!(vec(4), [0.25, 0.5, 0.75, 0.0], "perpendicular colour");
        assert_eq!(vec(5), [0.25, 0.5, 0.75, 0.0], "parallel colour");
        assert_eq!(vec(6), [0.0, 0.0, 0.0, 1.0], "perpendicular alpha");
        assert_eq!(vec(7), [0.0, 0.0, 0.0, 0.0], "parallel alpha");
        assert_eq!(
            rec.at("postprocess[0].colors[0].color"),
            Some(&Val::Int(i64::from(OPAQUE_ALPHA)))
        );
        let n = |p: &str| match rec.at(p) {
            Some(Val::Block(b)) => b.len(),
            _ => usize::MAX,
        };
        assert_eq!(n("postprocess[0].animations"), 3);
        assert_eq!(n("postprocess[0].property_refs"), 10);

        let one: HashMap<Role, Bound> = [(Role::Base, b(0xe300_0002, 1.0, 1.0))]
            .into_iter()
            .collect();
        let rec = lib.record_with(&ONE_ADD_ILLUM, &one, &extras).unwrap();
        assert_eq!(
            rec.at("postprocess[0].vectors[5].vector"),
            Some(&Val::floats(&[0.0, 0.0, 0.0, 0.0]))
        );
        assert_eq!(
            rec.at("postprocess[0].vectors[2].vector"),
            Some(&Val::floats(&[0.25, 0.5, 0.75, 0.0]))
        );
        // A layout without tint vectors ignores the tint.
        let plain = lib.record_with(&TEX_BUMP, &one, &extras).unwrap();
        assert_eq!(
            plain.at("postprocess[0].vectors[3].vector"),
            Some(&Val::floats(&[0.39, 0.0, 0.93, 0.0]))
        );
    }

    /// The teleporter shield's shape: seven stages, one noise bitmap twice
    /// scrolling two ways, then a still energy mask and a still picture.
    #[test]
    fn a_plasma_is_seven_stages_over_one_noise_twice_then_two_still_maps() {
        let noise = |scroll| chain_map(1, 0, scroll, 1.0);
        let maps = [
            noise(true),
            noise(true),
            chain_map(2, 0, false, 0.5),
            chain_map(3, 0, false, 0.8),
        ];
        assert!(is_plasma(&maps, SOTR_PLASMA_STAGES));
        assert!(!is_plasma(&maps, 4), "the stage count");
        assert!(!is_plasma(&maps[..3], SOTR_PLASMA_STAGES), "four maps");
        let mut other = maps;
        other[1] = chain_map(9, 0, true, 1.0);
        assert!(!is_plasma(&other, SOTR_PLASMA_STAGES), "one noise bitmap");
        let mut still = maps;
        still[1] = noise(false);
        assert!(!is_plasma(&still, SOTR_PLASMA_STAGES), "both noises scroll");
        let mut moving = maps;
        moving[3] = chain_map(3, 0, true, 0.8);
        assert!(
            !is_plasma(&moving, SOTR_PLASMA_STAGES),
            "the picture is still"
        );

        let plain = H1Shader {
            name: "shield".into(),
            group: "sotr".into(),
            layout: &ONE_ADD_ILLUM,
            maps: vec![(Role::Base, maps[0].bound)],
            extras: Extras::default(),
            fallback: None,
        };
        let light = PlasmaLight {
            hue: [1.0, 0.5, 0.0],
            glow: [0.25, 0.5, 1.0],
        };
        let s = plasma_shader("shield", &maps, FADE_PARALLEL, light, plain.clone());
        assert_eq!(s.layout, &PLASMA_ALPHA);
        assert_eq!(
            s.maps,
            vec![
                (Role::Base, h1_bound(1, 1.0)),
                (Role::Tile, h1_bound(1, 1.0)),
                (Role::Mask, h1_bound(3, 1.0))
            ]
        );
        // Gain 1 + the glow in the hue's brightest channel (red): 1.25.
        // Colours 0 and 1 hue × glow / gain (0.2, 0.2, 0); 2 and 3 hue ×
        // (1 + glow) / gain (1, 0.6, 0); alpha 0.
        assert_eq!(s.extras.brightest_mask, Some(1.25));
        assert_eq!(
            s.extras.colors,
            vec![
                (0, 0x0033_3300),
                (1, 0x0033_3300),
                (2, 0x00ff_9900),
                (3, 0x00ff_9900)
            ]
        );
        assert_eq!(s.extras.scroll, Some(maps[1].scroll));
        let t = s.extras.tint.unwrap();
        assert_eq!((t.perpendicular, t.parallel), (1.0, 0.0));
        assert_eq!(s.fallback.as_deref(), Some(&plain));
    }

    /// A hue is weighted by brightness and scaled to 1 in its brightest
    /// channel; black has none.
    #[test]
    fn a_hue_is_the_bright_texels_colour_scaled_to_one() {
        let h = hue(&[255, 0, 0, 255, 0, 0, 0, 0, 0, 128, 0, 255]).unwrap();
        // Sums (255, 128, 0): red is 1, green 128 / 255.
        assert_eq!(h[0], 1.0);
        assert!((h[1] - 128.0 / 255.0).abs() < 1e-6, "{h:?}");
        assert_eq!(h[2], 0.0);
        assert_eq!(hue(&[0, 0, 0, 255, 0, 0, 0, 0]), None);
    }

    /// Halo 1's lit picture `m × hue × (1 + glow)` at band 1 equals the
    /// stem's `min(1, gain × m) × band colour` in the hue's brightest
    /// channel, saturating together; at a dark line Halo 1's `m × hue ×
    /// glow` equals `gain × m × glow colour` (the line's `s` at 1).
    #[test]
    fn a_plasmas_light_matches_halo1_in_its_brightest_channel() {
        let light = PlasmaLight {
            hue: [0.03, 1.0, 0.16],
            glow: [0.0, 0.329, 0.424],
        };
        let g = light.gain();
        assert!((g - 1.329).abs() < 1e-6);
        let (band, glow) = (light.band_color(), light.glow_color());
        for m in [0.1f32, 0.3, 0.6, 0.796, 1.0] {
            let halo1 = (m * (1.0 + 0.329)).min(1.0);
            let ours = (g * m).min(1.0) * band[1];
            assert!((halo1 - ours).abs() < 1e-5, "{m}: {halo1} vs {ours}");
            if g * m <= 1.0 {
                for c in 0..3 {
                    let halo1 = m * light.hue[c] * light.glow[c];
                    assert!((halo1 - g * m * glow[c]).abs() < 1e-5, "{m} {c}");
                }
            }
        }
        assert!(band.iter().chain(&glow).all(|c| (0.0..=1.0).contains(c)));
    }

    /// On `plasma_alpha`: the noises on bitmaps 0 and 1, the mask on 2,
    /// each map's rows in its pair, the scroll on bitmap 1's pair (its
    /// scale in components 1 and 2), the alphas in vectors 7 (facing) and
    /// 6 (grazing), the colours over the layout's.
    #[test]
    fn a_plasma_scrolls_its_second_noise_and_fades_through_vectors_7_and_6() {
        let lib = library();
        let b = |id: u32, s: f32| Bound {
            id: TagId(id),
            scale: [s, s],
            lod_offset: 1.0,
        };
        let maps: HashMap<Role, Bound> = [
            (Role::Base, b(0xe300_0001, 8.0)),
            (Role::Tile, b(0xe300_0001, 16.0)),
            (Role::Mask, b(0xe300_0003, 1.0)),
        ]
        .into_iter()
        .collect();
        let extras = Extras {
            colors: vec![(0, 0x00cc_6600), (1, 0x00cc_6600)],
            scroll: Some([
                Some(Slide {
                    period: 10.0,
                    from: 1.0,
                    to: 0.0,
                }),
                None,
            ]),
            tint: Some(Tint::faded([1.0; 3], FADE_PARALLEL)),
            ..Extras::default()
        };
        let rec = lib.record_with(&PLASMA_ALPHA, &maps, &extras).unwrap();
        let at = |p: &str| rec.at(p).cloned();
        let id = |n: u32| Some(Val::Datum(Target::Id(TagId(n))));
        assert_eq!(at("postprocess[0].bitmaps[0].bitmap"), id(0xe300_0001));
        assert_eq!(at("postprocess[0].bitmaps[2].bitmap"), id(0xe300_0003));
        assert_eq!(
            at("postprocess[0].vectors[2].vector"),
            Some(Val::floats(&[16.0, 0.0, 0.0, 0.0]))
        );
        assert_eq!(
            at("postprocess[0].vectors[0].vector"),
            Some(Val::floats(&[8.0, 0.0, 0.0, 0.0]))
        );
        assert_eq!(
            at("postprocess[0].vectors[7].vector"),
            Some(Val::floats(&[0.0, 0.0, 0.0, 1.0])),
            "facing"
        );
        assert_eq!(
            at("postprocess[0].vectors[6].vector"),
            Some(Val::floats(&[0.0, 0.0, 0.0, 0.0])),
            "grazing"
        );
        assert_eq!(
            at("postprocess[0].colors[1].color"),
            Some(Val::Int(0x00cc_6600))
        );
        assert_eq!(at("postprocess[0].colors[2].color"), Some(Val::Int(0)));
        let n = |p: &str| match rec.at(p) {
            Some(Val::Block(b)) => b.len(),
            _ => usize::MAX,
        };
        assert_eq!(n("postprocess[0].animations"), 3);
        // Component 1 (the u scale) is the scrolled noise's 16, not 8.
        let scroll = TextureScroll {
            scale: [16.0, 16.0],
            offset: extras.scroll.unwrap(),
        };
        let blob = |k: usize| match rec.at(&format!("postprocess[0].properties[{k}].function")) {
            Some(Val::Data(d)) => d.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(blob(1), scroll.properties()[1].1);
    }

    #[test]
    fn a_missing_stem_or_default_refuses() {
        let lib = Library::new(&[], &[]);
        assert!(!lib.has(&TEX_BUMP));
        assert!(lib.record(&TEX_BUMP, &HashMap::new()).is_err());
    }

    fn numeric(limit: u8, images: usize, engine_number: bool) -> H1Numeric {
        H1Numeric {
            limit,
            bitmap: h1map::TagId(0xe000_0001),
            scale: [1.0, 1.0],
            images,
            engine_number,
        }
    }

    /// Halo 1's counter digit `p` is `(n / 10^p) mod 10`; with the limit
    /// at the loaded maximum `n` is the rounds loaded, Halo 2's ones (0)
    /// and tens (1). Anything else has no Halo 2 weapon function.
    #[test]
    fn a_counter_digit_is_halo2s_ammunition_digit_only_when_it_counts_the_rounds() {
        let ar = numeric(60, 10, false);
        assert_eq!(ar.ammunition_digit(0, 60), Some("primary_ammunition_ones"));
        assert_eq!(ar.ammunition_digit(1, 60), Some("primary_ammunition_tens"));
        assert_eq!(ar.ammunition_digit(2, 60), None, "no hundreds function");
        assert_eq!(ar.ammunition_digit(-1, 60), None);
        // A limit other than the loaded maximum scales the rounds.
        assert_eq!(ar.ammunition_digit(0, 32), None);
        // Eight images: the compass (D out, not the rounds).
        assert_eq!(numeric(8, 8, false).ammunition_digit(0, 8), None);
        // The engine's own number.
        assert_eq!(numeric(60, 10, true).ammunition_digit(0, 60), None);
    }

    /// A counter digit draws on `illum_detail`: the digits as bitmap 0 at
    /// their tiling, the plain detail, colour white, and retail's counter
    /// tree whose property 0 is the weapon's digit, an identity over the
    /// ten frames; the rest an identity transform, driven by time.
    #[test]
    fn a_counter_digit_draws_on_illum_detail_its_frame_from_the_weapons_digit() {
        use tags::effects::function::{Function, FunctionType};
        let lib = library();
        let n = numeric(60, 10, false);
        let s = counter_shader("numbers", &n, "primary_ammunition_tens");
        assert_eq!(s.layout, &ILLUM_DETAIL);
        assert!(lib.has(s.layout));
        let digits = Bound {
            id: TagId(0xe300_0001),
            scale: [1.0, 1.0],
            lod_offset: 1.0,
        };
        let maps: HashMap<Role, Bound> = [(Role::Base, digits)].into_iter().collect();
        let rec = lib.record_with(s.layout, &maps, &s.extras).unwrap();
        let at = |p: &str| rec.at(p).cloned();
        assert_eq!(
            at("postprocess[0].bitmaps[0].bitmap"),
            Some(Val::Datum(Target::Id(TagId(0xe300_0001))))
        );
        assert_eq!(
            at("postprocess[0].bitmaps[2].bitmap"),
            Some(Val::Datum(Target::Null))
        );
        assert_eq!(
            at("postprocess[0].colors[0].color"),
            Some(Val::Int(i64::from(WHITE)))
        );
        assert_eq!(
            at("postprocess[0].lods[0].layer_mask"),
            Some(Val::Int(1 << COUNTER_TREE.animated_layer))
        );
        let n = |p: &str| match rec.at(p) {
            Some(Val::Block(b)) => b.len(),
            _ => usize::MAX,
        };
        assert_eq!(n("postprocess[0].layers"), 25);
        assert_eq!(n("postprocess[0].properties"), 10);
        assert_eq!(n("postprocess[0].property_refs"), 10);
        for (k, b) in COUNTER_TREE.bindings.iter().enumerate() {
            assert_eq!(
                at(&format!("postprocess[0].animations[{k}].target")),
                Some(Val::Int(i64::from(b[0])))
            );
        }
        assert_eq!(
            at("postprocess[0].properties[0].input"),
            Some(Val::String(Str::Text("primary_ammunition_tens".into())))
        );
        let blob = |k: usize| match rec.at(&format!("postprocess[0].properties[{k}].function")) {
            Some(Val::Data(d)) => Function::parse(d).unwrap(),
            other => panic!("{other:?}"),
        };
        let f = blob(0);
        assert_eq!((f.kind, f.range), (FunctionType::Identity, [0.0, 10.0]));
        for k in 1..10 {
            assert_eq!(
                at(&format!("postprocess[0].properties[{k}].input")),
                Some(Val::String(Str::Null))
            );
            let f = blob(k);
            assert_eq!(f.kind, FunctionType::Constant, "property {k}");
            assert_eq!(f.range[0], if k <= 3 { 1.0 } else { 0.0 }, "property {k}");
        }
    }
}
