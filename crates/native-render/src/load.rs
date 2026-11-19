//! The static world from a map: one BSP's clusters and placed instanced
//! geometry ([`tags::structure::StructureRender`]), each with its
//! lightmap page from the paired lightmap group, and the BSP's material
//! shaders. Read from the user's own map at run time; nothing is copied.

use std::io::{Read, Seek};

use map::{Map, TagId};
use tags::geometry::{RawSource, Section, TagView, VertexKind};
use tags::lighting::{VertexLighting, vertex_lighting};
use tags::structure::{Lightmap, LightmapGroup, LightmapRenderInfo, StructureRender, load_bucket};

use crate::world::{self, LightmapKey, Owner, PartInput, Placement, SectionInput, World, WorldBuilder};

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// Part kind of transparent geometry (drawn by the transparents, not the
/// lightmap layer; docs/formats/structure-bsp.md).
pub const PART_TRANSPARENT: u16 = 4;

/// One BSP's world, ready to upload.
pub struct LoadedWorld {
    pub bsp: TagId,
    pub world: World,
    /// Shader tag per BSP material (`sbsp+0xa4`, stride `0x20`, shader at
    /// `+0xc`).
    pub shaders: Vec<TagId>,
    /// The lightmap group's bitmap and palettes, when the BSP is lit.
    pub lightmap_bitmap: Option<TagId>,
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
        positions: v.positions,
        uvs: v.texcoords,
        lightmap_uvs: v.lightmap_texcoords,
        normals: v.normals,
        binormals: v.binormals,
        tangents: v.tangents,
        lighting: lighting_vectors(s),
    })
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

fn key(info: Option<&LightmapRenderInfo>) -> Option<LightmapKey> {
    let info = info?;
    Some(LightmapKey {
        page: u16::try_from(info.bitmap_index).ok()?,
        palette: u8::try_from(info.palette_index).ok()?,
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
    let group = lightmap.and_then(|l| Lightmap::read(map, l).ok()).and_then(|l| {
        l.groups
            .into_iter()
            .find(|g| g.structure_checksum == bsp.checksum && g.bitmap.is_some())
    });
    let (clusters, instances) = group
        .as_ref()
        .map_or((&[][..], &[][..]), |g| (&g.cluster_render_info[..], &g.instance_render_info[..]));
    let materials = Some(bsp.material_count as usize);
    let mut failed = 0u32;
    let mut b = WorldBuilder::new();
    for (ci, c) in bsp.clusters.iter().enumerate() {
        match c.geometry.load(raw, materials) {
            Ok(Some(s)) => match section_input(&s) {
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
        .map(|d| match d.geometry.load(raw, materials) {
            Ok(Some(s)) => section_input(&s).or_else(|| {
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
        world: b.finish(),
        shaders,
        lightmap_bitmap: group.as_ref().and_then(|g| g.bitmap),
        palettes: group.map(|g| g.palettes).unwrap_or_default(),
        failed_sections: failed,
    })
}

/// One lightmap bucket's per-vertex streams.
#[derive(Clone, Debug, Default)]
pub struct BucketStreams {
    /// Stream `1f`.
    pub lightmap_uvs: Vec<[f32; 2]>,
    /// Stream `30`.
    pub lighting: Vec<[f32; 3]>,
}

/// Each of `group`'s buckets' streams, loaded once; empty for a bucket the
/// engine does not patch (flag bit 0), or one that does not load.
pub fn bucket_streams(group: &LightmapGroup, raw: &mut dyn RawSource) -> Vec<BucketStreams> {
    group
        .buckets
        .iter()
        .map(|b| {
            if !b.is_patched() {
                return BucketStreams::default();
            }
            match load_bucket(b, raw) {
                Ok(Some(s)) => BucketStreams {
                    lightmap_uvs: s.vertices(None).map(|v| v.lightmap_texcoords).unwrap_or_default(),
                    lighting: lighting_vectors(&s),
                },
                _ => BucketStreams::default(),
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
/// template's own program draws its material (docs/graphics/native-renderer.md,
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

    #[test]
    fn a_page_expands_through_its_palette() {
        let mut palette = [0u32; 256];
        palette[0] = 0xff10_2030;
        palette[7] = 0x80ff_0001;
        assert_eq!(expand_page(&[7, 0], &palette), vec![0xff, 0, 1, 0x80, 0x10, 0x20, 0x30, 0xff]);
    }

    #[test]
    fn render_info_without_a_page_or_palette_is_unlit() {
        let info = |bitmap_index, palette_index| LightmapRenderInfo {
            bitmap_index,
            palette_index,
            unknown_3: 0,
        };
        assert_eq!(key(Some(&info(3, 2))), Some(LightmapKey { page: 3, palette: 2 }));
        assert_eq!(key(Some(&info(-1, 2))), None);
        assert_eq!(key(Some(&info(3, -1))), None);
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
