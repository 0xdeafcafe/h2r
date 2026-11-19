//! Halo 1's sky shaders as copies of the base's Halo 2 sky shaders, drawing
//! Halo 1's own bitmaps at Halo 1's map scale.
//!
//! **Halo 1's side.** A sky model's shaders are `schi` (transparent
//! chicago), `sotr` (transparent generic) or `swat` (water). D (every sky
//! of the campaign and multiplayer maps):
//!
//! | Field | `schi` | `sotr` |
//! |---|---|---|
//! | framebuffer blend, u16 | `+0x2c` | `+0x2c` |
//! | maps block (count, address) | `+0x54` | `+0x54` |
//! | map element size | `0xdc` | `0x64` |
//! | map's bitmap reference | `+0x6c` | `+0x1c` |
//! | map's u and v scale, f32 | `+0x54`, `+0x58` | `+0x04`, `+0x08` |
//!
//! Blend 0 is alpha blending, 3 is additive: the additive ones are
//! glows over a backdrop (a galaxy, a starfield, a canyon silhouette
//! drawn as light). A `swat` sky shader (water seen far off) is its base
//! map (`+0x4c`, [`crate::textures::h1_maps_of`]) at scale 1.
//!
//! A shader with several maps layers them: a mask, stars, twinkle and a
//! picture. D: every multi-map sky shader has exactly one map at scale
//! 1×1 or none tiled less, and that one is the sky's picture (the dusk,
//! night and clear-day domes, the horizon land); the others tile detail
//! over it (stars at 8 to 18×, detail ground at 12 to 30×). The cloud
//! layers are the exception: a `schi` with two maps, a 1×1 mask first and
//! a tiled cloud texture second.
//!
//! **Halo 2's side.** Three sky stems (D: Coagulation's sky shaders), each
//! a [`crate::shad`] layout the shader is rebuilt with from the `shad`
//! definition. The sky stems are on most maps but not in `shared.map`;
//! without one, the shared transparent stem stands in
//! ([`SkyTemplates::layout`]).
//!
//! - `sky_one_alpha_env`: stage 3's bitmap is the picture (`rgb = t3.rgb ×
//!   t3.a` plus a stage-2 term the template's default bitmap leaves black;
//!   `a = t3.a`; blend source one, destination one minus source alpha).
//!   Halo 1's alpha-blended single-picture shaders.
//! - `two_alpha_clouds`: stage 1 a mask, stage 2 a tiled texture (`rgb =
//!   t1.rgb × t2.rgb`, `a = t1.a × t2.a`, alpha blended). Halo 1's cloud
//!   layers. A Halo 1 `A8` mask samples as white with its alpha, so the
//!   clouds keep the tile's colour and the mask's shape.
//! - `one_add_illum_detail`: stage 1 times stage 2 times colour 0, ×2,
//!   added (blend one, one). Halo 1's additive shaders: their picture on
//!   stage 1, `color_white` on stage 2 (colour 0 is half grey, so the ×2
//!   gives the picture back), the tint vector (`vector[4]`) white.
//!
//! **Scale.** A template's vectors (postprocess `+0x14`, `0x10` each) come
//! in pairs, a u row and a v row, `[su, 0, 0, 0]` and `[0, sv, 0, 0]`.
//! D: Coagulation's cloud cover tiles bitmap parameter 0 eight times with
//! pair 0 and leaves parameter 1 at pair 1's 1×; its sun volume tiles
//! parameter 1 three times with pair 1. So pair `p` is bitmap parameter
//! `p`'s, which the copies set to Halo 1's map scale. `sky_one_alpha_env`
//! has four bitmap parameters and three pairs; every pair is set to the
//! picture's scale (the other stages hold uniform default bitmaps).
//!
//! The shader is new, named for the Halo 1 shader; no Halo 2 shader's
//! bytes are copied.
//!
//! **Scrolling.** A map's 2D animation is three runs of (u16 source, u16
//! function, f32 period, f32 phase, f32 scale), u, v and rotation, at
//! `+0xa4` (`schi`) or `+0x2c` (`sotr`); D (every sky and transparent
//! shader on the disc): the function is mostly 6, a slide (a sawtooth),
//! periods from 1 to 250 s, negative on some (reversed), scale ±1 or 2.
//! A cloud layer's tile scrolls as Halo 1's slides it (source 0, game
//! time, function 6), through `two_alpha_clouds`' texture transform
//! ([`shad::CLOUDS_TREE`]): the offset runs from 0 to the scale over the
//! period, from the scale to 0 when the period is negative.
//!
//! **Alpha.** A `schi`'s alpha comes from the maps its alpha functions
//! reach ([`alpha_reach`]); a dome's picture, last after its star layers,
//! is not one of them, so it is drawn from an opaque copy
//! ([`H1SkyShader::opaque_picture`]).
//!
//! Lost: the masked star layers of the night and dusk domes (the picture
//! alone draws), the per-map colour functions, the phase, rotation, the
//! other animation functions (cosine, noise, spark: a handful), and
//! scrolling on the picture and additive layers.

