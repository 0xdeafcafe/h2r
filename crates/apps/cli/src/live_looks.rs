//! The pictures on a level's surfaces in `map.glb`: base maps and plasma
//! maps for BSPs and placed models. Pictures are shared across surfaces
//! and kept within a byte budget (docs/live/live-art.md, "Textures").

use std::collections::HashMap;
use std::sync::Arc;

use assets::ResourceFiles;
use map::TagId;

use tags::material::ShaderLibrary;

use crate::live_basemap;
use crate::live_geometry::MapReader;
use crate::live_layers::Texture;
use crate::live_models::{self, BitmapRole, Image};
use crate::live_plasma::{self, PlasmaLook};

/// The largest side of a surface picture in `map.glb`: above any retail
/// bitmap (the discs' largest are 1024), so every picture keeps the disc's
/// full resolution.
pub const MAX_SURFACE_TEXTURE: u32 = 2048;
/// The smallest side the budget may shrink pictures to.
pub const MIN_SURFACE_TEXTURE: u32 = 64;
/// The most bytes of surface pictures one level's `map.glb` holds: past
/// it, every picture of the level is halved until they fit (or reach
/// [`MIN_SURFACE_TEXTURE`]).
/// At full resolution the largest retail level's pictures are about 6 MiB
/// as WebP.
pub const TEXTURE_BUDGET: usize = 32 << 20;
/// WebP quality of surface pictures (libwebp's 0 to 100).
pub const WEBP_QUALITY: f32 = 90.0;

/// How a surface's picture is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cutout {
    /// Solid: the picture's alpha (often a specular mask) is not used.
    Opaque,
    /// Cut out where the picture's alpha is under a half (grates, leaves).
    Mask,
    /// An unmodified channel mask or noise map for the plasma pass.
    Plasma,
}

/// One textured surface material: a shader with a picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    /// The shader's tag ID: one look per shader in a level.
    pub shader: u32,
    /// The shader's own name (last path component).
    pub name: String,
    pub cutout: Cutout,
    pub texture: Arc<Texture>,
    /// How often the picture tiles per stored texture coordinate (u, v):
    /// the shader's stage-0 scale ([`live_basemap::BaseMap`]).
    pub scale: [f32; 2],
    /// The transparent pass parameters and its two noise maps.
    pub plasma: Option<(PlasmaLook, [Arc<Texture>; 2])>,
}

/// A picture ready for the glb.
#[derive(Clone, Debug, PartialEq)]
pub struct Encoded {
    pub bytes: Vec<u8>,
    /// `image/webp`.
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
}

/// A level's looks, read once per shader and decoded once per bitmap.
#[derive(Default)]
pub struct Looks {
    pub looks: Vec<Look>,
    of: HashMap<u32, Option<u32>>,
    decoded: HashMap<(u32, Cutout), Option<Arc<Texture>>>,
    /// The map's shaders, read the first time a look is (`None` inside
    /// when they could not be: looks then go by bitmap names alone).
    library: Option<Option<ShaderLibrary>>,
    /// Solid shaders left without a picture, by template (or why the
    /// shader could not be read) and how many: drawn in their surface
    /// class's colour instead ([`fallback_note`]).
    pub fallbacks: std::collections::BTreeMap<String, usize>,
}

/// The manifest's `map/<stem>/shaders` note: which shader templates were
/// drawn in their class colour for want of a picture, most used first.
/// `None` when every solid shader got one.
pub fn fallback_note(fallbacks: &std::collections::BTreeMap<String, usize>) -> Option<String> {
    if fallbacks.is_empty() {
        return None;
    }
    let mut v: Vec<(&String, &usize)> = fallbacks.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    let list: Vec<String> = v.iter().map(|(t, n)| format!("{t} ({n})")).collect();
    Some(format!(
        "{} shaders drawn in their class colour (no picture): {}",
        fallbacks.values().sum::<usize>(),
        list.join(", ")
    ))
}

/// How a shader template draws its picture: cut out when its name says
/// `alpha_test`, else solid. `None` for templates that are not solid
/// surfaces (transparent, additive, overlays; those are glass, water or
/// layers and keep their own look). An overlay is a template named for it
/// (`opaque\overlay`, `overlay_...`): `tex_bump_detail_overlay` is a solid
/// surface whose detail map is overlaid (Coagulation's rocks).
pub fn template_cutout(template: &str) -> Option<Cutout> {
    let lower = template.to_ascii_lowercase();
    let stem = lower.rsplit('\\').next().unwrap_or(&lower);
    let overlay = stem.split('_').next() == Some("overlay");
    if lower.split('\\').any(|c| c == "transparent") || overlay {
        return None;
    }
    Some(if stem.contains("alpha_test") {
        Cutout::Mask
    } else {
        Cutout::Opaque
    })
}

