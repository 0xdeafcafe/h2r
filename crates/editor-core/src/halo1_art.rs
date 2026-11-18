//! A Halo 1 map's structure BSP art for the viewport: each lightmap
//! material's base map (through its shader) and each lightmap's page, as
//! RGBA8 pictures, and the parts that draw with them. `scenario_edit`'s
//! scene and renderer draw the result the way they draw Halo 2's BSP.
//!
//! Halo 1's shader groups name their base map at different offsets; the
//! reads below are the smallest the viewport needs (docs/editor/editor.md,
//! "Halo 1 maps").
//!
//! Sources: the field offsets are facts read from Invader's published tag
//! layouts (<https://github.com/SnowyMouse/invader>, GPL; cited, no code or
//! definition files copied) and checked on the owner's disc by
//! `disc_multiplayer_maps_are_textured_and_lightmapped`.
use std::collections::HashMap;

use h1bitmaps::BitmapExt;
use h1map::bitmap::{Bitmap, ImageKind};
use h1map::bsp::{Lightmap, unpack_normal};
use h1map::{Block, Group, H1Map, TagId, TagRef};

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Pictures are decoded at the first mip level no larger than this a side.
pub const MAX_SIDE: u32 = 512;

/// An RGBA8 picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct H1Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// How a part blends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum H1Blend {
    #[default]
    Opaque,
    /// Cut out where the base map's alpha is low.
    Mask,
    /// Drawn see-through after everything else.
    Translucent,
}

/// What a shader draws with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct H1Look {
    /// The base map's tag, if the shader names a bitmap there.
    pub base: Option<TagId>,
    pub blend: H1Blend,
}

/// One lightmap material of a BSP, ready to draw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct H1Part {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Page-relative lightmap coordinates; empty without a page.
    pub lightmap_uvs: Vec<[f32; 2]>,
    pub tris: Vec<[u32; 3]>,
    /// Index into [`H1Art::textures`].
    pub texture: Option<usize>,
    /// Index into [`H1Art::lightmaps`].
    pub lightmap: Option<usize>,
    pub blend: H1Blend,
}

/// The structure BSPs' art.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct H1Art {
    pub parts: Vec<H1Part>,
    pub textures: Vec<H1Picture>,
    pub lightmaps: Vec<H1Picture>,
    /// Distinct shaders of the BSPs' materials, and how many of them have
    /// a decoded base map.
    pub shaders: usize,
    pub textured: usize,
    /// The shaders without one.
    pub untextured: Vec<TagId>,
}

const BITM: Group = Group::new(b"bitm");

/// Halo 1 shader groups, with where each names the map the viewport draws.
/// Every offset is past the 0x28-byte `shdr` header. V: on bloodgulch,
/// sidewinder, hangemhigh, damnation and beavercreek every tag of the
/// group has a `bitm` reference that resolves there (senv 71, soso 478,
/// schi 101, sotr 108, sgla 30, smet 35, spla 5, swat 2); the block
/// offsets and strides of schi and sotr resolve on bloodgulch.
pub fn shader_look(map: &H1Map, shader: TagId) -> Option<H1Look> {
    let tag = map.tag(shader).ok()?;
    let d = map.tag_data(tag).ok()?;
    let at_ref = |bytes: &[u8], at: usize| -> Option<TagId> {
        let r = TagRef::read(bytes, at)?;
        (r.group == BITM && !r.id.is_none()).then_some(r.id)
    };
    let u16_at = |at: usize| -> Option<u16> {
        Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
    };
    let group = tag.group;
    let g = |c: &[u8; 4]| group == Group::new(c);
    // The first element's map of a maps block: `stride` bytes each, the
    // map reference at `at`.
    let first_map = |block_at: usize, stride: usize, at: usize| -> Option<TagId> {
        let b = Block::read(d, block_at)?;
        if b.count == 0 {
            return None;
        }
        at_ref(map.block(b, stride).ok()?, at)
    };
    let (base, blend) = if g(b"senv") {
        // +0x28 flags (bit 0 alpha tested), +0x88 base map.
        let tested = u16_at(0x28)? & 1 != 0;
        (
            at_ref(d, 0x88),
            if tested {
                H1Blend::Mask
            } else {
                H1Blend::Opaque
            },
        )
    } else if g(b"soso") {
        // +0xa4 base map.
        (at_ref(d, 0xa4), H1Blend::Opaque)
    } else if g(b"schi") {
        // +0x54 maps block, 0xdc each, the map at +0x6c.
        (first_map(0x54, 0xdc, 0x6c), H1Blend::Translucent)
    } else if g(b"scex") {
        // As schi's: +0x54 four-stage maps, +0x60 two-stage maps. No
        // multiplayer map has one; not checked on the disc.
        (
            first_map(0x54, 0xdc, 0x6c).or_else(|| first_map(0x60, 0xdc, 0x6c)),
            H1Blend::Translucent,
        )
    } else if g(b"sotr") {
        // Xbox's generic transparent shader: +0x54 maps block, 0x64 each,
        // the map at +0x1c.
        (first_map(0x54, 0x64, 0x1c), H1Blend::Translucent)
    } else if g(b"swat") || g(b"smet") {
        // +0x4c base map (water) or map (meter).
        (at_ref(d, 0x4c), H1Blend::Translucent)
    } else if g(b"sgla") {
        // +0xac, else +0x158 (some glass names only the second).
        (
            at_ref(d, 0xac).or_else(|| at_ref(d, 0x158)),
            H1Blend::Translucent,
        )
    } else if g(b"spla") {
        (at_ref(d, 0xd4), H1Blend::Translucent)
    } else {
        return None;
    };
    Some(H1Look { base, blend })
}