use std::collections::HashMap;
use std::io::{Read, Seek};

use h1map::{Group, H1Map, TagRef};
use map::build::Block;
use map::{Map, TagId};

use crate::shad;
use crate::textures::h1_maps_of;

/// Halo 1 sky shader fields (module docs).
pub const BLEND: usize = 0x2c;
pub const BLEND_ADD: u16 = 3;
pub const MAPS: usize = 0x54;
/// (element size, bitmap reference, u scale) of `schi` and `sotr` maps.
pub const SCHI_MAP: (usize, usize, usize) = (0xdc, 0x6c, 0x54);
pub const SOTR_MAP: (usize, usize, usize) = (0x64, 0x1c, 0x04);
/// A map's u animation run (`schi`, `sotr`); v follows `0x10` later.
pub const SCHI_ANIMATION: usize = 0xa4;
pub const SOTR_ANIMATION: usize = 0x2c;
/// Halo 1's slide (sawtooth) animation function.
pub const SLIDE: u16 = 6;

pub const ALPHA_TEMPLATE: &str = shad::SKY_PICTURE.stem;
pub const CLOUDS_TEMPLATE: &str = shad::SKY_CLOUDS.stem;
pub const ADDITIVE_TEMPLATE: &str = shad::SKY_ADDITIVE.stem;
/// The plain white bitmap the additive copies draw on stage 2.
pub const WHITE: &str = "shaders\\default_bitmaps\\bitmaps\\color_white";

/// One map of a Halo 1 sky shader.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyMap {
    pub bitmap: h1map::TagId,
    /// u and v scale.
    pub scale: [f32; 2],
    /// Its LOD offset ([`shad::lod_offset`]).
    pub lod_offset: f32,
    /// Its u and v scrolling (module docs).
    pub scroll: [Option<shad::Slide>; 2],
    /// Whether its alpha reaches the shader's ([`alpha_reach`]); always
    /// for a `sotr` or `swat`.
    pub alpha: bool,
}

/// A `schi` map's colour and alpha functions (`u16` each, `+0x2c` and
/// `+0x2e`): how map `k + 1` combines with what maps `0..=k` made. The
/// names are ours, from the values (D, every `schi` sky shader on the
/// disc): 0 keeps the current value (the dome maps after its stars), 1
/// takes the next map's (horizon land's alpha, the land's DXT3 cut-out
/// after a detail map), 2 multiplies (stars by their mask, a cloud tile by
/// its `A8` mask, colour and alpha), 3 is a doubled multiply (detail
/// ground on horizon land) and 4 adds (the stars onto a dome).
pub const SCHI_FUNCTIONS: usize = 0x2c;
pub const FUNCTION_CURRENT: u16 = 0;
pub const FUNCTION_NEXT: u16 = 1;

