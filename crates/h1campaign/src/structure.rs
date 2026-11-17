//! Each Halo 1 structure BSP of a level, in Halo 1's order, converted into
//! the base's BSP slot of the same index or, past the base's count, new
//! `sbsp`/`ltmp` tags under Halo 1's name: `h1convert`'s collision, render geometry, materials, MOPP
//! and lightmap (docs/game/h1-multiplayer.md, "The converted structure
//! BSP"), plus AI pathfinding generated from the collision
//! (`pathfind::generate`, docs/formats/pathfinding.md) at `sbsp+0xc4`.

use std::collections::BTreeMap;
use std::io::{Read, Seek};

use h1convert::shaders::{self, BaseShader};
use h1convert::structure::{self, Mesh, Section, Structure, Templates};
use h1map::H1Map;
use map::build::{TagPayload, TagSet};
use map::{Map, TagId};
use tags::collision::Bsp;
use tags::pathfinding::PathfindingData;

use crate::scenario::BaseScenario;
use crate::{Report, Result, invalid};

/// `sbsp+0xc4`: the pathfinding data block (one `0x74` element).
pub const PATHFINDING: u32 = 0xc4;

/// One converted BSP, ready to write.
pub struct Prepared {
    /// Halo 1 BSP index.
    pub index: usize,
    /// Halo 1 BSP tag name, which a BSP past the base's slots takes.
    pub name: String,
    pub collision: Bsp,
    pub collision_materials: usize,
    pub sections: Vec<Section>,
    /// Per structure material, the base material it draws with.
    pub materials: Vec<usize>,
    /// Per structure material, its base material and Halo 1 shader name.
    pub picks: Vec<(usize, String)>,
    /// Halo 1's own textures on copies of the picked shaders
    /// (`h1convert::textures`).
    pub textures: h1convert::textures::Retexture,
    pub bounds: [f32; 6],
    pub mopp: Vec<u8>,
    pub checksum: u32,
    pub templates: Templates,
    /// Halo 1's lightmap bitmap, converted, and its tag name.
    pub lightmap: Option<(String, h1models::Converted)>,
    pub pathfinding: PathfindingData,
}

/// Counts for one BSP.
#[derive(Debug, Clone, Default)]
pub struct BspReport {
    pub index: usize,
    pub clusters: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub materials: usize,
    pub surfaces: usize,
    pub lightmap_pages: usize,
    pub sectors: usize,
    pub links: usize,
    pub flipped: bool,
    /// Shader copies drawing Halo 1's textures, Halo 1 bitmaps converted
    /// for them, and materials left on the base's textures (with why).
    pub textures: (usize, usize, Vec<String>),
}

impl std::fmt::Display for BspReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "bsp {}: {} cluster(s), {} vertices, {} triangles, {} materials, {} collision surfaces, \
             {} lightmap pages, pathfinding {} sectors / {} links, Halo 1 textures on {} shader \
             copies ({} bitmaps, {} notes){}",
            self.index,
            self.clusters,
            self.vertices,
            self.triangles,
            self.materials,
            self.surfaces,
            self.lightmap_pages,
            self.sectors,
            self.links,
            self.textures.0,
            self.textures.1,
            self.textures.2.len(),
            if self.flipped {
                ", winding flipped"
            } else {
                ""
            }
        )
    }
}

/// FNV-1a over the render positions: the structure checksum the BSP, its
/// lightmap and its pathfinding agree on.
fn fnv(sections: &[Section]) -> u32 {
    let mut h = 0x811c_9dc5u32;
    for s in sections {
        for v in &s.vertices {
            for f in v.position {
                for b in f.to_le_bytes() {
                    h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
                }
            }
        }
    }
    h
}

