//! A shader's base map as the engine draws it: which of its bitmaps is the
//! surface's picture, and how often it tiles (docs/live/live-art.md, "Looks").
//!
//! Bitmap tag names only hint at a bitmap's role, and on the disc they lie:
//! a bump map named `..._bmp` reads as a picture by its name, and terrain
//! blend shaders list a bump map, a colour overlay and up to four detail
//! maps. The resolved material (`tags::material`, docs/shader-instances.md)
//! says what each pass samples on which texture stage. The base map is what
//! a pass samples on stage 0: a single-pass template (`tex_bump`,
//! `tex_bump_env`) draws base, detail, bump and lightmap on stages 0 to 3;
//! a blend template (`tex_bump_three_detail_blend`) accumulates its base
//! map and detail maps in a first pass and lights the result in a later
//! one whose stage 0 is the engine's accumulation texture.

use map::TagId;
use tags::material::{Material, Origin, ShaderLibrary, TextureSource};
use tags::{DecodedTag, PixelFormat, TagData};

use crate::live_geometry::MapReader;

/// A shader's base map and how often it tiles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BaseMap {
    pub bitmap: TagId,
    /// The image (sequence frame) the parameter selects.
    pub frame: u32,
    /// Texture coordinate scale (u, v) the pass gives it.
    pub scale: [f32; 2],
}

/// One pass of a resolved material, as [`base_candidates`] reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stage0 {
    /// The instance bitmap on texture stage 0, with its frame.
    pub bitmap: Option<(TagId, u32)>,
    /// The instance's x and y of [`STAGE0_SCALE`], when it writes both.
    pub scale: Option<[f32; 2]>,
}

/// The vertex constant whose x and y scale texture stage 0's coordinates.
/// On the disc every lit pass writes register 18 + k for stage k: a
/// `tex_bump` pass gives the detail map on stage 1 the shader's detail
/// scale (7 to 30 on the vehicles) and the base map on stage 0 one; a
/// blend template's accumulation pass gives its four stages 18.xy, 19.xy,
/// 20.xy and 20.zw (an Outskirts beach: 1, 150 × 75, 80 × 40, 111 × 55).
pub const STAGE0_SCALE: u8 = 18;

/// The passes of a material's first level of detail, in layer and pass
/// order (the order the engine draws them), each by its first
/// implementation.
pub fn passes(material: &Material) -> Vec<Stage0> {
    let mut out = Vec::new();
    for layer in material.layers.iter().filter(|l| l.lod == 0) {
        for pass in &layer.passes {
            let Some(imp) = pass.implementations.first() else {
                continue;
            };
            let bitmap = match imp.textures[0].map(|t| t.source) {
                Some(TextureSource::Bitmap { bitmap, image, .. }) => Some((bitmap, image)),
                _ => None,
            };
            let scale = imp
                .vertex_constants
                .iter()
                .find(|r| {
                    r.index == STAGE0_SCALE && r.origin == Origin::Instance && r.written & 3 == 3
                })
                .map(|r| [r.value[0], r.value[1]]);
            out.push(Stage0 { bitmap, scale });
        }
    }
    out
}

/// The candidates for the base map, best first: each pass's stage-0
/// instance bitmap in draw order, each bitmap once, with that pass's
/// stage-0 scale (1 when not written, or not a plausible tiling: zero,
/// not finite, or beyond 1/64 to 256).
pub fn base_candidates(passes: &[Stage0]) -> Vec<BaseMap> {
    let mut out: Vec<BaseMap> = Vec::new();
    for p in passes {
        let Some((bitmap, frame)) = p.bitmap else {
            continue;
        };
        if bitmap == TagId::NULL || out.iter().any(|b| b.bitmap == bitmap) {
            continue;
        }
        let plausible = |v: f32| v.is_finite() && (1.0 / 64.0..=256.0).contains(&v.abs());
        let scale = p
            .scale
            .filter(|s| s.iter().all(|&v| plausible(v)))
            .unwrap_or([1.0, 1.0]);
        out.push(BaseMap {
            bitmap,
            frame,
            scale,
        });
    }
    out
}