/// A bitmap's image `index`, at the first mip level no larger than
/// [`MAX_SIDE`]. 2D images only.
pub fn picture(map: &H1Map, bitmap: &Bitmap, index: usize) -> Option<H1Picture> {
    let img = bitmap.images.get(index)?;
    if img.kind != ImageKind::D2 {
        return None;
    }
    let mut mip = 0;
    while u32::from(img.width.max(img.height)) >> mip > MAX_SIDE && mip < u32::from(img.mip_count) {
        mip += 1;
    }
    let p = bitmap.decode(map, index, mip).ok()?;
    Some(H1Picture {
        width: p.width,
        height: p.height,
        rgba: p.rgba,
    })
}

/// Halves a Halo 1 lightmap page's colour (RGBA8; alpha kept, each
/// channel rounded to nearest, halves up): Halo 1's light, the albedo's
/// multiplier, on the scale the renderer draws (albedo × page × 2, as
/// Halo 2 does), the same as the converter's pages
/// (`h1bitmaps::atlas::to_halo2_light`). Drawn as stored, Halo 1's full
/// light doubled.
pub fn to_halo2_light(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        for c in &mut px[..3] {
            *c = (u16::from(*c)).div_ceil(2) as u8;
        }
    }
}

/// Builds the art of `map`'s structure BSPs from what [`crate::halo1::view`]
/// read: per BSP its render surfaces, lightmaps and lightmap bitmap.
pub struct ArtBuilder<'m> {
    map: &'m H1Map,
    art: H1Art,
    looks: HashMap<TagId, (Option<usize>, H1Blend)>,
    bitmaps: HashMap<TagId, Option<Bitmap>>,
}

impl<'m> ArtBuilder<'m> {
    pub fn new(map: &'m H1Map) -> Self {
        Self {
            map,
            art: H1Art::default(),
            looks: HashMap::new(),
            bitmaps: HashMap::new(),
        }
    }

    fn bitmap(&mut self, id: TagId) -> Option<&Bitmap> {
        let map = self.map;
        self.bitmaps
            .entry(id)
            .or_insert_with(|| {
                let tag = map.tag(id).ok()?;
                (tag.group == BITM)
                    .then(|| Bitmap::read(map, tag).ok())
                    .flatten()
            })
            .as_ref()
    }

    fn look(&mut self, shader: TagId) -> (Option<usize>, H1Blend) {
        if let Some(l) = self.looks.get(&shader) {
            return *l;
        }
        self.art.shaders += 1;
        let look = shader_look(self.map, shader);
        let blend = look.map_or(H1Blend::Opaque, |l| l.blend);
        let texture = look.and_then(|l| l.base).and_then(|b| {
            let map = self.map;
            let p = picture(map, self.bitmap(b)?, 0)?;
            self.art.textures.push(p);
            Some(self.art.textures.len() - 1)
        });
        if texture.is_some() {
            self.art.textured += 1;
        } else {
            self.art.untextured.push(shader);
        }
        self.looks.insert(shader, (texture, blend));
        (texture, blend)
    }

