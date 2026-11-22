//! Single-BSP, offline split in the retail page's stored colour units.
//! All private products are sent to the caller, which owns the output directory.
mod input;
mod lamp;
mod report;
mod scene;
mod solve;

use anyhow::{Context, Result, ensure};
use assets::ResourceFiles;
use map::{Map, RawPointer};
use std::io::{Read, Seek};
use tags::{geometry::RawSource, lighting::bsp_lightmaps, structure::StructureRender};

/// Route tag resource pointers through the same bounded reader used by bitmaps.
pub struct Raw<'a>(pub &'a mut ResourceFiles);

impl RawSource for Raw<'_> {
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.0.read(p.file, p.offset, size).map_err(|e| match e {
            assets::Error::Map(e) => tags::Error::Map(e),
            assets::Error::Io(e) => tags::Error::Map(e.into()),
            assets::Error::MissingResource(f) => tags::Error::MissingResource(f),
            _ => tags::Error::Invalid("raw resource read rejected"),
        })
    }
}

// Scenery and lamps share the scenario object placement header. A manual
// BSP mask can select a different set than the identifier's origin BSP.
fn placement_in_bsp(row: &[u8], bsp: usize) -> Result<bool> {
    ensure!(row.len() > 0x30, "truncated scenario placement");
    let origin = i16::from_le_bytes([row[0x2c], row[0x2d]]);
    let mask = u16::from_le_bytes([row[0x26], row[0x27]]);
    Ok(match row[0x30] {
        0 => origin < 0 || origin as usize == bsp,
        1 => true,
        2 => bsp < 16 && mask & (1 << bsp) != 0,
        policy => anyhow::bail!("unknown placement BSP policy {policy}"),
    })
}

#[derive(Clone, Copy)]
pub enum LightId {
    Sky(usize),
    Placed(usize),
}

pub struct Outcome {
    pub summary: String,
    /// The model has a measurable contribution and all required inputs.
    /// Acceptance still requires the owner's visual judgment.
    pub reviewable: bool,
}

pub fn compare<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
    map_name: &str,
    bsp_index: usize,
    light_id: LightId,
    mut write: impl FnMut(&str, &[u8]) -> Result<()>,
) -> Result<Outcome> {
    let pairs = bsp_lightmaps(map)?;
    let &(bsp_tag, ltmp) = pairs
        .get(bsp_index)
        .with_context(|| format!("BSP {bsp_index} outside {} scenario BSPs", pairs.len()))?;
    let bsp = StructureRender::read(map, bsp_tag)?;
    let ltmp = ltmp.with_context(|| format!("BSP {bsp_index} has no lightmap"))?;
    let sky = match light_id {
        LightId::Sky(index) => Some(index),
        LightId::Placed(_) => None,
    };
    let scene = scene::Scene::load(map, files, &bsp, bsp_index, sky)?;
    let (source, description) = match light_id {
        LightId::Sky(id) => {
            let light = scene
                .light
                .as_ref()
                .context("selected sky light is missing")?;
            (
                solve::Source::sky(light.direction, light.cutoff)?,
                format!("Sky light {id} · {}", light.description),
            )
        }
        LightId::Placed(id) => {
            let lamp = lamp::PlacedLamp::load(map, id, bsp_index)?;
            let description = format!(
                "Placed lamp {id} · {} · position {:?} · radius {:.3} world units · linear radial attenuation",
                lamp.name, lamp.position, lamp.radius
            );
            (solve::Source::Placed(lamp), description)
        }
    };
    let inputs = input::load(
        map,
        files,
        &bsp,
        ltmp,
        &scene.selected_clusters,
        &scene.objects,
    )?;
    let mut report = report::Report::new(
        map_name,
        bsp_index,
        bsp.checksum,
        &description,
        scene.world_triangles,
        scene.scenery_triangles,
    );
    report.warnings.extend(scene.warnings.iter().cloned());
    report.warnings.extend(inputs.warnings);
    let visible = |p, dir, distance| scene.visible_to(p, dir, distance);
    let mut samples = solve::FitSamples::default();
    let mut traced = Vec::new();
    for page in &inputs.pages {
        for mip in &page.mips {
            let trace = solve::trace(
                mip.width as usize,
                mip.height as usize,
                &page.triangles,
                &source,
                &visible,
            )?;
            ensure!(
                mip.retail.len() == mip.width as usize * mip.height as usize
                    && mip.alpha.len() == mip.retail.len(),
                "decoded page dimensions disagree"
            );
            // Lower mip levels are filtered repetitions of this evidence.
            // Trace them independently, and use only mip zero to fit the source.
            if mip.level == 0 {
                samples.add(&trace, &mip.retail);
            }
            traced.push((page, mip, trace));
        }
    }
    let fits = [samples.fit(0), samples.fit(1)];
    drop(samples);
    let k = fits[1].map_or([0.; 3], |fit| fit.k);
    for (page, mip, trace) in traced {
        let split = solve::split_page(&trace, &mip.retail, k, 1);
        let stem = format!(
            "g{}-page{}-pal{}-mip{}",
            page.group, page.image, page.palette, mip.level
        );
        report.page(&stem, mip, &split, fits[1].is_some(), &mut write)?;
    }
    for (index, run) in inputs.vertices.iter().enumerate() {
        let split = solve::split_vertices(&run.vertices, &run.edges, &source, &visible, k, 1)?;
        report.vertices(
            index,
            &run.label,
            &run.vertices,
            &split,
            fits[1].is_some(),
            &mut write,
        )?;
    }
    report.finish(fits, &mut write)
}
