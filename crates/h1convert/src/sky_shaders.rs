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
//! **Halo 2's side.** Three templates the base's own sky draws with
//! (D: Coagulation's sky shaders):
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
//! The copy is the template's whole tree ([`Relocator::to_built`]), named
//! for the Halo 1 shader, with the bitmap parameters and vectors changed
//! (and their blocks' sharing keys cleared). Animation the template
//! carries stays.
//!
//! Lost: the masked star layers of the night and dusk domes (the picture
//! alone draws), the per-map colour functions, and `schi` animation.

use std::io::{Read, Seek};

use h1map::{Group, H1Map, TagRef};
use map::build::{Block, FixupKind};
use map::{FourCC, Map, TagId};
use tags::material::{ShaderLibrary, TextureSource};
use tags::schema::{Relocator, Schema};
use tags::shader::SHAD;

use crate::textures::{block_child, h1_maps_of};

/// Halo 1 sky shader fields (module docs).
pub const BLEND: usize = 0x2c;
pub const BLEND_ADD: u16 = 3;
pub const MAPS: usize = 0x54;
/// (element size, bitmap reference, u scale) of `schi` and `sotr` maps.
pub const SCHI_MAP: (usize, usize, usize) = (0xdc, 0x6c, 0x54);
pub const SOTR_MAP: (usize, usize, usize) = (0x64, 0x1c, 0x04);

pub const ALPHA_TEMPLATE: &str = "shaders\\shader_templates\\transparent\\sky_one_alpha_env";
pub const CLOUDS_TEMPLATE: &str = "shaders\\shader_templates\\transparent\\two_alpha_clouds";
pub const ADDITIVE_TEMPLATE: &str = "shaders\\shader_templates\\transparent\\one_add_illum_detail";
/// The plain white bitmap the additive copies draw on stage 2.
pub const WHITE: &str = "shaders\\default_bitmaps\\bitmaps\\color_white";

/// Postprocess (`shad +0x20`) blocks: bitmap parameters, vectors.
const PARAMS: u32 = 0x04;
const PARAM: usize = 0xc;
const VECTORS: u32 = 0x14;
const VECTOR: usize = 0x10;
/// `one_add_illum_detail`'s tint vector.
const TINT: usize = 4;

