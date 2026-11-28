//! The static world from a map: one BSP's clusters and placed instanced
//! geometry ([`tags::structure::StructureRender`]), each with its
//! lightmap page from the paired lightmap group, and the BSP's material
//! shaders. Read from the user's own map at run time; nothing is copied.

use std::io::{Read, Seek};

use map::{Map, TagId};
use tags::geometry::{RawSource, Section, TagView, VertexKind};
use tags::lighting::{VertexLighting, vertex_lighting};
use tags::structure::{Lightmap, LightmapGroup, LightmapRenderInfo, StructureRender, load_bucket};

use crate::gpu::Image;
use crate::world::{self, LightmapKey, Owner, PartInput, Placement, SectionInput, World, WorldBuilder};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// Part kind of transparent geometry (drawn by the transparents, not the
/// lightmap layer; docs/formats/structure-bsp.md).
pub const PART_TRANSPARENT: u16 = 4;

/// One BSP's world, ready to upload.
pub struct LoadedWorld {
    pub bsp: TagId,
    pub checksum: u32,
    pub world: World,
    /// Shader tag per BSP material (`sbsp+0xa4`, stride `0x20`, shader at
    /// `+0xc`).
    pub shaders: Vec<TagId>,
    /// The lightmap group's bitmap and palettes, when the BSP is lit.
    pub lightmap_bitmap: Option<TagId>,
    /// Placement lighting mode at +0x56. With a matching lightmap group,
    /// the BSP record builder selects implementation 1 for a nonzero mode.
    pub per_vertex_placements: Vec<bool>,
    /// A damaged lightmap tag makes the lighting selector unknown.
    /// Geometry remains available to other passes.
    pub lightmap_error: Option<String>,
    pub palettes: Vec<[u32; 256]>,
    /// Sections that did not decode (left to the D3D path).
    pub failed_sections: u32,
}

/// The scenario's (BSP, lightmap) pairs: `scnr+0x210`, stride `0x44`, BSP
/// at `+0x14`, lightmap at `+0x1c`.
pub fn scenario_bsps<R: Read + Seek>(map: &mut Map<R>) -> Result<Vec<(TagId, Option<TagId>)>, Error> {
    let scenario = map.scenario().ok_or("no scenario")?.clone();
    let location = scenario.location.ok_or("scenario has no payload")?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    Ok(view
        .block(view.at(0, 0x218)?, 0x210, 0x44)?
        .into_iter()
        .map(|b| {
            let word = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            let lightmap = word(0x1c);
            (TagId(word(0x14)), (lightmap != u32::MAX).then_some(TagId(lightmap)))
        })
        .collect())
}

/// Per BSP material, its shader.
pub fn bsp_shaders<R: Read + Seek>(map: &mut Map<R>, bsp: TagId) -> Result<Vec<TagId>, Error> {
    let tag = map.tag(bsp)?.clone();
    let location = tag.location.ok_or("BSP has no payload")?;
    let bytes = map.read_tag_bytes(bsp)?;
    let view = TagView::new(&bytes, location.address);
    Ok(view
        .block(view.at(0, 0xac)?, 0xa4, 0x20)?
        .into_iter()
        .map(|e| TagId(u32::from_le_bytes([e[0xc], e[0xd], e[0xe], e[0xf]])))
        .collect())
}

/// A decoded section as builder input; `None` when its vertices do not
/// decode.
pub fn section_input(s: &Section) -> Option<SectionInput> {
    let tangent_frame = VertexKind::TangentFrame;
    if let Some(frame) = s.stream(tangent_frame)
        && tangent_frame.expected_stride().is_some_and(|stride| frame.stride < stride)
    {
        return None;
    }
    let v = s.vertices(None).ok()?;
    if v.positions.len() != s.vertex_count() {
        return None;
    }
    Some(SectionInput {
        // Every part, in the section's order, so a part's position is its
        // index (what a transparent entry names); a part not drawn has no
        // triangles and becomes no draw.
        parts: s
            .parts
            .iter()
            .map(|p| match p.is_drawn() {
                true => PartInput {
                    material: p.material,
                    transparent: p.kind == PART_TRANSPARENT,
                    triangles: s.part_triangles(p),
                },
                false => PartInput {
                    material: p.material,
                    transparent: false,
                    triangles: Vec::new(),
                },
            })
            .collect(),
        source: None,
        positions: v.positions,
        uvs: v.texcoords,
        lightmap_uvs: v.lightmap_texcoords,
        normals: v.normals,
        binormals: v.binormals,
        tangents: v.tangents,
        lighting: lighting_vectors(s),
        bucket_colors: false,
        baked_colors: s.vertex_buffers.iter().rev().find_map(|vb|
            baked_colors(vb.type_id, vb.stride, s.data(vb))).unwrap_or_default(),
    })
}

