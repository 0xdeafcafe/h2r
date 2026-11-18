//! The static world from a map: one BSP's clusters and placed instanced
//! geometry ([`tags::structure::StructureRender`]), each with its
//! lightmap page from the paired lightmap group, and the BSP's material
//! shaders. Read from the user's own map at run time; nothing is copied.

use std::io::{Read, Seek};

use map::{Map, TagId};
use tags::geometry::{RawSource, Section, TagView};
use tags::structure::{Lightmap, LightmapRenderInfo, StructureRender};

use crate::world::{LightmapKey, Owner, PartInput, Placement, SectionInput, World, WorldBuilder};

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
    let v = s.vertices(None).ok()?;
    if v.positions.len() != s.vertex_count() {
        return None;
    }
    Some(SectionInput {
        parts: s
            .parts
            .iter()
            .filter(|p| p.is_drawn())
            .map(|p| PartInput {
                material: p.material,
                transparent: p.kind == PART_TRANSPARENT,
                triangles: s.part_triangles(p),
            })
            .collect(),
        positions: v.positions,
        uvs: v.texcoords,
        lightmap_uvs: v.lightmap_texcoords,
        normals: v.normals,
    })
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
    for (ii, inst) in bsp.instances.iter().enumerate() {
        let Some(Some(def)) = usize::try_from(inst.definition).ok().and_then(|i| defs.get(i)) else {
            continue;
        };
        if !inst.is_rigid() || inst.scale <= 0.0 {
            failed += 1;
            continue;
        }
        let place = Placement {
            scale: inst.scale,
            basis: inst.basis,
            position: inst.position,
        };
        b.add_section(Owner::Instance(ii as u32), def, &place, key(instances.get(ii)));
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
}