/// One map of a Halo 1 sky shader.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SkyMap {
    pub bitmap: h1map::TagId,
    /// u and v scale.
    pub scale: [f32; 2],
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
    let chicago = tag.group == Group::new(b"schi");
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
            let maps = bytes
                .chunks_exact(size)
                .filter_map(|e| {
                    let r = TagRef::read(e, bitmap)?;
                    (r.group == Group::new(b"bitm") && !r.id.is_none()).then_some(SkyMap {
                        bitmap: r.id,
                        scale: [f32_at(e, scale)?, f32_at(e, scale + 4)?],
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

/// A base shader to copy: its tree, and which bitmap parameter each
/// texture stage of its first pass samples.
#[derive(Debug, Clone)]
pub struct Template {
    pub shader: TagId,
    pub name: String,
    pub stages: [Option<usize>; 4],
    tree: Block,
}

impl Template {
    /// A template from `tree` (a `shad` tree) whose stage `s` samples
    /// bitmap parameter `stages[s]`.
    pub fn new(shader: TagId, name: String, tree: Block, stages: [Option<usize>; 4]) -> Self {
        Self {
            shader,
            name,
            stages,
            tree,
        }
    }
}

/// The base's sky templates, and the shader a material falls back to.
#[derive(Debug, Clone, Default)]
pub struct SkyTemplates {
    pub alpha: Option<Template>,
    pub clouds: Option<Template>,
    pub additive: Option<Template>,
    /// [`WHITE`] on the base.
    pub white: Option<TagId>,
    /// For a material with no copy: the base sky's own shader
    /// ([`crate::sky::base_sky_shader`]), never NONE (`crate::sky`).
    pub fallback: Option<TagId>,
}

/// Finds the templates on `base`: for each, the first `shad` drawn with
/// it, `scenarios\skies\` ones first, then by name, whose tree copies and
/// whose stages resolve.
pub fn sky_templates<R: Read + Seek>(base: &mut Map<R>) -> SkyTemplates {
    let mut out = SkyTemplates {
        fallback: crate::sky::base_sky_shader(base),
        white: base
            .tags()
            .iter()
            .find(|t| t.group == FourCC(*b"bitm") && t.name.as_deref() == Some(WHITE))
            .map(|t| t.id),
        ..SkyTemplates::default()
    };
    let mut candidates: Vec<(bool, String, TagId)> = base
        .tags()
        .iter()
        .filter(|t| t.group == SHAD && t.location.is_some())
        .filter_map(|t| {
            let n = t.name.clone()?;
            Some((!n.starts_with("scenarios\\skies\\"), n, t.id))
        })
        .collect();
    candidates.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let Ok(mut lib) = ShaderLibrary::read(base) else {
        return out;
    };
    let Ok(rel) = Relocator::new(Schema::builtin(), base) else {
        return out;
    };
    for (_, name, id) in candidates {
        if out.alpha.is_some() && out.clouds.is_some() && out.additive.is_some() {
            break;
        }
        let Ok((template, params)) = scenario_edit::scene::shader_bitmaps(base, id) else {
            continue;
        };
        let slot = match template.as_str() {
            ALPHA_TEMPLATE => &mut out.alpha,
            CLOUDS_TEMPLATE => &mut out.clouds,
            ADDITIVE_TEMPLATE => &mut out.additive,
            _ => continue,
        };
        if slot.is_some() {
            continue;
        }
        let Ok(material) = lib.material(id, None) else {
            continue;
        };
        let Some(imp) = material
            .layers
            .iter()
            .filter(|l| l.lod == 0)
            .flat_map(|l| &l.passes)
            .find_map(|p| p.implementations.first())
        else {
            continue;
        };
        let mut stages = [None; 4];
        for (s, stage) in stages.iter_mut().enumerate() {
            if let Some(TextureSource::Bitmap { bitmap, .. }) = imp.textures[s].map(|t| t.source)
                && bitmap != TagId::NULL
            {
                *stage = params.iter().position(|&p| p == bitmap);
            }
        }
        let Ok(tree) = rel.to_built(id) else {
            continue;
        };
        *slot = Some(Template::new(id, name, tree, stages));
    }
    out
}

/// The postprocess child block at `at` (bitmap parameters, vectors) of a
/// `shad` tree, with the sharing key cleared on it and the blocks above.
fn postprocess_mut(root: &mut Block, at: u32) -> Option<&mut Block> {
    root.share = None;
    let pp = block_child(root, 0x20)?;
    pp.share = None;
    let b = block_child(pp, at)?;
    b.share = None;
    Some(b)
}

/// Points bitmap parameter `p` at `id`, replacing any fixup there.
fn set_param(params: &mut Block, p: usize, id: TagId) -> Option<()> {
    let at = (p * PARAM) as u32;
    if p * PARAM + 4 > params.bytes.len() {
        return None;
    }
    params.fixups.retain(|f| f.offset != at);
    params.bytes[p * PARAM..p * PARAM + 4].copy_from_slice(&id.0.to_le_bytes());
    params.fixup(at, FixupKind::TagId(id));
    Some(())
}

/// Writes vector `k` (four f32s), when the block has it.
fn set_vector(vectors: &mut Block, k: usize, v: [f32; 4]) {
    let at = k * VECTOR;
    if let Some(dst) = vectors.bytes.get_mut(at..at + VECTOR) {
        for (i, x) in v.iter().enumerate() {
            dst[4 * i..4 * i + 4].copy_from_slice(&x.to_le_bytes());
        }
    }
}

/// Sets vector pair `p` (u row `2p`, v row `2p + 1`) to `scale`.
fn set_pair(vectors: &mut Block, p: usize, scale: [f32; 2]) {
    set_vector(vectors, 2 * p, [scale[0], 0.0, 0.0, 0.0]);
    set_vector(vectors, 2 * p + 1, [0.0, scale[1], 0.0, 0.0]);
}

impl SkyTemplates {
    /// The copy for `recipe`, each map of the shader given as its
    /// installed bitmap's datum and scale (`maps`, indexed as
    /// [`H1SkyShader::maps`]). `None` when the template, a stage or a
    /// map is missing.
    pub fn copy(&self, recipe: Recipe, maps: &[Option<(TagId, [f32; 2])>]) -> Option<Block> {
        let map = |k: usize| maps.get(k).copied().flatten();
        // (template, [(stage, datum, scale)], the tint to whiten)
        let (t, binds, tint) = match recipe {
            Recipe::Picture(k) => {
                let (id, s) = map(k)?;
                (self.alpha.as_ref()?, vec![(3, id, s)], false)
            }
            Recipe::Clouds { mask, tile } => {
                let (m, ms) = map(mask)?;
                let (t, ts) = map(tile)?;
                (self.clouds.as_ref()?, vec![(1, m, ms), (2, t, ts)], false)
            }
            Recipe::Additive(k) => {
                let (id, s) = map(k)?;
                let white = self.white?;
                (
                    self.additive.as_ref()?,
                    vec![(1, id, s), (2, white, [1.0, 1.0])],
                    true,
                )
            }
        };
        let mut tree = t.tree.clone();
        let mut pairs = Vec::new();
        {
            let params = postprocess_mut(&mut tree, PARAMS)?;
            for &(stage, id, scale) in &binds {
                let p = t.stages[stage]?;
                set_param(params, p, id)?;
                pairs.push((p, scale));
            }
        }
        if let Some(vectors) = postprocess_mut(&mut tree, VECTORS) {
            if matches!(recipe, Recipe::Picture(_)) {
                let n = vectors.bytes.len() / (2 * VECTOR);
                for p in 0..n {
                    set_pair(vectors, p, pairs[0].1);
                }
            } else {
                for &(p, scale) in &pairs {
                    set_pair(vectors, p, scale);
                }
            }
            if tint && let Some(w) = vectors.bytes.get(TINT * VECTOR + 12..TINT * VECTOR + 16) {
                let w = f32::from_le_bytes(w.try_into().expect("4"));
                set_vector(vectors, TINT, [1.0, 1.0, 1.0, w]);
            }
        }
        Some(tree)
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
                })
                .collect(),
        }
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

    /// A `shad` tree shaped as the templates are: root `+0x20` one
    /// postprocess element, its `+0x04` bitmap parameters and `+0x14`
    /// vectors.
    fn tree(params: &[u32], vectors: usize) -> Block {
        let mut p = Block::zeroed(params.len() * PARAM);
        for (k, id) in params.iter().enumerate() {
            p.bytes[k * PARAM..k * PARAM + 4].copy_from_slice(&id.to_le_bytes());
            p.fixup((k * PARAM) as u32, FixupKind::TagId(TagId(*id)));
        }
        p.share = Some(1);
        let mut v = Block::zeroed(vectors * VECTOR);
        for k in 0..vectors {
            set_vector(&mut v, k, [0.5; 4]);
        }
        let mut pp = Block::zeroed(0x7c);
        pp.block(PARAMS, params.len() as u32, p);
        pp.block(VECTORS, vectors as u32, v);
        let mut root = Block::zeroed(0x54);
        root.block(0x20, 1, pp);
        root
    }

    fn parts(root: &Block) -> (Vec<(u32, TagId)>, Vec<[f32; 4]>) {
        let pp = match &root.fixups[0].kind {
            FixupKind::Block { child, .. } => child,
            _ => panic!("postprocess"),
        };
        let child = |at: u32| {
            pp.fixups
                .iter()
                .find_map(|f| match &f.kind {
                    FixupKind::Block { child, .. } if f.offset == at => Some(child),
                    _ => None,
                })
                .expect("child")
        };
        let ids = child(PARAMS)
            .fixups
            .iter()
            .filter_map(|f| match f.kind {
                FixupKind::TagId(id) => Some((f.offset, id)),
                _ => None,
            })
            .collect();
        let v = child(VECTORS)
            .bytes
            .as_chunks::<VECTOR>()
            .0
            .iter()
            .map(|c| {
                std::array::from_fn(|i| f32::from_le_bytes(c[4 * i..4 * i + 4].try_into().unwrap()))
            })
            .collect();
        (ids, v)
    }

    fn templates() -> SkyTemplates {
        SkyTemplates {
            // Coagulation's layouts: the picture parameter 2 on stage 3.
            alpha: Some(Template::new(
                TagId(1),
                "alpha".into(),
                tree(&[0xa0, 0xa1, 0xa2, 0xa3], 6),
                [Some(0), Some(1), Some(3), Some(2)],
            )),
            // Tile parameter 0 on stage 2, mask parameter 1 on stage 1.
            clouds: Some(Template::new(
                TagId(2),
                "clouds".into(),
                tree(&[0xb0, 0xb1, 0xb2], 4),
                [None, Some(1), Some(0), None],
            )),
            additive: Some(Template::new(
                TagId(3),
                "additive".into(),
                tree(&[0xc0, 0xc1], 8),
                [None, Some(0), Some(1), None],
            )),
            white: Some(TagId(0xff)),
            fallback: Some(TagId(9)),
        }
    }

    #[test]
    fn a_picture_copy_draws_the_picture_on_stage_3_at_its_scale() {
        let t = templates();
        let tree = t
            .copy(
                Recipe::Picture(1),
                &[None, Some((TagId(0x77), [12.0, 6.0]))],
            )
            .unwrap();
        let (ids, v) = parts(&tree);
        assert_eq!(
            ids,
            [
                (0x0, TagId(0xa0)),
                (0xc, TagId(0xa1)),
                (0x24, TagId(0xa3)),
                (0x18, TagId(0x77)),
            ]
        );
        for p in 0..3 {
            assert_eq!(v[2 * p], [12.0, 0.0, 0.0, 0.0]);
            assert_eq!(v[2 * p + 1], [0.0, 6.0, 0.0, 0.0]);
        }
        // The template is untouched; the copy's changed blocks aren't shared.
        assert_eq!(parts(&t.alpha.as_ref().unwrap().tree).1[0], [0.5; 4]);
        assert_eq!(tree.share, None);
        // A missing map: no copy.
        assert!(t.copy(Recipe::Picture(0), &[None]).is_none());
    }

    #[test]
    fn a_cloud_copy_scales_each_parameter_by_its_own_pair() {
        let tree = templates()
            .copy(
                Recipe::Clouds { mask: 0, tile: 1 },
                &[
                    Some((TagId(0x10), [1.0, 1.0])),
                    Some((TagId(0x11), [6.0, 6.0])),
                ],
            )
            .unwrap();
        let (ids, v) = parts(&tree);
        assert!(ids.contains(&(0xc, TagId(0x10))), "mask on parameter 1");
        assert!(ids.contains(&(0x0, TagId(0x11))), "tile on parameter 0");
        assert_eq!(ids.len(), 3);
        assert_eq!(
            &v[..4],
            [
                [6.0, 0.0, 0.0, 0.0],
                [0.0, 6.0, 0.0, 0.0],
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0]
            ]
        );
    }

    #[test]
    fn an_additive_copy_draws_the_picture_over_white_untinted() {
        let tree = templates()
            .copy(Recipe::Additive(0), &[Some((TagId(0x20), [1.0, 1.0]))])
            .unwrap();
        let (ids, v) = parts(&tree);
        assert_eq!(ids, [(0x0, TagId(0x20)), (0xc, TagId(0xff))]);
        assert_eq!(v[TINT], [1.0, 1.0, 1.0, 0.5]);
        assert_eq!(v[5], [0.5; 4], "other vectors kept");
        let mut no_white = templates();
        no_white.white = None;
        assert!(
            no_white
                .copy(Recipe::Additive(0), &[Some((TagId(0x20), [1.0, 1.0]))])
                .is_none()
        );
    }
}
