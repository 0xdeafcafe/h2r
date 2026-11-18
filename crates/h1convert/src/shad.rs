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
    /// detail, shown where the base map's alpha is 1.
    Detail,
    /// Halo 1's secondary detail map, shown where the base map's alpha is
    /// 0 (`senv` type blended).
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
}

impl Fixed {
    pub const ALL: [Fixed; 7] = [
        Fixed::DefaultVector,
        Fixed::DefaultDetail,
        Fixed::ColorWhite,
        Fixed::DefaultAdditive,
        Fixed::DefaultMultiplicative,
        Fixed::DefaultCube,
        Fixed::CornerFade,
    ];

    /// The bitmap's tag path on every Halo 2 map (V: all 30 disc maps).
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
};

/// Opaque bumped surfaces with Halo 1's detail blend: stage 0 the base
/// (bitmap 1, `c[18]` vector 3), stage 1 the detail shown where the base
/// alpha is 0 (bitmap 2, `c[19]` vector 0), stage 2 the one shown where
/// it is 1 (bitmap 3, `c[20].xy` vector 1), stage 3 a micro detail
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
        Bm::Role(Role::Secondary, Fixed::DefaultDetail),
        Bm::Role(Role::Detail, Fixed::DefaultDetail),
        Bm::Role(Role::Micro, Fixed::DefaultDetail),
        Bm::Fixed(Fixed::CornerFade),
        Bm::Fixed(Fixed::DefaultDetail),
    ],
    colors: &[WHITE, 0, 0, 0],
    vectors: &[
        Vector::Tile(Role::Secondary),
        Vector::Tile(Role::Detail),
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
};

/// Alpha blended: stage 0 a reflection cube (black: none), stage 1 a
/// multiplier (neutral), stages 2 and 3 the picture (bitmap 2) and the
/// additive default (bitmap 4); the picture's rows vectors 4 and 5.
pub const ONE_ALPHA_ENV: Layout = Layout {
    stem: "shaders\\shader_templates\\transparent\\one_alpha_env",
    word_14: 5,
    ints: [0, 0],
    bitmaps: &[
        Bm::Fixed(Fixed::DefaultCube),
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
};

/// Every layout.
pub const LAYOUTS: [&Layout; 10] = [
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
        build_record(layout, stem, &bitmaps, &colors, &vectors, picture)
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
/// `senv` type (`+0x2a`): 0 normal, 1 blended (the base map's alpha
/// blends the primary and secondary detail maps), 2 blended with the
/// base specular. V (395 `senv` on the disc): 0 or 1; Blood Gulch's
/// ground (primary sand, secondary grass) is 1.
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
/// another group or one with no map this reads.
///
/// - `senv` not alpha-tested, with a detail map:
///   [`TEX_BUMP_DETAIL_BLEND_DETAIL`] (`senv_blend`), falling back to the
///   single-detail [`TEX_BUMP`] on a base without its stem.
/// - `senv`: [`TEX_BUMP`], or [`TEX_BUMP_ALPHA_TEST`] with flags bit 0;
///   base, detail and bump maps with their scales
///   ([`crate::textures::h1_maps_of`], [`crate::textures::h1_scales_of`]);
///   a bump map is kept only when it is palettized bump (format 17).
/// - `soso`: [`TEX_BUMP`] with its base and detail maps.
/// - `swat`, `sgla`: [`ONE_ALPHA_ENV`] with the base map.
/// - `smet`: [`ILLUM`] with its map.
/// - `spla`: [`ONE_ADD_ILLUM`] with its map.
/// - `schi`, `scex`, `sotr`: their least tiled map
///   ([`crate::sky_shaders::recipe`]), [`ONE_ADD_ILLUM`] when the
///   framebuffer blend is additive, else [`ONE_ALPHA_ENV`].
pub fn read_h1(h1: &h1map::H1Map, tag: &h1map::Tag) -> Option<H1Shader> {
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
            let alpha_tested = group == "senv" && flags & 1 != 0;
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
                        scale: [s.detail; 2],
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
            Some(out)
        }
        "swat" | "sgla" => one(&ONE_ALPHA_ENV, crate::textures::h1_maps_of(h1, tag)?.base),
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
            let m = s.maps[k];
            Some(H1Shader {
                name: tag.name.clone(),
                group,
                layout: if s.additive {
                    &ONE_ADD_ILLUM
                } else {
                    &ONE_ALPHA_ENV
                },
                maps: vec![(
                    Role::Base,
                    H1Bound {
                        bitmap: m.bitmap,
                        scale: m.scale,
                    },
                )],
                extras: Extras::default(),
                fallback: None,
            })
        }
        _ => None,
    }
}

/// The maps of an `senv` (`d`, its data) for
/// [`TEX_BUMP_DETAIL_BLEND_DETAIL`], from `single`, its maps for
/// [`TEX_BUMP`] (base and bump): Halo 1's primary detail where the base
/// alpha is 1 ([`Role::Detail`]) and, for a blended type, its secondary
/// where it is 0 ([`Role::Secondary`]); a normal one (or one missing
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
    let r = h1map::TagRef::read(d, SENV_CUBE)?;
    if r.group != h1map::Group::new(b"bitm") || r.id.is_none() || h1.tag(r.id).is_err() {
        return None;
    }
    let f = |o: usize| {
        d.get(o..o + 4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .filter(|v| v.is_finite())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0)
    };
    let (perp, par) = (f(SENV_PERPENDICULAR_BRIGHTNESS), f(SENV_PARALLEL_BRIGHTNESS));
    if perp == 0.0 && par == 0.0 {
        return None;
    }
    let bright = f(SENV_SPECULAR);
    let byte = |v: f32| (v * 255.0).round() as u32;
    let rgb = |o: usize| (0..3).fold(0u32, |c, i| (c << 8) | byte(f(o + 4 * i) * bright));
    Some((
        H1Bound {
            bitmap: r.id,
            scale: [1.0, 1.0],
        },
        Extras {
            colors: vec![
                (1, rgb(SENV_PERPENDICULAR_COLOR)),
                (2, rgb(SENV_PARALLEL_COLOR)),
                (3, byte(perp) << 24),
                (4, byte(par) << 24),
            ],
        },
    ))
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
        assert_eq!(bm(2), id(0xe3000003), "secondary, stage 1 (base alpha 0)");
        assert_eq!(bm(3), id(0xe3000002), "primary, stage 2 (base alpha 1)");
        assert_eq!(bm(4), id(0xe3000004), "micro, stage 3");
        assert_eq!(vec(0), [60.0, 60.0, 0.0, 0.0]);
        assert_eq!(vec(1), [100.0, 100.0, 0.0, 0.0]);
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