/// D3DCOLOR carries alpha; PBYTE3 supplies 1. Both fetches use UNORM8.
fn baked_colors(kind: u8, stride: u8, bytes: &[u8]) -> Option<Vec<[u8; 4]>> {
    match (kind, stride) {
        (0x2e, 4) if bytes.len().is_multiple_of(4) =>
            Some(bytes.as_chunks::<4>().0.iter().map(|&[b, g, r, a]| [r, g, b, a]).collect()),
        (0x2f, 3) if bytes.len().is_multiple_of(3) =>
            Some(bytes.as_chunks::<3>().0.iter().map(|&[r, g, b]| [r, g, b, 255]).collect()),
        _ => None,
    }
}

fn map_section(s: &Section, section: u32) -> Option<SectionInput> {
    let mut input = section_input(s)?;
    input.source = Some(world::light_parts::SourceSection::from_section(u16::try_from(section).ok()?, s));
    Some(input)
}

/// A section's signed lighting vectors (stream `30`, found by
/// `tags::lighting::vertex_lighting`) as the vertex program reads them
/// ([`program_lighting_vector`]); empty without that stream.
pub fn lighting_vectors(s: &Section) -> Vec<[f32; 3]> {
    s.vertex_buffers
        .iter()
        .find_map(|vb| match vertex_lighting(s, vb) {
            Ok(Some(VertexLighting::Vector(_))) => Some(
                s.data(vb)
                    .chunks_exact(4)
                    .map(|b| program_lighting_vector(u32::from_le_bytes([b[0], b[1], b[2], b[3]])))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// One stream-`30` word as the GPU hands it to the vertex program: the
/// stream format table (`0x0043f408`, type `0x30`) declares its one
/// attribute (semantic 8) with format code 16, `NORMPACKED3`, so the
/// hardware decodes `s / 1023`, `s / 1023`, `s / 511`, clamped at -1
/// (`tags::geometry::unpack_normpacked3`, the D3D path's
/// `xvs_normpacked3`). Not the CPU's midpoint decode
/// (`tags::lighting::unpack_lighting_vector`, `(2 s + 1) / (2^bits - 1)`,
/// docs/formats/lighting.md), which sits up to half a step higher in every
/// component: with it the native world's layer 3 came out one level darker
/// than the D3D path's on about 4% of 03a's pixels (03000), clusters and
/// placements alike.
pub fn program_lighting_vector(word: u32) -> [f32; 3] {
    tags::geometry::unpack_normpacked3(word)
}

/// The page a cluster's or instance's render info names: none for page
/// `-1` (unlit), a colour page for palette `-1` (the converted Halo 1
/// pages, docs/halo1/bitmaps.md).
fn key(info: Option<&LightmapRenderInfo>) -> Option<LightmapKey> {
    let info = info?;
    Some(LightmapKey {
        page: u16::try_from(info.bitmap_index).ok()?,
        palette: u8::try_from(info.palette_index).ok(),
    })
}

/// Loads BSP `index` of the scenario.
pub fn load_world<R: Read + Seek>(
    map: &mut Map<R>,
    raw: &mut dyn RawSource,
    index: usize,
) -> Result<LoadedWorld, Error> {
    let bsps = scenario_bsps(map)?;
    let &(bsp_id, lightmap) = bsps.get(index).ok_or("no such BSP")?;
    let shaders = bsp_shaders(map, bsp_id)?;
    let bsp = StructureRender::read(map, bsp_id)?;
    // The record builder tests the first group only. A later group with
    // the same checksum cannot change the lighting implementation.
    let (group, lightmap_error) = match lightmap.map(|id| Lightmap::read(map, id)).transpose() {
        Ok(l) => (l.and_then(|l| l.groups.into_iter().next())
            .filter(|g| g.structure_checksum == bsp.checksum && g.bitmap.is_some()), None),
        Err(e) => (None, Some(e.to_string())),
    };
    let (clusters, instances) = group
        .as_ref()
        .map_or((&[][..], &[][..]), |g| (&g.cluster_render_info[..], &g.instance_render_info[..]));
    let materials = Some(bsp.material_count as usize);
    let mut failed = 0u32;
    let mut b = WorldBuilder::new();
    for (ci, c) in bsp.clusters.iter().enumerate() {
        match c.geometry.load(raw, materials) {
            Ok(Some(s)) => match map_section(&s, ci as u32) {
                Some(input) => b.add_section(Owner::Cluster(ci as u32), &input, &Placement::IDENTITY, key(clusters.get(ci))),
                None => failed += 1,
            },
            Ok(None) => {}
            Err(_) => failed += 1,
        }
    }
    let defs: Vec<Option<SectionInput>> = bsp
        .instance_definitions
        .iter()
        .enumerate()
        .map(|(i, d)| match d.geometry.load(raw, materials) {
            Ok(Some(s)) => map_section(&s, i as u32).or_else(|| {
                failed += 1;
                None
            }),
            Ok(None) => None,
            Err(_) => {
                failed += 1;
                None
            }
        })
        .collect();
    // An instance's lightmap coordinates are its own, in a bucket of the
    // lightmap group, not the definition's (which every placement shares).
    let buckets = group.as_ref().map(|g| bucket_streams(g, raw)).unwrap_or_default();
    for (ii, inst) in bsp.instances.iter().enumerate() {
        let Some(Some(def)) = usize::try_from(inst.definition).ok().and_then(|i| defs.get(i)) else {
            continue;
        };
        let n = def.positions.len();
        let own = group.as_ref().and_then(|g| instance_run(g, &buckets, ii));
        let with_own;
        let def = match own {
            Some((b, first)) => {
                let lightmap_uvs = bucket_slice(&b.lightmap_uvs, first, n).map(<[_]>::to_vec);
                let lighting = bucket_slice(&b.lighting, first, n).map(<[_]>::to_vec);
                with_own = SectionInput {
                    bucket_colors: b.colors.is_some(),
                    baked_colors: match &b.colors {
                        Some(colors) => bucket_slice(colors, first, n).unwrap_or_default().to_vec(),
                        None => def.baked_colors.clone(),
                    },
                    lightmap_uvs: lightmap_uvs.unwrap_or_else(|| def.lightmap_uvs.clone()),
                    lighting: lighting.unwrap_or_else(|| def.lighting.clone()),
                    ..def.clone()
                };
                &with_own
            }
            None => def,
        };
        if !inst.is_rigid() {
            failed += 1;
            continue;
        }
        // Unscaled: the game draws a placement with its rows alone
        // (`0x0004d830` does not apply the scale at `+0`), so the placed
        // copy (bounds, culling, the lights and transparents) is where the
        // D3D path draws it.
        let place = Placement {
            scale: 1.0,
            basis: inst.basis,
            position: inst.position,
        };
        b.add_section(Owner::Instance(ii as u32), def, &place, key(instances.get(ii)));
        // What the structure draw uploads for it, from its tag element.
        if let Some(rows) = world::model_rows(&inst.raw) {
            b.place_rows(ii as u32, rows);
        }
    }
    Ok(LoadedWorld {
        bsp: bsp_id,
        checksum: bsp.checksum,
        world: b.finish(),
        shaders,
        lightmap_bitmap: group.as_ref().and_then(|g| g.bitmap),
        lightmap_error,
        per_vertex_placements: bsp.instances.iter().map(|inst| {
            u16::from_le_bytes([inst.raw[0x56], inst.raw[0x57]]) != 0
        }).collect(),
        palettes: group.map(|g| g.palettes).unwrap_or_default(),
        failed_sections: failed,
    })
}

/// What a live edit names in BSP `bsp`: a cluster, or an instanced
/// definition, whose every rigid placement takes its section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SectionOwner {
    Cluster(u32),
    InstanceDefinition(u32),
}

/// The sections `wanted` of BSP tag `bsp`, decoded again from the map as
/// [`load_world`] decodes them, each with the owner its draws carry: a
/// definition once per rigid placement of it (the placement's own
/// lightmap coordinates are the world's to keep,
/// `World::replace_section`). For a live edit's native refresh
/// (docs/editor/realtime-edits.md, "The native refresh"). An owner that
/// does not exist or does not decode is an error: the host reloads the
/// world.
pub fn load_sections<R: Read + Seek>(
    map: &mut Map<R>,
    raw: &mut dyn RawSource,
    bsp: TagId,
    wanted: &[SectionOwner],
) -> Result<Vec<(Owner, SectionInput)>, Error> {
    let structure = StructureRender::read(map, bsp)?;
    let materials = Some(structure.material_count as usize);
    let mut out = Vec::new();
    for &owner in wanted {
        let (geometry, owners) = match owner {
            SectionOwner::Cluster(i) => {
                let c = structure.clusters.get(i as usize).ok_or("no such cluster")?;
                (&c.geometry, vec![Owner::Cluster(i)])
            }
            SectionOwner::InstanceDefinition(i) => {
                let d = structure.instance_definitions.get(i as usize).ok_or("no such instanced definition")?;
                let placed = structure
                    .instances
                    .iter()
                    .enumerate()
                    .filter(|(_, inst)| usize::try_from(inst.definition).ok() == Some(i as usize) && inst.is_rigid())
                    .map(|(ii, _)| Owner::Instance(ii as u32))
                    .collect();
                (&d.geometry, placed)
            }
        };
        let section = geometry.load(raw, materials)?.ok_or("the section has no geometry")?;
        let index = match owner { SectionOwner::Cluster(i) | SectionOwner::InstanceDefinition(i) => i };
        let input = map_section(&section, index).ok_or("the section's vertices do not decode")?;
        out.extend(owners.into_iter().map(|o| (o, input.clone())));
    }
    Ok(out)
}

/// One lightmap bucket's per-vertex streams.
#[derive(Clone, Debug, Default)]
pub struct BucketStreams {
    /// Stream `1f`.
    pub lightmap_uvs: Vec<[f32; 2]>,
    /// Stream `30`.
    pub lighting: Vec<[f32; 3]>,
    /// Second descriptor when the colour flag is set. An unreadable
    /// bound stream stays empty, so a part requiring it fails admission.
    pub colors: Option<Vec<[u8; 4]>>,
}

/// Each bucket's streams, loaded once. Flag bit 0 enables the first
/// descriptor and bit 1 the baked colour descriptor.
pub fn bucket_streams(group: &LightmapGroup, raw: &mut dyn RawSource) -> Vec<BucketStreams> {
    group
        .buckets
        .iter()
        .map(|b| {
            let color_bound = b.flags & 2 != 0;
            if !b.is_patched() && !color_bound {
                return BucketStreams::default();
            }
            match load_bucket(b, raw) {
                Ok(Some(s)) => BucketStreams {
                    lightmap_uvs: if b.is_patched() {
                        s.vertices(None).map(|v| v.lightmap_texcoords).unwrap_or_default()
                    } else { Vec::new() },
                    lighting: if b.is_patched() { lighting_vectors(&s) } else { Vec::new() },
                    colors: color_bound.then(|| s.vertex_buffers.iter().find(|v| v.slot == 1)
                        .and_then(|v| baked_colors(v.type_id, v.stride, s.data(v))).unwrap_or_default()),
                },
                _ => BucketStreams { colors: color_bound.then(Vec::new), ..Default::default() },
            }
        })
        .collect()
}

/// Instance `index`'s bucket and first vertex in it, as the structure draw
/// binds them (`0x0004d367`..`0x0004d3a4`): its bucket reference (group
/// `+0x54`, one per instance) and the reference's first offset (`None`
/// is the disc's `0xffff`).
pub fn instance_run<'a>(
    group: &LightmapGroup, buckets: &'a [BucketStreams], index: usize,
) -> Option<(&'a BucketStreams, Option<u16>)> {
    let r = group.instance_bucket_refs.get(index)?;
    Some((buckets.get(usize::from(r.bucket))?, *r.section_offsets.first()?))
}

/// `vertices` values of a bucket stream from `offset` (`None` is the
/// disc's `0xffff`), or `None` when they do not fit.
pub fn bucket_slice<T>(stream: &[T], offset: Option<u16>, vertices: usize) -> Option<&[T]> {
    let first = usize::from(offset?);
    stream.get(first..first.checked_add(vertices)?).filter(|s| !s.is_empty())
}

/// A P8 lightmap page's indices (one byte per texel) through a palette of
/// `A8R8G8B8` words, to RGBA8. The palette is a lookup, applied before
/// any filtering (docs/formats/lighting.md).
pub fn expand_page(indices: &[u8], palette: &[u32; 256]) -> Vec<u8> {
    indices
        .iter()
        .flat_map(|&i| {
            let c = palette[usize::from(i)];
            [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]
        })
        .collect()
}

/// The palette `key` names in a group's `palettes`: `None` for a colour
/// page, an error for an index past the block.
pub fn page_palette(palettes: &[[u32; 256]], key: LightmapKey) -> Result<Option<&[u32; 256]>, String> {
    key.palette
        .map(|n| palettes.get(usize::from(n)).ok_or_else(|| "no such lightmap palette".to_owned()))
        .transpose()
}

/// A lightmap page decoded to RGBA8, with its mip chain: a P8 page
/// expanded through `palette`, a colour page (palette `-1`, the converted
/// Halo 1 pages: `R5G6B5`, `A8R8G8B8`, `X8R8G8B8`) decoded direct.
/// `refuse_linear` refuses a linear (unswizzled) page, whose coordinates
/// the game gives in texels.
///
/// # Panics
///
/// A P8 page with no palette, and a colour page with one: the render info
/// and the bitmap disagree.
pub fn decode_lightmap(
    image: &tags::Image,
    palette: Option<&[u32; 256]>,
    files: &mut assets::ResourceFiles,
    refuse_linear: bool,
) -> Result<Image, String> {
    use tags::PixelFormat as F;
    match (image.format, palette) {
        (F::P8, Some(_)) | (F::Rgb565 | F::Argb8888 | F::Xrgb8888, None) => {}
        (F::P8, None) => unimplemented!("lightmap: a P8 page with palette -1"),
        (f @ (F::Rgb565 | F::Argb8888 | F::Xrgb8888), Some(_)) => {
            unimplemented!("lightmap: a {f:?} colour page with a palette")
        }
        (f, _) => return Err(format!("unsupported lightmap format {f:?}")),
    }
    let decoded = assets::decode_image(image, files).map_err(|e| e.to_string())?;
    if refuse_linear && decoded.desc.format.layout() == gfx::format::Layout::Linear {
        return Err("linear lightmap coordinates are in texels".into());
    }
    match palette {
        Some(palette) => expand_levels(decoded.levels, palette),
        None => colour_levels(decoded.levels),
    }
    .ok_or_else(|| "no usable lightmap mip levels".into())
}

/// A colour page's decoded levels (already RGBA8), largest first, as one
/// image with its mip chain.
pub fn colour_levels(levels: Vec<assets::Level>) -> Option<Image> {
    let mut levels: Vec<_> = levels.into_iter().filter(|l| l.face == 0 && l.depth <= 1).collect();
    levels.sort_by_key(|l| l.level);
    Image::from_levels(levels.into_iter().map(|l| (l.width, l.height, l.rgba)).collect())
}

/// A P8 page's decoded levels (the index in every channel) through
/// `palette`, largest first, as one image with its mip chain.
pub fn expand_levels(levels: Vec<assets::Level>, palette: &[u32; 256]) -> Option<Image> {
    let mut levels: Vec<_> = levels.into_iter().filter(|l| l.face == 0 && l.depth <= 1).collect();
    levels.sort_by_key(|l| l.level);
    Image::from_levels(
        levels
            .into_iter()
            .map(|l| {
                // P8 decodes as L8: the index in every channel.
                let indices: Vec<u8> = l.rgba.chunks_exact(4).map(|p| p[0]).collect();
                (l.width, l.height, expand_page(&indices, palette))
            })
            .collect(),
    )
}

/// The first image of a bitmap as RGBA, with its mip chain.
pub fn decode_first<R: Read + Seek>(map: &mut Map<R>, files: &mut assets::ResourceFiles, id: TagId) -> Option<Image> {
    decode_image(map, files, id, 0)
}

/// Image `index` of a bitmap as RGBA, with its mip chain.
pub fn decode_image<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    id: TagId,
    index: usize,
) -> Option<Image> {
    let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, id).ok()?.data else {
        return None;
    };
    let d = assets::decode_image(b.images.get(index)?, files).ok()?;
    let mut levels: Vec<_> = d.levels.into_iter().filter(|l| l.face == 0 && l.depth <= 1).collect();
    levels.sort_by_key(|l| l.level);
    // The whole decoded chain: the native pass samples with mipmaps, as
    // the D3D path does.
    Image::from_levels(levels.into_iter().map(|l| (l.width, l.height, l.rgba)).collect())
}