/// The base map among a shader's bitmaps by the words of their tag names
/// ([`live_models::bitmap_role`]): the first diffuse one. Only for shaders
/// whose passes do not say ([`live_basemap::base_map`]).
pub fn pick_base(names: &[(u32, String)]) -> Option<u32> {
    names
        .iter()
        .find(|(_, n)| live_models::bitmap_role(n) == BitmapRole::Diffuse)
        .map(|(id, _)| *id)
}

/// A decoded picture as a surface draws it: at most [`MAX_SURFACE_TEXTURE`]
/// a side. Solid shaders use alpha for specularity or blending rather
/// than coverage. Plasma uses its stored channels.
pub fn surface_image(image: Image, cutout: Cutout) -> Image {
    let mut image = image.fit(MAX_SURFACE_TEXTURE);
    match cutout {
        Cutout::Opaque => {
            for px in image.rgba.as_chunks_mut::<4>().0.iter_mut() {
                px[3] = 255;
            }
        }
        Cutout::Mask => cut_at_a_quarter(&mut image),
        Cutout::Plasma => {}
    }
    image
}

/// A cut-out picture's alpha stretched so its strongest alpha is opaque,
/// then doubled: a reader cutting at a half (glTF's default) keeps what is
/// at least a quarter of the strongest. On the disc the leaves of trees
/// and bushes keep 0.5 % to 3 % of their pixels at a half but 10 % to 30 %
/// at a quarter, and one never reaches a third: cut at a half they vanish.
pub fn cut_at_a_quarter(image: &mut Image) {
    let max = image
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[3])
        .max()
        .unwrap_or(0);
    if max == 0 {
        return;
    }
    for px in image.rgba.as_chunks_mut::<4>().0.iter_mut() {
        px[3] = (u32::from(px[3]) * 510 / u32::from(max)).min(255) as u8;
    }
}

impl Looks {
    /// The look of a shader, read once, for a solid surface or a supported
    /// plasma pass with pictures that decode.
    pub fn look_of(
        &mut self,
        map: &mut MapReader,
        files: &mut ResourceFiles,
        shader: TagId,
    ) -> Option<u32> {
        if shader == TagId::NULL {
            return None;
        }
        if let Some(&k) = self.of.get(&shader.0) {
            return k;
        }
        let k = self.read(map, files, shader);
        self.of.insert(shader.0, k);
        k
    }

    fn read(
        &mut self,
        map: &mut MapReader,
        files: &mut ResourceFiles,
        shader: TagId,
    ) -> Option<u32> {
        let (template, bitmaps) = match live_models::shader_bitmaps(map, shader) {
            Ok(t) => t,
            Err(_) => {
                *self
                    .fallbacks
                    .entry("(unreadable shader)".into())
                    .or_default() += 1;
                return None;
            }
        };
        if live_plasma::is_template(&template) {
            return self.read_plasma(map, files, shader);
        }
        let cutout = template_cutout(&template)?;
        let look = self.read_solid(map, files, shader, &bitmaps, cutout);
        if look.is_none() {
            let own = template.rsplit('\\').next().unwrap_or(&template);
            *self.fallbacks.entry(own.to_string()).or_default() += 1;
        }
        look
    }

    fn read_solid(
        &mut self,
        map: &mut MapReader,
        files: &mut ResourceFiles,
        shader: TagId,
        bitmaps: &[TagId],
        cutout: Cutout,
    ) -> Option<u32> {
        let names: Vec<(u32, String)> = bitmaps
            .iter()
            .filter_map(|&id| Some((id.0, map.tag(id).ok()?.name.clone()?)))
            .collect();
        let library = self
            .library
            .get_or_insert_with(|| ShaderLibrary::read(map).ok())
            .as_mut();
        let (bitmap, scale) = match library.and_then(|l| live_basemap::base_map(l, map, shader)) {
            Some(b) => (b.bitmap.0, b.scale),
            None => (pick_base(&names)?, [1.0, 1.0]),
        };
        let texture = self.texture(map, files, TagId(bitmap), cutout)?;
        Some(self.add(Look {
            shader: shader.0,
            name: shader_name(map, shader),
            cutout,
            texture,
            scale,
            plasma: None,
        }))
    }