    /// One BSP: its render surfaces, its lightmaps (with materials) and its
    /// lightmap bitmap's pages per lightmap (`h1bitmaps::lightmap_pages`).
    pub fn bsp(
        &mut self,
        surfaces: &[[u16; 3]],
        lightmaps: &[Lightmap],
        bitmap: Option<TagId>,
        pages: &[Option<u16>],
    ) {
        for (li, l) in lightmaps.iter().enumerate() {
            let page = pages.get(li).copied().flatten().and_then(|p| {
                let map = self.map;
                let mut pic = picture(map, self.bitmap(bitmap?)?, usize::from(p))?;
                to_halo2_light(&mut pic.rgba);
                self.art.lightmaps.push(pic);
                Some(self.art.lightmaps.len() - 1)
            });
            for m in &l.materials {
                let (texture, blend) = self.look(m.shader.id);
                let lit = page.filter(|_| m.lightmap_vertices.len() == m.vertices.len());
                let first = m.first_surface as usize;
                let Some(tris) =
                    surfaces.get(first..first.saturating_add(m.surface_count as usize))
                else {
                    continue;
                };
                let mut part = H1Part {
                    texture,
                    lightmap: lit,
                    blend,
                    ..H1Part::default()
                };
                let mut remap = HashMap::new();
                for t in tris {
                    if t.iter().any(|&i| usize::from(i) >= m.vertices.len()) {
                        continue;
                    }
                    let tri = t.map(|i| {
                        *remap.entry(i).or_insert_with(|| {
                            let v = &m.vertices[usize::from(i)];
                            part.positions.push(v.position);
                            part.normals.push(unpack_normal(v.frame[0]));
                            part.uvs.push(v.texcoord);
                            if lit.is_some() {
                                part.lightmap_uvs
                                    .push(m.lightmap_vertices[usize::from(i)].texcoord);
                            }
                            (part.positions.len() - 1) as u32
                        })
                    });
                    // Halo 1 winds its surfaces clockwise seen from the
                    // front (its vertex normals' side); the renderer culls
                    // clockwise ones, so turn each to face its normals.
                    let p = tri.map(|i| part.positions[i as usize]);
                    let face = cross(sub(p[1], p[0]), sub(p[2], p[0]));
                    let n = tri.iter().fold([0.0; 3], |a, &i| {
                        let n = part.normals[i as usize];
                        [a[0] + n[0], a[1] + n[1], a[2] + n[2]]
                    });
                    let facing = face[0] * n[0] + face[1] * n[1] + face[2] * n[2];
                    part.tris.push(if facing < 0.0 {
                        [tri[0], tri[2], tri[1]]
                    } else {
                        tri
                    });
                }
                if !part.tris.is_empty() {
                    self.art.parts.push(part);
                }
            }
        }
    }