/// Which of a `schi`'s maps (by element, `alpha_functions` in order) the
/// shader's alpha comes from: map 0's to start, and each map's alpha
/// function then keeps it ([`FUNCTION_CURRENT`]), replaces it with the
/// next map's ([`FUNCTION_NEXT`]) or combines the next map's in (any
/// other).
///
/// D: every clear, dusk, night and doom dome on the disc is a stack of
/// star layers, twinkle and stars added on (colour functions 2 and 4),
/// then the dome's picture last, every alpha function 0: the alpha is the
/// first star map's, DXT1 or Y8, opaque. Halo 1 draws those domes opaque
/// (alpha blended, framebuffer blend 0) whatever the picture's own alpha:
/// Blood Gulch's and b30's `sky clear blue` is A8R8G8B8 with alpha 0 over
/// most of it (a star mask, 0 to 252), and the sky is blue. The other
/// domes' pictures are X8R8G8B8.
pub fn alpha_reach(alpha_functions: &[u16]) -> Vec<bool> {
    let mut reach = vec![false; alpha_functions.len()];
    if let Some(first) = reach.first_mut() {
        *first = true;
    }
    for (k, &f) in alpha_functions.iter().enumerate() {
        let Some(next) = k.checked_add(1).filter(|&n| n < reach.len()) else {
            break;
        };
        match f {
            FUNCTION_CURRENT => {}
            FUNCTION_NEXT => {
                reach[..next].fill(false);
                reach[next] = true;
            }
            _ => reach[next] = true,
        }
    }
    reach
}

/// The slide of a map element's animation run at `at`: source 0 (game
/// time), function [`SLIDE`], a period and scale not 0.
pub(crate) fn slide(e: &[u8], at: usize) -> Option<shad::Slide> {
    let u16_at = |o: usize| Some(u16::from_le_bytes(e.get(o..o + 2)?.try_into().ok()?));
    let f32_at = |o: usize| Some(f32::from_le_bytes(e.get(o..o + 4)?.try_into().ok()?));
    if u16_at(at)? != 0 || u16_at(at + 2)? != SLIDE {
        return None;
    }
    let (period, scale) = (f32_at(at + 4)?, f32_at(at + 12)?);
    if !period.is_finite() || !scale.is_finite() || period == 0.0 || scale == 0.0 {
        return None;
    }
    let (from, to) = if period > 0.0 { (0.0, scale) } else { (scale, 0.0) };
    Some(shad::Slide {
        period: period.abs(),
        from,
        to,
    })
}

/// A Halo 1 sky shader, as the conversion needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct H1SkyShader {
    pub name: String,
    /// `schi`.
    pub chicago: bool,
    /// Framebuffer blend [`BLEND_ADD`].
    pub additive: bool,
    /// Its maps with a bitmap, in order.
    pub maps: Vec<SkyMap>,
}