/// A shader's template name and its bitmaps in parameter order: `shad +4`
/// the template's datum, `shad +0x20` the compiled postprocess (first
/// element, `0x7c` bytes) whose `+4` block lists the bitmap parameters
/// (`0xc` bytes, datum first; docs/formats/shader-instances.md).
pub fn shader_bitmaps<R: Read + Seek>(map: &mut Map<R>, shader: TagId) -> Result<(String, Vec<TagId>), Error> {
    let tag = map.tag(shader)?.clone();
    if tag.group != map::FourCC(*b"shad") {
        return Err("not a shader".into());
    }
    let address = tag.location.ok_or("shader has no payload")?.address;
    let bytes = map.read_tag_bytes(shader)?;
    let view = TagView::new(&bytes, address);
    let word = |b: &[u8], o: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?)) };
    let template = word(&bytes, 4)
        .and_then(|id| map.tag(TagId(id)).ok())
        .and_then(|t| t.name.clone())
        .unwrap_or_default();
    let root = view.at(0, 0x54)?;
    let post = view.block(root, 0x20, 0x7c)?;
    let post = post.first().ok_or("no compiled postprocess")?;
    let ids = view
        .block(post, 4, 0xc)?
        .iter()
        .filter_map(|b| word(b, 0).map(TagId))
        .filter(|id| *id != TagId::NULL)
        .collect();
    Ok((template, ids))
}