    pub fn finish(self) -> H1Art {
        self.art
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use h1map::bsp::{LightmapVertex, Material, RenderVertex};
    use std::io::Cursor;

    /// A page's colour halves (full light 255 to 128, which the
    /// renderer's ×2 draws full again); alpha stays.
    #[test]
    fn lightmap_pages_draw_on_halo2s_scale() {
        let mut rgba = vec![255, 0, 7, 200, 128, 64, 1, 255];
        to_halo2_light(&mut rgba);
        assert_eq!(rgba, [128, 0, 4, 200, 64, 32, 1, 255]);
        for v in 0..=255u8 {
            let mut px = [v, v, v, v];
            to_halo2_light(&mut px);
            assert!((f32::from(px[0]) - f32::from(v) / 2.0).abs() <= 0.5);
            assert_eq!(px[3], v);
        }
    }

    fn tiny() -> H1Map {
        H1Map::from_reader(Cursor::new(h1map::testing::tiny_map())).unwrap()
    }

    fn material(shader: TagId, lit: bool) -> Material {
        let v = |p: [f32; 3]| RenderVertex {
            position: p,
            // +z packed 11:11:10: z = 511.
            frame: [511 << 22, 0, 0],
            texcoord: [p[0], p[1]],
        };
        let vertices = vec![
            v([0.0, 0.0, 0.0]),
            v([1.0, 0.0, 0.0]),
            v([1.0, 1.0, 0.0]),
            v([0.0, 1.0, 0.0]),
        ];
        Material {
            shader: TagRef {
                group: Group::new(b"senv"),
                name_address: 0,
                name_len: 0,
                id: shader,
            },
            first_surface: 1,
            surface_count: 2,
            lightmap_vertices: if lit {
                vertices
                    .iter()
                    .map(|v| LightmapVertex {
                        normal: 0,
                        texcoord: [v.position[0] * 0.5, v.position[1] * 0.5],
                    })
                    .collect()
            } else {
                Vec::new()
            },
            vertices,
        }
    }

    /// Each material is a part with its own vertices (normals unpacked,
    /// UVs kept); lightmap UVs come only with a page, and a material
    /// without lightmap vertices stays unlit on a lit lightmap.
    #[test]
    fn materials_become_parts_with_their_lightmap_coordinates() {
        let map = tiny();
        let mut b = ArtBuilder::new(&map);
        let surfaces = [[0, 0, 0], [0, 1, 2], [0, 2, 3]];
        let lightmaps = [
            Lightmap {
                bitmap: -1,
                materials: vec![material(TagId(u32::MAX), true)],
            },
            Lightmap {
                bitmap: 0,
                materials: vec![material(TagId(u32::MAX), false)],
            },
        ];
        // No lightmap bitmap: no page decodes, nothing is lit.
        b.bsp(&surfaces, &lightmaps, None, &[None, Some(0)]);
        let art = b.finish();
        assert_eq!(art.parts.len(), 2);
        let p = &art.parts[0];
        assert_eq!(p.tris, [[0, 1, 2], [0, 2, 3]]);
        assert_eq!(p.positions.len(), 4);
        assert!((p.normals[0][2] - 1.0).abs() < 1e-3, "{:?}", p.normals[0]);
        assert_eq!(p.uvs[2], [1.0, 1.0]);
        assert!(p.lightmap.is_none() && p.lightmap_uvs.is_empty());
        assert!(art.parts.iter().all(|p| p.texture.is_none()));
        // One distinct shader, and it has no base map.
        assert_eq!((art.shaders, art.textured), (1, 0));
        assert!(art.lightmaps.is_empty());
    }

    /// The invented map's `senv` is too short to name a base map: the look
    /// reads its flags and finds no bitmap rather than reading past it.
    /// Tags of other groups have no look.
    #[test]
    fn a_short_shader_has_no_base_map() {
        let map = tiny();
        let senv = map
            .find(Group::new(b"senv"), "levels\\test\\tiny\\shaders\\floor")
            .unwrap();
        let look = shader_look(&map, senv.id).unwrap();
        assert_eq!(look.base, None);
        let hmt = map
            .tags()
            .iter()
            .find(|t| t.group == Group::new(b"hmt "))
            .unwrap();
        assert_eq!(shader_look(&map, hmt.id), None);
    }

    /// Every multiplayer map of the owner's Halo 1 disc (`$H1_ISO`,
    /// `$H1_MAP_DIR` or `disc/halo1.iso`): how many of its BSP shaders draw
    /// with a base map, and how many lightmap pages it has.
    #[test]
    #[ignore = "needs the owner's Halo 1 disc or maps"]
    fn disc_multiplayer_maps_are_textured_and_lightmapped() {
        use crate::halo1::{H1Source, default_source, view};
        let Some(path) = default_source() else {
            eprintln!("no Halo 1 source: set H1_ISO or H1_MAP_DIR");
            return;
        };
        let src = H1Source::open(&path).unwrap();
        let skip: Vec<&str> = h1map::disc::CAMPAIGN_LEVELS
            .iter()
            .chain(&h1map::disc::CAMPAIGN_EXTRAS)
            .copied()
            .collect();
        let mut seen = 0;
        for name in src.map_names() {
            if skip.iter().any(|s| s.eq_ignore_ascii_case(&name)) {
                continue;
            }
            let map = src.open_map(&name).unwrap();
            let art = view(&name, &map).art;
            for id in &art.untextured {
                let t = map.tag(*id).map(|t| format!("{} {}", t.group, t.name));
                eprintln!("  untextured: {}", t.unwrap_or_else(|e| e.to_string()));
            }
            let lit = art.parts.iter().filter(|p| p.lightmap.is_some()).count();
            eprintln!(
                "{name}: {} of {} shaders textured, {} lightmap pages, {lit} of {} parts lit",
                art.textured,
                art.shaders,
                art.lightmaps.len(),
                art.parts.len()
            );
            assert!(art.textured * 10 >= art.shaders * 9, "{name}");
            assert!(!art.lightmaps.is_empty() && lit > 0, "{name}");
            seen += 1;
        }
        assert_eq!(seen, 13, "the disc has 13 multiplayer maps");
    }
}