/// Reads the Halo 1 shader `id` of `h1` (module docs). `None` for a
/// group this doesn't read or one with no bitmap.
pub fn read_h1(h1: &H1Map, id: h1map::TagId) -> Option<H1SkyShader> {
    let tag = h1.tag(id).ok()?;
    let d = h1.tag_data(tag).ok()?;
    let f32_at = |b: &[u8], at: usize| -> Option<f32> {
        Some(f32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
    };
    // `scex` (chicago extended) reads as `schi` (`editor_core::halo1_art`).
    let chicago = tag.group == Group::new(b"schi") || tag.group == Group::new(b"scex");
    let layout = if chicago {
        Some(SCHI_MAP)
    } else if tag.group == Group::new(b"sotr") {
        Some(SOTR_MAP)
    } else {
        None
    };
    let (additive, maps) = match layout {
        Some((size, bitmap, scale)) => {
            let blend = u16::from_le_bytes(d.get(BLEND..BLEND + 2)?.try_into().ok()?);
            let block = h1map::Block::read(d, MAPS)?;
            let bytes = h1.block(block, size).ok()?;
            let elements: Vec<&[u8]> = bytes.chunks_exact(size).collect();
            let reach = if chicago {
                let at = SCHI_FUNCTIONS + 2;
                alpha_reach(
                    &elements
                        .iter()
                        .map(|e| {
                            e.get(at..at + 2)
                                .map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
                        })
                        .collect::<Vec<_>>(),
                )
            } else {
                vec![true; elements.len()]
            };
            let maps = elements
                .iter()
                .zip(reach)
                .filter_map(|(&e, alpha)| {
                    let r = TagRef::read(e, bitmap)?;
                    let anim = if chicago { SCHI_ANIMATION } else { SOTR_ANIMATION };
                    (r.group == Group::new(b"bitm") && !r.id.is_none()).then_some(SkyMap {
                        bitmap: r.id,
                        scale: [f32_at(e, scale)?, f32_at(e, scale + 4)?],
                        lod_offset: shad::h1_lod_offset(h1, r.id),
                        scroll: [slide(e, anim), slide(e, anim + 0x10)],
                        alpha,
                    })
                })
                .collect();
            (blend == BLEND_ADD, maps)
        }
        None => {
            let base = h1_maps_of(h1, tag)?.base?;
            (
                false,
                vec![SkyMap {
                    bitmap: base,
                    scale: [1.0, 1.0],
                    lod_offset: shad::h1_lod_offset(h1, base),
                    scroll: [None, None],
                    alpha: true,
                }],
            )
        }
    };
    if maps.is_empty() {
        return None;
    }
    Some(H1SkyShader {
        name: tag.name.clone(),
        chicago,
        additive,
        maps,
    })
}

impl H1SkyShader {
    /// The map drawn as the picture with its alpha made opaque
    /// (`h1bitmaps::convert::convert_bitmap_opaque`): an alpha-blended
    /// shader's [`Recipe::Picture`] whose own alpha doesn't reach the
    /// shader's ([`alpha_reach`]). `sky_one_alpha_env` draws `t3.rgb ×
    /// t3.a` with alpha `t3.a`; with the picture's star mask as that alpha
    /// Blood Gulch's sky was black, the planet and ring drawn over nothing.
    pub fn opaque_picture(&self) -> Option<usize> {
        match recipe(self)? {
            Recipe::Picture(k) if !self.maps[k].alpha => Some(k),
            _ => None,
        }
    }
}

/// Which Halo 2 template a Halo 1 sky shader becomes, and with which of
/// its maps (indices into [`H1SkyShader::maps`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recipe {
    /// [`ALPHA_TEMPLATE`], the picture on stage 3.
    Picture(usize),
    /// [`CLOUDS_TEMPLATE`]: the mask on stage 1, the tile on stage 2.
    Clouds { mask: usize, tile: usize },
    /// [`ADDITIVE_TEMPLATE`], the picture on stage 1.
    Additive(usize),
}

impl Recipe {
    /// The scrolling the recipe draws: a cloud layer's tile's.
    pub fn scroll(self, s: &H1SkyShader) -> [Option<shad::Slide>; 2] {
        match self {
            Recipe::Clouds { tile, .. } => s.maps.get(tile).map_or([None, None], |m| m.scroll),
            _ => [None, None],
        }
    }
}

/// The recipe for `s` (module docs). `None` with no maps.
pub fn recipe(s: &H1SkyShader) -> Option<Recipe> {
    let one = |m: &SkyMap| m.scale == [1.0, 1.0];
    // The least tiled map, the first of equals.
    let picture = (0..s.maps.len()).min_by(|&a, &b| {
        let area = |k: usize| (s.maps[k].scale[0] * s.maps[k].scale[1]).abs();
        area(a).total_cmp(&area(b))
    })?;
    Some(if s.additive {
        Recipe::Additive(picture)
    } else if s.chicago && s.maps.len() == 2 && one(&s.maps[0]) && !one(&s.maps[1]) {
        Recipe::Clouds { mask: 0, tile: 1 }
    } else {
        Recipe::Picture(picture)
    })
}

/// The base's stems and default bitmaps the sky shaders are rebuilt with,
/// and the shader a material falls back to.
#[derive(Debug, Clone, Default)]
pub struct SkyTemplates {
    pub library: shad::Library,
    /// For a material with no shader: the base sky's own shader
    /// ([`crate::sky::base_sky_shader`]), never NONE (`crate::sky`).
    pub fallback: Option<TagId>,
}

/// The base's stems, default bitmaps and fallback sky shader.
pub fn sky_templates<R: Read + Seek>(base: &mut Map<R>) -> SkyTemplates {
    SkyTemplates {
        library: shad::Library::read(base),
        fallback: crate::sky::base_sky_shader(base),
    }
}