    fn read_plasma(
        &mut self,
        map: &mut MapReader,
        files: &mut ResourceFiles,
        shader: TagId,
    ) -> Option<u32> {
        let lib = self
            .library
            .get_or_insert_with(|| ShaderLibrary::read(map).ok())
            .as_mut()?;
        let (look, [a, b, mask]) = live_plasma::read(map, lib, shader)?;
        let cutout = Cutout::Plasma;
        let noise = [
            self.texture(map, files, a, cutout)?,
            self.texture(map, files, b, cutout)?,
        ];
        let texture = self.texture(map, files, mask, cutout)?;
        Some(self.add(Look {
            shader: shader.0,
            name: shader_name(map, shader),
            cutout,
            texture,
            scale: look.uv[2].scale,
            plasma: Some((look, noise)),
        }))
    }

    fn texture(
        &mut self,
        map: &mut MapReader,
        files: &mut ResourceFiles,
        bitmap: TagId,
        cutout: Cutout,
    ) -> Option<Arc<Texture>> {
        self.decoded
            .entry((bitmap.0, cutout))
            .or_insert_with(|| {
                let image = live_models::decode_bitmap(map, files, bitmap).ok()?;
                Some(Arc::new(Texture {
                    bitmap: bitmap.0,
                    image: surface_image(image, cutout),
                }))
            })
            .clone()
    }

    /// Adds a look (tests build them by hand).
    pub fn add(&mut self, look: Look) -> u32 {
        self.of.insert(look.shader, Some(self.looks.len() as u32));
        self.looks.push(look);
        (self.looks.len() - 1) as u32
    }

    /// Every distinct picture, encoded, keyed by `(bitmap, cutout)`: at the
    /// largest size whose total fits `budget`.
    pub fn encode(&self, budget: usize) -> HashMap<(u32, Cutout), Encoded> {
        let mut distinct: Vec<((u32, Cutout), &Image)> = Vec::new();
        for l in &self.looks {
            let noise = l.plasma.as_ref().into_iter().flat_map(|(_, textures)| textures);
            for t in std::iter::once(&l.texture).chain(noise) {
                let key = (t.bitmap, l.cutout);
                if !distinct.iter().any(|(k, _)| *k == key) {
                    distinct.push((key, &t.image));
                }
            }
        }
        // From the largest picture's side, so the first halving shrinks it.
        let mut cap = distinct
            .iter()
            .map(|(_, img)| img.width.max(img.height))
            .max()
            .unwrap_or(1)
            .min(MAX_SURFACE_TEXTURE);
        loop {
            let out: HashMap<(u32, Cutout), Encoded> = distinct
                .iter()
                .map(|(k, img)| (*k, encode_image(&(*img).clone().fit(cap))))
                .collect();
            let total: usize = out.values().map(|e| e.bytes.len()).sum();
            if total <= budget || cap <= MIN_SURFACE_TEXTURE {
                return out;
            }
            cap /= 2;
        }
    }
}

fn shader_name(map: &MapReader, shader: TagId) -> String {
    let name = map
        .tag(shader)
        .ok()
        .and_then(|t| t.name.as_deref())
        .unwrap_or_default();
    name.rsplit(['\\', '/']).next().unwrap_or(name).to_string()
}