/// Whether a bitmap can be a surface's picture, by its tag name's folders
/// and its first image's format: engine defaults (`rasterizer`,
/// `default_bitmaps`) and bump maps (8-bit palettized normals) cannot.
pub fn can_be_base(name: &str, format: Option<PixelFormat>) -> bool {
    let lower = name.to_ascii_lowercase();
    let engine = lower
        .split('\\')
        .any(|c| c == "default_bitmaps" || c == "rasterizer");
    !engine && format.is_some_and(|f| f != PixelFormat::P8Bump)
}

/// A shader's base map: the first of [`base_candidates`] that
/// [`can_be_base`]. `None` when the shader does not resolve or draws no
/// such bitmap on stage 0; callers then go by bitmap names.
pub fn base_map(lib: &mut ShaderLibrary, map: &mut MapReader, shader: TagId) -> Option<BaseMap> {
    let material = lib.material(shader, None).ok()?;
    base_candidates(&passes(&material)).into_iter().find(|b| {
        let name = map
            .tag(b.bitmap)
            .ok()
            .and_then(|t| t.name.clone())
            .unwrap_or_default();
        let format = match DecodedTag::read(map, b.bitmap).map(|d| d.data) {
            Ok(TagData::Bitmap(bm)) => bm.images.first().map(|i| i.format),
            _ => None,
        };
        can_be_base(&name, format)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(bitmap: Option<u32>, scale: Option<[f32; 2]>) -> Stage0 {
        Stage0 {
            bitmap: bitmap.map(|b| (TagId(b), 0)),
            scale,
        }
    }

    #[test]
    fn a_single_pass_template_s_base_is_stage_0_at_its_scale() {
        // tex_bump: stages base, detail, bump, lightmap; then a bump-only
        // pass and passes on engine textures.
        let p = [
            pass(Some(12), Some([1.0, 1.0])),
            pass(Some(10), None),
            pass(None, None),
        ];
        let c = base_candidates(&p);
        assert_eq!(c[0].bitmap, TagId(12));
        assert_eq!(c[0].scale, [1.0, 1.0]);
        assert_eq!(c[1].bitmap, TagId(10), "the bump map comes after");
    }

    #[test]
    fn a_blend_template_s_base_is_its_accumulation_pass_stage_0() {
        // tex_bump_three_detail_blend: accumulate (overlay, details), then
        // light the accumulation (engine texture on stage 0).
        let p = [
            pass(Some(21), Some([3.0, 2.0])),
            pass(None, Some([1.0, 1.0])),
            pass(Some(20), None),
        ];
        let c = base_candidates(&p);
        assert_eq!(
            c[0],
            BaseMap {
                bitmap: TagId(21),
                frame: 0,
                scale: [3.0, 2.0]
            }
        );
    }

    #[test]
    fn implausible_scales_read_as_one_and_bitmaps_come_once() {
        for bad in [
            [0.0, 1.0],
            [f32::NAN, 1.0],
            [1.0, 1000.0],
            [f32::INFINITY, 2.0],
        ] {
            let c = base_candidates(&[pass(Some(5), Some(bad))]);
            assert_eq!(c[0].scale, [1.0, 1.0], "{bad:?}");
        }
        let c = base_candidates(&[
            pass(Some(5), Some([2.0, 2.0])),
            pass(Some(5), Some([4.0, 4.0])),
            pass(Some(0xffff_ffff), None),
        ]);
        assert_eq!(
            c.len(),
            1,
            "a null bitmap is no candidate; a repeat is not either"
        );
        assert_eq!(c[0].scale, [2.0, 2.0], "the first pass's scale");
        assert_eq!(
            base_candidates(&[pass(Some(5), Some([-2.0, 0.5]))])[0].scale,
            [-2.0, 0.5]
        );
    }

    #[test]
    fn engine_defaults_and_bump_maps_are_not_pictures() {
        let ok = Some(PixelFormat::Dxt1);
        assert!(can_be_base(
            r"scenarios\bitmaps\solo\city\beach_sandwet",
            ok
        ));
        assert!(!can_be_base(
            r"scenarios\bitmaps\multi\x\sand_bmp",
            Some(PixelFormat::P8Bump)
        ));
        assert!(!can_be_base(r"rasterizer\default_2d", ok));
        assert!(!can_be_base(
            r"shaders\default_bitmaps\bitmaps\gray_50_percent",
            ok
        ));
        assert!(!can_be_base(r"scenarios\bitmaps\x\unreadable", None));
    }
}