/// Prepares every Halo 1 BSP, in Halo 1's order, so `switch_bsp`, zone
/// and placement BSP indices keep their meaning. BSP `i` takes its
/// templates from the base's BSP `i`, or the base's first past its count.
pub fn prepare_all<R: Read + Seek>(
    h1: &H1Map,
    base: &mut Map<R>,
    scnr: &BaseScenario,
    report: &mut Report,
) -> Result<Vec<Prepared>> {
    let refs = h1.structure_bsps()?;
    let mut out = Vec::new();
    for (i, r) in refs.iter().enumerate() {
        let d = scnr
            .descriptors
            .get(i)
            .or(scnr.descriptors.first())
            .ok_or_else(|| invalid("the base has no BSP"))?;
        let p = prepare(h1, r, i, base, d.sbsp)?;
        report.bsps.push(BspReport {
            index: i,
            clusters: p.sections.len().max(1),
            vertices: p.sections.iter().map(|s| s.vertices.len()).sum(),
            triangles: p.sections.iter().map(|s| s.indices.len() / 3).sum(),
            materials: p.materials.len(),
            surfaces: p.collision.surfaces.len(),
            lightmap_pages: p.lightmap.as_ref().map_or(0, |_| {
                p.sections.iter().filter(|s| s.page.is_some()).count()
            }),
            sectors: p.pathfinding.sectors.len(),
            links: p.pathfinding.links.len(),
            flipped: false,
            textures: (
                p.textures.copies.len(),
                p.textures.bitmaps.len(),
                p.textures.notes.clone(),
            ),
        });
        out.push(p);
    }
    Ok(out)
}

fn prepare<R: Read + Seek>(
    h1: &H1Map,
    r: &h1map::BspRef,
    index: usize,
    base: &mut Map<R>,
    sbsp: TagId,
) -> Result<Prepared> {
    let src = h1.structure_bsp(r)?;
    let [col] = src
        .collision_bsps()?
        .try_into()
        .map_err(|_| invalid(format!("bsp {index}: not one collision BSP")))?;
    let collision = h1convert::collision::convert(&col)?;
    let largest = collision
        .surfaces
        .iter()
        .map(|s| s.material)
        .filter(|&m| m != u16::MAX)
        .max()
        .map_or(0, |m| usize::from(m) + 1);
    let collision_materials = src.collision_materials()?.len().max(largest).max(1);

    let name_of = |r: h1map::TagRef| h1.tag(r.id).map(|t| t.name.clone()).unwrap_or_default();
    let mut meshes: Vec<Mesh> = structure::meshes(&src, name_of)?;
    structure::fix_winding(&mut meshes);
    let mut index_of: BTreeMap<String, u16> = BTreeMap::new();
    let mut names = Vec::new();
    let material: Vec<u16> = meshes
        .iter()
        .map(|m| {
            *index_of.entry(m.shader.clone()).or_insert_with(|| {
                names.push(m.shader.clone());
                (names.len() - 1) as u16
            })
        })
        .collect();
    let sections = structure::pack(&meshes, &material);

    let sbsp_at = base
        .tag(sbsp)?
        .location
        .ok_or_else(|| invalid("base sbsp has no data"))?
        .address;
    let templates = Templates::from_base(&base.read_tag_bytes(sbsp)?, sbsp_at)?;
    let mut base_shaders = Vec::new();
    for i in 0..templates.material_heads.len() {
        let id = templates.shader(i);
        let name = base
            .tag(id)
            .ok()
            .and_then(|t| t.name.clone())
            .unwrap_or_default();
        let template = scenario_edit::scene::shader_bitmaps(base, id)
            .map(|(t, _)| t)
            .unwrap_or_default();
        base_shaders.push(BaseShader { name, template });
    }
    let h1_shaders: Vec<(String, String)> = names
        .iter()
        .map(|n| {
            let group = h1
                .tags()
                .iter()
                .find(|t| &t.name == n)
                .map(|t| t.group.to_string())
                .unwrap_or_default();
            (n.clone(), group)
        })
        .collect();
    let materials = if base_shaders.is_empty() {
        vec![0; names.len()]
    } else {
        shaders::pick(&h1_shaders, &base_shaders)
    };
    // Halo 1's own textures on copies of the picked shaders; a base whose
    // shaders can't be read keeps its own (with a note saying why).
    let picks: Vec<(usize, String)> = materials
        .iter()
        .copied()
        .zip(names.iter().cloned())
        .collect();
    let textures = h1convert::textures::plan(h1, base, &templates, &picks).unwrap_or_else(|e| {
        h1convert::textures::Retexture {
            notes: vec![format!("the base's textures kept: {e}")],
            ..Default::default()
        }
    });

    let pages = h1bitmaps::lightmap_pages(h1, &src).map_err(|e| invalid(e.to_string()))?;
    let lightmap = match &pages.tag {
        Some(tag) => {
            let (_, bitmap, _) =
                h1bitmaps::convert_bitmap(h1, tag).map_err(|e| invalid(e.to_string()))?;
            Some((tag.name.clone(), bitmap))
        }
        None => None,
    };
    let mopp = h1convert::convert::structure_mopp(&collision)?;
    let checksum = fnv(&sections);
    let pathfinding = pathfind::generate::generate(&collision, 0, checksum, &Default::default())
        .map_err(|e| invalid(format!("bsp {index} pathfinding: {e:?}")))?;
    Ok(Prepared {
        index,
        name: r.name.clone(),
        collision,
        collision_materials,
        sections,
        materials,
        picks,
        textures,
        bounds: src.world_bounds(),
        mopp,
        checksum,
        templates,
        lightmap,
        pathfinding,
    })
}