/// Whether a bitmap's tag name reads as a base (diffuse) map: not a
/// default, detail, bump, reflection, change-colour, self-illumination,
/// specular or mask map. A name rule, stage 1's stand-in until each
/// template's own program draws its material (docs/graphics/native-world.md,
/// "Match the look").
pub fn is_base_map_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let last = lower.rsplit('\\').next().unwrap_or(&lower);
    let folders = ["default_bitmaps", "rasterizer", "detail_maps", "reflection_maps", "cube"];
    let words = [
        "_cc", "change_color", "change_colour", "color_change", "colour_change", "bump", "_bmp", "detail", "reflection",
        "cube", "fade", "normal", "illum", "self", "specular", "mask",
    ];
    !(lower.split('\\').any(|c| folders.contains(&c)) || words.iter().any(|w| last.contains(w)))
}

/// A material's base map: the first bitmap of its shader whose name reads
/// as one ([`is_base_map_name`]).
pub fn base_map<R: Read + Seek>(map: &mut Map<R>, shader: TagId) -> Option<TagId> {
    let (_, bitmaps) = shader_bitmaps(map, shader).ok()?;
    bitmaps
        .into_iter()
        .find(|&b| map.tag(b).ok().and_then(|t| t.name.clone()).is_some_and(|n| is_base_map_name(&n)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A P8 page's levels go through the palette, largest first; a
    /// cube face past the first and a level that is not half the last
    /// are left out.
    #[test]
    fn a_p8_pages_levels_expand_through_the_palette() {
        let mut palette = [0u32; 256];
        palette[1] = 0x8010_2030;
        palette[2] = 0xff40_5060;
        let level = |level: u32, size: u32, index: u8, face: u32| assets::Level {
            face,
            level,
            width: size,
            height: size,
            depth: 1,
            rgba: [index, index, index, 255].repeat((size * size) as usize),
        };
        let image = expand_levels(vec![level(1, 1, 2, 0), level(0, 2, 1, 0), level(0, 2, 2, 1)], &palette).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.rgba, [0x10, 0x20, 0x30, 0x80].repeat(4));
        assert_eq!(image.mips, vec![vec![0x40, 0x50, 0x60, 0xff]]);
        assert!(expand_levels(vec![level(0, 2, 1, 1)], &palette).is_none(), "no first face");
    }

    #[test]
    fn baked_color_streams_preserve_channels_and_the_placement_offset() {
        let packed = baked_colors(0x2e, 4, &[1, 3, 5, 7, 19, 23, 29, 31]).unwrap();
        assert_eq!(bucket_slice(&packed, Some(1), 1), Some(&[[29, 23, 19, 31]][..]));
        let rgb = baked_colors(0x2f, 3, &[29, 23, 19]).unwrap();
        assert_eq!(rgb, [[29, 23, 19, 255]]);
        assert!(baked_colors(0x2f, 4, &[0; 4]).is_none());
        assert!(baked_colors(0x2e, 4, &[0; 3]).is_none());
    }

    #[test]
    fn an_instance_takes_its_run_of_the_bucket() {
        let uvs: Vec<[f32; 2]> = (0..6).map(|i| [i as f32, 0.0]).collect();
        assert_eq!(bucket_slice(&uvs, Some(2), 3), Some(&uvs[2..5]));
        assert_eq!(bucket_slice(&uvs, Some(4), 3), None, "past the bucket");
        assert_eq!(bucket_slice(&uvs, None, 3), None, "0xffff: no lightmap vertices");
        assert_eq!(bucket_slice::<[f32; 2]>(&[], Some(0), 0), None, "nothing to take");
    }

    #[test]
    fn base_map_names() {
        assert!(is_base_map_name("scenarios\\bitmaps\\oldmombasa\\walls\\concrete_wall"));
        for other in [
            "shaders\\default_bitmaps\\bitmaps\\gray_50_percent",
            "scenarios\\bitmaps\\x\\concrete_wall_bump",
            "scenarios\\bitmaps\\x\\detail_maps\\grit",
            "scenarios\\bitmaps\\x\\light_illum",
            "scenarios\\bitmaps\\x\\metal_cc",
        ] {
            assert!(!is_base_map_name(other), "{other}");
        }
    }

    /// A 2x2 linear page of `format` at offset 0 of the shared resource
    /// file, 128 bytes (pitch 64).
    fn page_image(format: tags::PixelFormat) -> tags::Image {
        use tags::wire::{BinaryRecord, Flags16};
        let mut wire = tags::records::BitmapImage::decode(&[0u8; tags::records::BitmapImage::SIZE]).unwrap();
        wire.flags = Flags16(0x10);
        let lod = tags::ResourceRange { pointer: map::RawPointer { file: map::ResourceFile::Shared, offset: 0 }, size: 128 };
        tags::Image {
            format,
            kind: tags::TextureKind::D2,
            dimensions: [2, 2, 1],
            mip_count: 0,
            lods: [Some(lod), None, None],
            low_detail: None,
            wire,
        }
    }

    /// A converted Halo 1 page (`R5G6B5`, render info palette -1) decodes
    /// direct, its colour as the format holds it.
    #[test]
    fn a_colour_page_decodes_without_a_palette() {
        // Red, green, blue, white: 0xf800, 0x07e0, 0x001f, 0xffff.
        let row = [0x00, 0xf8, 0xe0, 0x07];
        let mut bytes = row.to_vec();
        bytes.resize(64, 0);
        bytes.extend([0x1f, 0x00, 0xff, 0xff]);
        bytes.resize(128, 0);
        let path = std::env::temp_dir().join(format!("native-render-colour-page-{}.bin", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let mut files = assets::ResourceFiles::new();
        files.set(map::ResourceFile::Shared, assets::Region { path: path.clone(), base: 0, length: 128 }).unwrap();
        let page = decode_lightmap(&page_image(tags::PixelFormat::Rgb565), None, &mut files, false);
        std::fs::remove_file(path).unwrap();
        let page = page.unwrap();
        assert_eq!((page.width, page.height), (2, 2));
        assert_eq!(page.rgba, [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]].concat());
    }

    #[test]
    #[should_panic(expected = "lightmap: a P8 page with palette -1")]
    fn a_p8_page_without_a_palette_panics() {
        let _ = decode_lightmap(&page_image(tags::PixelFormat::P8), None, &mut assets::ResourceFiles::new(), false);
    }

    #[test]
    #[should_panic(expected = "lightmap: a Rgb565 colour page with a palette")]
    fn a_colour_page_with_a_palette_panics() {
        let _ = decode_lightmap(&page_image(tags::PixelFormat::Rgb565), Some(&[0; 256]), &mut assets::ResourceFiles::new(), false);
    }

    #[test]
    fn a_key_finds_its_palette() {
        let palettes = [[1u32; 256], [2; 256]];
        let key = |palette| LightmapKey { page: 0, palette };
        assert_eq!(page_palette(&palettes, key(None)), Ok(None));
        assert_eq!(page_palette(&palettes, key(Some(1))), Ok(Some(&[2; 256])));
        assert!(page_palette(&palettes, key(Some(2))).is_err());
    }

    #[test]
    fn a_page_expands_through_its_palette() {
        let mut palette = [0u32; 256];
        palette[0] = 0xff10_2030;
        palette[7] = 0x80ff_0001;
        assert_eq!(expand_page(&[7, 0], &palette), vec![0xff, 0, 1, 0x80, 0x10, 0x20, 0x30, 0xff]);
    }

    #[test]
    fn render_info_without_a_page_is_unlit_and_without_a_palette_is_colour() {
        let info = |bitmap_index, palette_index| LightmapRenderInfo {
            bitmap_index,
            palette_index,
            unknown_3: 0,
        };
        assert_eq!(key(Some(&info(3, 2))), Some(LightmapKey { page: 3, palette: Some(2) }));
        assert_eq!(key(Some(&info(-1, 2))), None);
        // Palette -1 is a colour page (the converted Halo 1 pages).
        assert_eq!(key(Some(&info(3, -1))), Some(LightmapKey { page: 3, palette: None }));
        assert_eq!(key(None), None);
    }

    #[test]
    fn the_lighting_vector_is_read_as_the_vertex_program_reads_it() {
        // Zero is zero to the hardware, not the midpoint decode's half step.
        assert_eq!(program_lighting_vector(0), [0.0; 3]);
        assert_ne!(tags::lighting::unpack_lighting_vector(0), [0.0; 3]);
        // The largest fields are one; the smallest clamp to -1.
        let max = 1023 | 1023 << 11 | 511 << 22;
        let min = 1024 | 1024 << 11 | 512 << 22;
        assert_eq!(program_lighting_vector(max), [1.0; 3]);
        assert_eq!(program_lighting_vector(min), [-1.0; 3]);
        // x 10, y -3, z 100: over 1023, 1023 and 511 (NORMPACKED3).
        let w = 10 | (0x7ff - 2) << 11 | 100 << 22;
        assert_eq!(program_lighting_vector(w), [10.0 / 1023.0, -3.0 / 1023.0, 100.0 / 511.0]);
    }

    /// The world vertex's packed words (`world::pack_normpacked3`, what
    /// the material programs decode `v2..v5` from): a value decoded from a
    /// `NORMPACKED3` word packs back to it, but the smallest fields, which
    /// come back as the next ones up and decode to -1 as they do.
    #[test]
    fn a_decoded_frame_packs_back_to_its_word() {
        use crate::world::pack_normpacked3;
        use tags::geometry::unpack_normpacked3;
        for x in [-1023i32, -511, -1, 0, 1, 2, 500, 1023] {
            for z in [-511i32, -3, 0, 7, 511] {
                let word = (x as u32 & 0x7ff) | ((-x) as u32 & 0x7ff) << 11 | (z as u32 & 0x3ff) << 22;
                assert_eq!(pack_normpacked3(unpack_normpacked3(word)), word, "x {x} z {z}");
            }
        }
        let min = 0x400 | 0x400 << 11 | 0x200 << 22;
        let packed = pack_normpacked3(unpack_normpacked3(min));
        assert_eq!(packed, 0x401 | 0x401 << 11 | 0x201 << 22);
        assert_eq!(unpack_normpacked3(packed), [-1.0; 3]);
    }
}