/// A picture as WebP ([`live_server::art_pack::webp`]): lossy at
/// [`WEBP_QUALITY`], the alpha of a cut-out kept exactly.
pub fn encode_image(image: &Image) -> Encoded {
    let bytes = live_server::art_pack::webp(&image.rgba, image.width, image.height, WEBP_QUALITY)
        .expect("a decoded picture encodes");
    Encoded {
        bytes,
        mime: "image/webp",
        width: image.width,
        height: image.height,
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: u32, h: u32, alpha: u8) -> Image {
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                rgba.extend([(x * 7) as u8, (y * 5) as u8, ((x ^ y) * 3) as u8, alpha]);
            }
        }
        Image {
            width: w,
            height: h,
            rgba,
        }
    }

    fn look(shader: u32, bitmap: u32, cutout: Cutout, img: Image) -> Look {
        Look {
            shader,
            name: format!("s{shader}"),
            cutout,
            texture: Arc::new(Texture { bitmap, image: img }),
            scale: [1.0, 1.0],
            plasma: None,
        }
    }

    #[test]
    fn templates_say_solid_cut_out_or_not_a_surface() {
        let t = |s: &str| template_cutout(s);
        assert_eq!(
            t(r"shaders\shader_templates\opaque\tex_bump"),
            Some(Cutout::Opaque)
        );
        assert_eq!(
            t(r"shaders\shader_templates\opaque\tex_bump_env"),
            Some(Cutout::Opaque)
        );
        assert_eq!(
            t(r"shaders\shader_templates\opaque\tex_bump_alpha_test"),
            Some(Cutout::Mask)
        );
        assert_eq!(t(r"shaders\shader_templates\transparent\glass"), None);
        assert_eq!(t(r"shaders\shader_templates\opaque\overlay"), None);
        assert_eq!(
            t(r"shaders\shader_templates\opaque\tex_bump_detail_overlay"),
            Some(Cutout::Opaque)
        );
    }

    #[test]
    fn the_base_map_is_the_first_diffuse_bitmap() {
        let names = vec![
            (
                1,
                r"scenarios\bitmaps\shared\detail_maps\metal_detail".to_string(),
            ),
            (2, r"scenarios\bitmaps\x\wall_bump".to_string()),
            (3, r"scenarios\bitmaps\x\wall_plate".to_string()),
            (4, r"scenarios\bitmaps\x\wall_trim".to_string()),
        ];
        assert_eq!(pick_base(&names), Some(3));
        assert_eq!(pick_base(&names[..2]), None);
    }

    #[test]
    fn solid_pictures_lose_their_alpha_and_shrink_to_the_cap() {
        let big = image(2048, 1024, 40);
        let solid = surface_image(big.clone(), Cutout::Opaque);
        assert_eq!(
            (solid.width, solid.height),
            (MAX_SURFACE_TEXTURE, MAX_SURFACE_TEXTURE / 2)
        );
        assert!(solid.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        let cut = surface_image(big, Cutout::Mask);
        assert!(
            cut.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255),
            "the strongest alpha is opaque"
        );
    }

    #[test]
    fn cut_outs_keep_what_is_a_quarter_of_their_strongest_alpha() {
        let mut im = Image {
            width: 4,
            height: 1,
            rgba: [0u8, 20, 21, 80]
                .iter()
                .flat_map(|&a| [9, 9, 9, a])
                .collect(),
        };
        cut_at_a_quarter(&mut im);
        let kept: Vec<bool> = im
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| p[3] >= 128)
            .collect();
        assert_eq!(kept, [false, false, true, true]);
        assert_eq!(im.rgba[15], 255);
        let mut clear = Image {
            width: 1,
            height: 1,
            rgba: vec![1, 2, 3, 0],
        };
        cut_at_a_quarter(&mut clear);
        assert_eq!(clear.rgba[3], 0);
    }

    #[test]
    fn pictures_are_webp_at_full_size() {
        for alpha in [255, 0] {
            let e = encode_image(&image(1024, 512, alpha));
            assert_eq!(e.mime, "image/webp");
            assert_eq!(&e.bytes[8..12], b"WEBP");
            assert_eq!((e.width, e.height), (1024, 512));
        }
        // A disc-sized picture is not shrunk on its way to the glb.
        let big = surface_image(image(1024, 1024, 255), Cutout::Opaque);
        assert_eq!((big.width, big.height), (1024, 1024));
    }

    #[test]
    fn a_bitmap_is_encoded_once_and_shrunk_to_fit_the_budget() {
        let mut looks = Looks::default();
        let a = looks.add(look(10, 7, Cutout::Opaque, image(512, 512, 255)));
        let b = looks.add(look(11, 7, Cutout::Opaque, image(512, 512, 255)));
        let c = looks.add(look(12, 8, Cutout::Mask, image(256, 256, 0)));
        assert_eq!((a, b, c), (0, 1, 2));
        let all = looks.encode(usize::MAX);
        assert_eq!(all.len(), 2, "two bitmaps");
        assert_eq!(all[&(7, Cutout::Opaque)].width, 512);
        // A budget too small for full size halves every picture.
        let full: usize = all.values().map(|e| e.bytes.len()).sum();
        let small = looks.encode(full - 1);
        assert_eq!(small[&(7, Cutout::Opaque)].width, 256);
        assert_eq!(
            small[&(8, Cutout::Mask)].width,
            256,
            "already within the cap"
        );
        // Never below the floor.
        let tiny = looks.encode(0);
        assert_eq!(tiny[&(7, Cutout::Opaque)].width, MIN_SURFACE_TEXTURE);
    }

    #[test]
    fn shaders_left_without_a_picture_are_named_most_used_first() {
        let mut f = std::collections::BTreeMap::new();
        assert_eq!(fallback_note(&f), None);
        f.insert("tex_bump_mod_custom".to_string(), 1);
        f.insert("(unreadable shader)".to_string(), 3);
        f.insert("illum_custom".to_string(), 1);
        assert_eq!(
            fallback_note(&f).unwrap(),
            "5 shaders drawn in their class colour (no picture): (unreadable shader) (3), \
             illum_custom (1), tex_bump_mod_custom (1)"
        );
    }
}
