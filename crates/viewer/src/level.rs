//! Structure BSP geometry from a scenario, shared by the `bsp` and
//! `shadowlab` binaries. Reads the user's own disc; nothing is copied.
use crate::disc::{Disc, DiscResources};
use anyhow::{Context, Result};
use map::{Map, TagId};
use std::fs::File;
use std::io::BufReader;
use tags::geometry::{Section, TagView};
use tags::structure::StructureRender;

pub type MapReader = Map<BufReader<File>>;

/// (bsp, lightmap) pairs from the scenario's BSP block (+0x210, stride 0x44).
pub fn scenario_bsps(map: &mut MapReader) -> Result<Vec<(TagId, Option<TagId>)>> {
    let scenario = map.scenario().context("no scenario")?.clone();
    let location = scenario.location.context("scenario has no payload")?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    let mut out = Vec::new();
    for b in view.block(view.at(0, 0x218)?, 0x210, 0x44)? {
        let word = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        let lightmap = word(0x1c);
        out.push((
            TagId(word(0x14)),
            (lightmap != u32::MAX).then_some(TagId(lightmap)),
        ));
    }
    Ok(out)
}

/// A triangle as three world-space positions.
#[allow(dead_code)]
pub type Tri = [[f32; 3]; 3];

/// One BSP's drawn geometry: the clusters and placed instances as world
/// triangles, and each instance definition on its own (in its local space)
/// for per-piece statistics.
#[allow(dead_code)]
pub struct Level {
    pub clusters: Vec<Tri>,
    pub instances: Vec<Tri>,
    pub definitions: Vec<Vec<Tri>>,
    /// Placements per definition.
    pub placements: Vec<usize>,
}

#[allow(dead_code)]
fn section_tris(s: &Section, f: &dyn Fn([f32; 3]) -> [f32; 3], out: &mut Vec<Tri>) -> Result<()> {
    let p = s.vertices(None)?.positions;
    for part in s.parts.iter().filter(|p| p.is_drawn()) {
        for t in s.part_triangles(part) {
            out.push(t.map(|i| f(p[usize::from(i)])));
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn load_level(disc: &Disc, name: &str, bsp_index: usize) -> Result<Level> {
    let mut map = disc.open_map(name)?;
    let mut raw: DiscResources = disc.resources(name)?;
    let bsps = scenario_bsps(&mut map)?;
    let (bsp_id, _) = *bsps
        .get(bsp_index)
        .with_context(|| format!("{name} has {} BSPs, no index {bsp_index}", bsps.len()))?;
    let bsp = StructureRender::read(&mut map, bsp_id)?;
    let materials = Some(bsp.material_count as usize);
    let mut level = Level {
        clusters: Vec::new(),
        instances: Vec::new(),
        definitions: Vec::new(),
        placements: vec![0; bsp.instance_definitions.len()],
    };
    for c in &bsp.clusters {
        if let Some(s) = c.geometry.load(&mut raw, materials)? {
            section_tris(&s, &|v| v, &mut level.clusters)?;
        }
    }
    let defs: Vec<Option<Section>> = bsp
        .instance_definitions
        .iter()
        .map(|d| d.geometry.load(&mut raw, materials))
        .collect::<Result<_, _>>()?;
    for d in &defs {
        let mut tris = Vec::new();
        if let Some(s) = d {
            section_tris(s, &|v| v, &mut tris)?;
        }
        level.definitions.push(tris);
    }
    for i in &bsp.instances {
        let def = i.definition as usize;
        if let Some(Some(s)) = defs.get(def) {
            level.placements[def] += 1;
            section_tris(s, &|v| i.transform(v), &mut level.instances)?;
        }
    }
    Ok(level)
}
