//! Each Halo 1 structure BSP of a level, in Halo 1's order, converted into
//! the base's BSP slot of the same index or, past the base's count, new
//! `sbsp`/`ltmp` tags under Halo 1's name: `h1convert`'s collision, render geometry, materials, MOPP
//! and lightmap (docs/halo1/multiplayer.md, "The converted structure
//! BSP"), plus AI pathfinding generated from the collision
//! (`pathfind::generate`, docs/formats/pathfinding.md) at `sbsp+0xc4`.

use std::collections::BTreeMap;
use std::io::{Read, Seek};

use h1convert::structure::{self, Mesh, Section, Structure, Templates};
use h1map::H1Map;
use map::build::{TagPayload, TagSet};
use map::{Map, TagId};
use tags::collision::Bsp;
use tags::pathfinding::PathfindingData;

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
    /// The cluster lightmap pages cut from Halo 1's lightmap bitmap
    /// (`h1convert::clusters::lightmap_bitmap`), and its tag name.
    pub lightmap: Option<(String, h1models::Converted)>,
    pub pathfinding: PathfindingData,
    /// Per cluster, its background sound and sound environment (indices
    /// into the level's palettes, -1 none), and per collision leaf its
    /// cluster (`h1convert::clusters::sound_zones`).
    pub sound_clusters: Vec<[i16; 2]>,
    pub sound_leaves: Vec<u16>,
    /// Halo 1's portals and each cluster's list of them
    /// (`h1convert::clusters::portals`).
    pub visibility: h1convert::clusters::Visibility,
    /// Per cluster, whether it names no sky: Halo 1 fogs those with the
    /// level's indoor fog (`h1convert::clusters::skyless`).
    pub skyless: Vec<bool>,
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
    /// Halo 1's portals, kept (`h1convert::clusters::portals`).
    pub portals: usize,
}

impl std::fmt::Display for BspReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "bsp {}: {} cluster(s), {} vertices, {} triangles, {} materials, {} collision surfaces, \
             {} lightmap pages, pathfinding {} sectors / {} links, Halo 1 textures on {} shader \
             copies ({} bitmaps, {} notes), {} portals{}",
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
            self.portals,
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
/// and placement BSP indices keep their meaning.
pub fn prepare_all<R: Read + Seek>(
    h1: &H1Map,
    base: &mut Map<R>,
    ambience: &h1convert::ambience::Ambience,
    report: &mut Report,
) -> Result<Vec<Prepared>> {
    let refs = h1.structure_bsps()?;
    let mut out = Vec::new();
    for (i, r) in refs.iter().enumerate() {
        let p = prepare(h1, r, i, base, ambience)?;
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
            portals: p.visibility.portals.len(),
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
    ambience: &h1convert::ambience::Ambience,
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
    let collision_materials = src.collision_materials()?.len();
    if largest > collision_materials {
        return Err(invalid("surface outside source collision material palette"));
    }

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
    let laid = h1convert::clusters::lay_out(h1, &src, &mut meshes, &material, col.planes.len())?;
    let sections = laid.sections;
    // Ambient sound: the scenario's palettes are rebuilt, so every entry
    // up to the engine's bound is written (`scenario::write`).
    let written = h1convert::ambience::written(ambience);
    let (sound_clusters, sound_leaves) =
        h1convert::clusters::sound_zones(&src, collision.leaves.len(), |z| {
            ambience.index(index, z, written)
        })?;

    let templates = Templates::default();
    let picks: Vec<_> = names.into_iter().enumerate().collect();
    let materials = (0..picks.len()).collect();
    let textures = h1convert::textures::plan_bsp(h1, base, &picks, &src.collision_materials()?)?;

    let skyless = h1convert::clusters::skyless(&src)?;
    let lightmap = h1convert::clusters::lightmap_bitmap(h1, &src, &laid.pages)?
        .map(|(name, _, bitmap)| (name, bitmap));
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
        sound_clusters,
        visibility: laid.visibility,
        sound_leaves,
        skyless,
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

    /// Each cluster's atmosphere (`+0x6f`): `indoor` on the skyless ones,
    /// none (`0xff`) on the rest.
    pub fn atmosphere(&self, indoor: Option<u8>) -> Vec<u8> {
        self.skyless
            .iter()
            .map(|&s| indoor.filter(|_| s).unwrap_or(0xff))
            .collect()
    }

    /// Writes the BSP and lightmap into the base's `sbsp`/`ltmp` slots,
    /// the lightmap bitmap as a new tag, and the pathfinding into the BSP.
    /// `indoor` is the scenario's atmosphere element holding the level's
    /// indoor fog, which every skyless cluster names (`None`: there is
    /// none, and every cluster takes its sky's fog). Returns the BSP
    /// section's index.
    pub fn write(
        &self,
        set: &mut TagSet,
        sbsp: TagId,
        ltmp: TagId,
        sounds: &h1convert::ambience::Installed,
        indoor: Option<u8>,
    ) -> Result<usize> {
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
        let sound = h1convert::structure::Sound {
            backgrounds: sounds.backgrounds.clone(),
            environments: sounds.environments.clone(),
            clusters: self.sound_clusters.clone(),
            leaves: self.sound_leaves.clone(),
        };
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
            sound: &sound,
            visibility: &self.visibility,
            atmosphere: &self.atmosphere(indoor),
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