impl SkyTemplates {
    /// The layout `recipe` draws with: the sky stem ([`shad::SKY_PICTURE`],
    /// [`shad::SKY_CLOUDS`], [`shad::SKY_ADDITIVE`]) when the base has
    /// it, else the shared transparent one ([`shad::ONE_ALPHA_ENV`],
    /// [`shad::ONE_ADD_ILLUM`]; a cloud layer then draws its tile alone).
    /// `None` when the base has neither.
    pub fn layout(&self, recipe: Recipe) -> Option<&'static shad::Layout> {
        let (sky, shared) = match recipe {
            Recipe::Picture(_) => (&shad::SKY_PICTURE, &shad::ONE_ALPHA_ENV),
            Recipe::Clouds { .. } => (&shad::SKY_CLOUDS, &shad::ONE_ALPHA_ENV),
            Recipe::Additive(_) => (&shad::SKY_ADDITIVE, &shad::ONE_ADD_ILLUM),
        };
        [sky, shared].into_iter().find(|l| self.library.has(l))
    }

    /// The maps (indices into [`H1SkyShader::maps`]) the shader `recipe`
    /// rebuilds draws, as [`Self::copy`] binds them: the picture; a cloud
    /// layer's mask and tile, or its tile alone on the shared layout. The
    /// others (a dome's star mask, twinkle and stars) have no stage, so
    /// their bitmaps aren't added: nothing would name them, and a fresh
    /// map leaves them out anyway. Empty when no layout fits.
    pub fn drawn(&self, recipe: Recipe) -> Vec<usize> {
        let Some(layout) = self.layout(recipe) else {
            return Vec::new();
        };
        match recipe {
            Recipe::Picture(k) | Recipe::Additive(k) => vec![k],
            Recipe::Clouds { mask, tile } if layout == &shad::SKY_CLOUDS => vec![mask, tile],
            Recipe::Clouds { tile, .. } => vec![tile],
        }
    }

    /// The rebuilt shader for `recipe`, each map of the shader given as
    /// its installed bitmap's datum, scale and LOD offset (`maps`, indexed
    /// as [`H1SkyShader::maps`]). `None` when no layout fits or a map is
    /// missing. `scroll` is the scrolling map's (a cloud layer's tile),
    /// drawn when the layout animates.
    pub fn copy(
        &self,
        recipe: Recipe,
        maps: &[Option<(TagId, [f32; 2], f32)>],
        scroll: [Option<shad::Slide>; 2],
    ) -> Option<Block> {
        let layout = self.layout(recipe)?;
        let bound = |k: usize| {
            maps.get(k)
                .copied()
                .flatten()
                .map(|(id, scale, lod_offset)| shad::Bound {
                    id,
                    scale,
                    lod_offset,
                })
        };
        let mut m = HashMap::new();
        match recipe {
            Recipe::Picture(k) | Recipe::Additive(k) => {
                m.insert(shad::Role::Base, bound(k)?);
            }
            Recipe::Clouds { mask, tile } => {
                if layout == &shad::SKY_CLOUDS {
                    m.insert(shad::Role::Mask, bound(mask)?);
                    m.insert(shad::Role::Tile, bound(tile)?);
                } else {
                    m.insert(shad::Role::Base, bound(tile)?);
                }
            }
        }
        let extras = shad::Extras {
            scroll: Some(scroll),
            ..shad::Extras::default()
        };
        self.library.build_with(layout, &m, &extras).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shader(chicago: bool, additive: bool, scales: &[[f32; 2]]) -> H1SkyShader {
        H1SkyShader {
            name: "sky\\test".into(),
            chicago,
            additive,
            maps: scales
                .iter()
                .enumerate()
                .map(|(k, &scale)| SkyMap {
                    bitmap: h1map::TagId(0xe000_0000 + k as u32),
                    scale,
                    lod_offset: 0.0,
                    scroll: [None, None],
                    alpha: true,
                })
                .collect(),
        }
    }

    /// Only the maps a rebuilt sky shader draws get their bitmaps added
    /// (`Sky::install`): Blood Gulch's clear afternoon added its dome's
    /// `detail star twinkle`, which no stage names, so the pinned map held
    /// a bitmap nothing reached and the fresh map left it out.
    #[test]
    fn a_sky_shader_draws_only_its_recipe_s_maps() {
        let stem = |l: &shad::Layout, k: u32| (l.stem, TagId(0xe000_0100 + k));
        // A layout counts only with its stem and its default bitmaps
        // (`Library::has`).
        let fixed: Vec<(shad::Fixed, shad::Bound)> = shad::Fixed::ALL
            .iter()
            .enumerate()
            .map(|(k, f)| {
                let id = TagId(0xe000_0200 + k as u32);
                let scale = [1.0, 1.0];
                (*f, shad::Bound { id, scale, lod_offset: 0.0 })
            })
            .collect();
        let sky = SkyTemplates {
            library: shad::Library::new(
                &[
                    stem(&shad::SKY_PICTURE, 0),
                    stem(&shad::SKY_CLOUDS, 1),
                    stem(&shad::SKY_ADDITIVE, 2),
                ],
                &fixed,
            ),
            fallback: None,
        };
        // A dusk dome: star mask, twinkle, stars, then the picture.
        let dusk = shader(
            true,
            false,
            &[[3.0, 3.0], [8.0, 8.0], [18.0, 18.0], [1.0, 1.0]],
        );
        assert_eq!(sky.drawn(recipe(&dusk).unwrap()), [3]);
        let clouds = shader(true, false, &[[1.0, 1.0], [4.0, 4.0]]);
        assert_eq!(sky.drawn(recipe(&clouds).unwrap()), [0, 1]);
        let glow = shader(false, true, &[[6.0, 6.0], [1.0, 1.0]]);
        assert_eq!(sky.drawn(recipe(&glow).unwrap()), [1]);
        // Without the sky stems a cloud layer draws its tile alone.
        let shared = SkyTemplates {
            library: shad::Library::new(&[stem(&shad::ONE_ALPHA_ENV, 3)], &fixed),
            fallback: None,
        };
        assert_eq!(shared.drawn(recipe(&clouds).unwrap()), [1]);
        // No layout: nothing drawn, nothing added.
        assert!(SkyTemplates::default().drawn(Recipe::Picture(0)).is_empty());
    }

    #[test]
    fn the_alpha_follows_the_alpha_functions() {
        // Blood Gulch's clear blue dome: twinkle, stars, the picture, every
        // alpha function "current": the first map's alpha only.
        assert_eq!(alpha_reach(&[0, 0, 0]), [true, false, false]);
        // A cloud layer: the mask's alpha times the tile's.
        assert_eq!(alpha_reach(&[2, 0]), [true, true]);
        // Horizon land: the land's alpha replaces the detail's.
        assert_eq!(alpha_reach(&[1, 0]), [false, true]);
        // Damnation's horizon: a doubled multiply keeps both.
        assert_eq!(alpha_reach(&[3, 0]), [true, true]);
        // Replaced, then combined.
        assert_eq!(alpha_reach(&[1, 4, 0]), [false, true, true]);
        assert_eq!(alpha_reach(&[0]), [true]);
        assert!(alpha_reach(&[]).is_empty());
    }

    #[test]
    fn a_dome_whose_picture_alpha_is_not_drawn_is_opaque() {
        let mut dome = shader(true, false, &[[12.0, 12.0], [8.0, 8.0], [1.0, 1.0]]);
        for (m, a) in dome.maps.iter_mut().zip(alpha_reach(&[0, 0, 0])) {
            m.alpha = a;
        }
        assert_eq!(recipe(&dome), Some(Recipe::Picture(2)));
        assert_eq!(dome.opaque_picture(), Some(2));
        // A picture whose alpha is drawn keeps it: a planet (`sotr`), the
        // horizon land's cut-out.
        let planet = shader(false, false, &[[1.0, 1.0]]);
        assert_eq!(planet.opaque_picture(), None);
        let mut land = shader(true, false, &[[30.0, 18.0], [1.0, 1.0]]);
        for (m, a) in land.maps.iter_mut().zip(alpha_reach(&[1, 0])) {
            m.alpha = a;
        }
        assert_eq!(land.opaque_picture(), None);
        // Clouds and additive layers are not pictures.
        let mut clouds = shader(true, false, &[[1.0, 1.0], [6.0, 6.0]]);
        clouds.maps[1].alpha = false;
        assert_eq!(clouds.opaque_picture(), None);
        let mut glow = shader(true, true, &[[3.0, 3.0], [1.0, 1.0]]);
        glow.maps[1].alpha = false;
        assert_eq!(glow.opaque_picture(), None);
    }

    #[test]
    fn the_picture_is_the_least_tiled_map() {
        // A dusk dome: star mask, twinkle, stars, then the picture.
        let dusk = shader(
            true,
            false,
            &[[3.0, 3.0], [8.0, 8.0], [18.0, 18.0], [1.0, 1.0]],
        );
        assert_eq!(recipe(&dusk), Some(Recipe::Picture(3)));
        // Horizon land: detail ground first.
        let land = shader(true, false, &[[30.0, 18.0], [1.0, 1.0]]);
        assert_eq!(recipe(&land), Some(Recipe::Picture(1)));
        // A generic shader with a tiled detail second stays a picture.
        let desert = shader(false, false, &[[1.0, 1.0], [12.0, 12.0]]);
        assert_eq!(recipe(&desert), Some(Recipe::Picture(0)));
        // One map tiled 80 times.
        let land = shader(true, false, &[[80.0, 80.0]]);
        assert_eq!(recipe(&land), Some(Recipe::Picture(0)));
        assert_eq!(recipe(&shader(true, false, &[])), None);
    }

    #[test]
    fn clouds_are_a_mask_then_a_tile() {
        let clouds = shader(true, false, &[[1.0, 1.0], [6.0, 6.0]]);
        assert_eq!(recipe(&clouds), Some(Recipe::Clouds { mask: 0, tile: 1 }));
        // Additive wins.
        let glow = shader(true, true, &[[3.0, 3.0], [16.0, 10.0], [1.0, 1.0]]);
        assert_eq!(recipe(&glow), Some(Recipe::Additive(2)));
    }

    fn library(stems: &[&'static shad::Layout]) -> SkyTemplates {
        let stems: Vec<(&'static str, TagId)> = stems
            .iter()
            .enumerate()
            .map(|(k, l)| (l.stem, TagId(0xe100_0000 + k as u32)))
            .collect();
        let fixed: Vec<(shad::Fixed, shad::Bound)> = shad::Fixed::ALL
            .iter()
            .enumerate()
            .map(|(k, &f)| {
                (
                    f,
                    shad::Bound {
                        id: TagId(0xe200_0000 + k as u32),
                        scale: [1.0, 1.0],
                        lod_offset: 1.0,
                    },
                )
            })
            .collect();
        SkyTemplates {
            library: shad::Library::new(&stems, &fixed),
            fallback: Some(TagId(9)),
        }
    }

    fn read(tree: &Block) -> tags::defs::Record {
        tags::defs::from_built(shad::shad_def(), tree).unwrap()
    }

    fn floats(r: &tags::defs::Record, path: &str) -> Vec<f32> {
        match r.at(path) {
            Some(tags::defs::Val::List(v)) => v
                .iter()
                .map(|x| match x {
                    tags::defs::Val::Float(f) => *f,
                    _ => f32::NAN,
                })
                .collect(),
            other => panic!("{path}: {other:?}"),
        }
    }

    fn bitmap(r: &tags::defs::Record, k: usize) -> Option<tags::defs::Val> {
        r.at(&format!("postprocess[0].bitmaps[{k}].bitmap")).cloned()
    }

    fn id(v: u32) -> Option<tags::defs::Val> {
        Some(tags::defs::Val::Datum(tags::defs::Target::Id(TagId(v))))
    }

    #[test]
    fn a_picture_draws_on_the_sky_stem_at_its_scale() {
        let t = library(&[&shad::SKY_PICTURE, &shad::ONE_ALPHA_ENV]);
        let tree = t
            .copy(
                Recipe::Picture(1),
                &[None, Some((TagId(0x77), [12.0, 6.0], 2.0))],
                [None, None],
            )
            .unwrap();
        let r = read(&tree);
        assert_eq!(bitmap(&r, 2), id(0x77), "the picture on stage 3");
        for p in 0..3 {
            assert_eq!(
                floats(&r, &format!("postprocess[0].vectors[{}].vector", 2 * p)),
                [12.0, 0.0, 0.0, 0.0]
            );
            assert_eq!(
                floats(&r, &format!("postprocess[0].vectors[{}].vector", 2 * p + 1)),
                [0.0, 6.0, 0.0, 0.0]
            );
        }
        // A missing map: no shader.
        assert!(t.copy(Recipe::Picture(0), &[None], [None, None]).is_none());
        // Without the sky stem, the shared transparent one.
        let shared = library(&[&shad::ONE_ALPHA_ENV]);
        assert_eq!(shared.layout(Recipe::Picture(0)), Some(&shad::ONE_ALPHA_ENV));
        assert!(library(&[]).layout(Recipe::Picture(0)).is_none());
    }

    #[test]
    fn a_cloud_layer_scales_its_mask_and_tile_apart() {
        let tree = library(&[&shad::SKY_CLOUDS])
            .copy(
                Recipe::Clouds { mask: 0, tile: 1 },
                &[
                    Some((TagId(0x10), [1.0, 1.0], 1.0)),
                    Some((TagId(0x11), [6.0, 6.0], 1.0)),
                ],
                [
                    Some(shad::Slide {
                        period: 120.0,
                        from: 0.0,
                        to: 1.0,
                    }),
                    None,
                ],
            )
            .unwrap();
        let r = read(&tree);
        // The tile scrolls in u: ten transform components on the clouds
        // tree, u offset a 120 s ramp from 0 to 1, v offset still.
        let n = |p: &str| match r.at(p) {
            Some(tags::defs::Val::Block(v)) => v.len(),
            _ => 0,
        };
        assert_eq!(n("postprocess[0].properties"), 10);
        assert_eq!(n("postprocess[0].animations"), 3);
        assert_eq!(n("postprocess[0].layers"), 25);
        let prop = |k: usize| {
            let Some(tags::defs::Val::Data(b)) = r.at(&format!("postprocess[0].properties[{k}].function")) else {
                panic!("property {k}")
            };
            let Some(tags::defs::Val::Float(period)) = r.at(&format!("postprocess[0].properties[{k}].input_b")) else {
                panic!("period {k}")
            };
            (*period, tags::effects::function::Function::parse(b).unwrap())
        };
        let value = |k: usize, t: f32| {
            let (period, f) = prop(k);
            let x = (t / period).rem_euclid(1.0);
            let v = f.evaluate(x, 0.0).clamp(0.0, 1.0);
            (f.range[1] - f.range[0]) * v + f.range[0]
        };
        assert!((value(4, 30.0) - 0.25).abs() < 1e-3, "u offset a quarter in");
        assert!((value(4, 90.0) - 0.75).abs() < 1e-3);
        assert_eq!(value(5, 30.0), 0.0, "v still");
        assert_eq!(value(1, 0.0), 6.0, "u scale");
        assert_eq!(value(2, 0.0), 6.0, "v scale");
        assert_eq!(value(0, 0.0), 1.0);
        assert_eq!(value(9, 0.0), 0.0);
        assert_eq!(bitmap(&r, 0), id(0x11), "tile on parameter 0");
        assert_eq!(bitmap(&r, 1), id(0x10), "mask on parameter 1");
        let v = |k: usize| floats(&r, &format!("postprocess[0].vectors[{k}].vector"));
        assert_eq!(v(0), [6.0, 0.0, 0.0, 0.0]);
        assert_eq!(v(1), [0.0, 6.0, 0.0, 0.0]);
        assert_eq!(v(2), [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(v(3), [0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn an_additive_layer_draws_the_picture_over_white_untinted() {
        let t = library(&[&shad::SKY_ADDITIVE]);
        let tree = t
            .copy(Recipe::Additive(0), &[Some((TagId(0x20), [1.0, 1.0], 1.0))], [None, None])
            .unwrap();
        let r = read(&tree);
        assert_eq!(bitmap(&r, 0), id(0x20));
        let white = t.library.build(&shad::SKY_ADDITIVE, &HashMap::new()).unwrap();
        assert_eq!(bitmap(&r, 1), bitmap(&read(&white), 1), "white on stage 2");
        assert_eq!(
            floats(&r, "postprocess[0].vectors[4].vector"),
            [1.0, 1.0, 1.0, 0.0]
        );
    }
}