impl Prepared {
    /// Each cluster's centroid (the middle of its vertices' bounds), for
    /// the scenario's per-cluster data; one at the origin with none.
    pub fn centroids(&self) -> Vec<[f32; 3]> {
        if self.sections.is_empty() {
            return vec![[0.0; 3]];
        }
        self.sections
            .iter()
            .map(|s| {
                let b = s.bounds();
                [
                    (b[0] + b[1]) / 2.0,
                    (b[2] + b[3]) / 2.0,
                    (b[4] + b[5]) / 2.0,
                ]
            })
            .collect()
    }

    /// Writes the BSP and lightmap into the base's `sbsp`/`ltmp` slots,
    /// the lightmap bitmap as a new tag, and the pathfinding into the BSP.
    /// Returns the BSP section's index.
    pub fn write(&self, set: &mut TagSet, sbsp: TagId, ltmp: TagId) -> Result<usize> {
        let lightmap_bitmap = match &self.lightmap {
            Some((name, bm)) => bm
                .clone()
                .install(set, &format!("{name}_h2_{}", self.index))?,
            None => TagId::NULL,
        };
        // Halo 1's own textures: new bitmaps and shader copies (shared with
        // the level's other BSPs by name), and the material heads naming
        // them.
        let (templates, materials) = self.textures.install(set, &self.templates, &self.picks)?;
        let s = Structure {
            collision: &self.collision,
            collision_materials: self.collision_materials,
            sections: &self.sections,
            materials: &materials,
            bounds: self.bounds,
            mopp: &self.mopp,
            checksum: self.checksum,
            templates: &templates,
            lightmap_bitmap,
        };
        let section = s.write(set, sbsp, ltmp)?;
        let element = pathfind::build::element(&self.pathfinding)
            .map_err(|e| invalid(format!("bsp {} pathfinding: {e}", self.index)))?;
        let tag = set
            .tag_mut(sbsp)
            .ok_or_else(|| invalid("no sbsp in its slot"))?;
        let TagPayload::Built(root) = &mut tag.payload else {
            return Err(invalid("the converted sbsp should be built"));
        };
        root.fixups.retain(|f| f.offset != PATHFINDING);
        root.block(PATHFINDING, 1, element);
        Ok(section)
    }
}
