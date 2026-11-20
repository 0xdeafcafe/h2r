//! Static casters and the selected BSP's lighting sky. All data lives for
//! one invocation, in the map's own coordinates.
mod bvh;
mod mask;

use super::{
    Raw,
    input::{Scenery, ScenerySection, placed_position},
};
use anyhow::{Context, Result, bail, ensure};
use assets::ResourceFiles;
use bvh::{Bvh, Triangle};
use h1_assets::sky::h2::Sky;
use map::{Map, TagId};
use mask::{Coverage, Materials};
use scenario_edit::{
    BlockKind, Scenario,
    geometry::{Vec3, bsp_shaders, markers},
    scene::{model_to_world, skies_in},
};
use std::{
    collections::{BTreeSet, HashMap},
    io::{Read, Seek},
};
use tags::{
    geometry::{Section, TagView, Vertices},
    model::RenderModelGeometry,
    structure::StructureRender,
    wire::BinaryRecord,
};

pub(super) struct Light {
    pub direction: Vec3,
    pub cutoff: f32,
    pub description: String,
}

pub(super) struct Scene {
    pub light: Option<Light>,
    pub selected_clusters: Vec<bool>,
    pub objects: Vec<Scenery>,
    pub warnings: Vec<String>,
    pub world_triangles: usize,
    pub scenery_triangles: usize,
    world: Bvh,
    scenery: Bvh,
    coverage: Vec<Coverage>,
}

impl Scene {
    pub fn load<R: Read + Seek>(
        map: &mut Map<R>,
        files: &mut ResourceFiles,
        bsp: &StructureRender,
        bsp_index: usize,
        light: Option<usize>,
    ) -> Result<Self> {
        let (light, selected_clusters) = match light {
            Some(id) => {
                let (light, selected) = select_light(map, bsp, bsp_index, id)?;
                (Some(light), selected)
            }
            None => (None, vec![true; bsp.clusters.len()]),
        };
        let mut materials = Materials::new(map)?;
        let world = world_geometry(map, files, bsp, &mut materials)?;
        let mut scenery = Vec::new();
        let objects = scenery_geometry(map, files, bsp_index, &mut materials, &mut scenery)?;
        Ok(Self {
            light,
            selected_clusters,
            objects,
            world_triangles: world.len(),
            scenery_triangles: scenery.len(),
            warnings: materials.warnings,
            coverage: materials.coverage,
            world: Bvh::new(world),
            scenery: Bvh::new(scenery),
        })
    }

    /// Shadow rays are two-sided. The endpoint is exclusive for finite lamps.
    pub fn visible_to(&self, origin: Vec3, dir: Vec3, max_distance: f32) -> [bool; 2] {
        if self
            .world
            .blocked(origin, dir, max_distance, &self.coverage)
        {
            return [false, false];
        }
        [
            true,
            !self
                .scenery
                .blocked(origin, dir, max_distance, &self.coverage),
        ]
    }
}

// The object-light sky ray (docs/formats/lighting.md, 0x000d1a30) uses
// the signed spherical direction and a positive displacement. The sky
// model's rotation belongs to the flare/display path.
fn receiver_to_sky(yaw: f32, pitch: f32) -> Vec3 {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    [cp * cy, cp * sy, sp]
}

// Lighting selection (0x00130f60), including the BSP fallback permission.
// The camera's draw-sky fallback at 0x0014b360 is a different selector.
fn lighting_sky(
    first: i8,
    drawn: i8,
    bsp: Option<usize>,
    sky_flags: &[Option<u32>],
) -> Option<usize> {
    if first != -1
        && let Ok(i) = usize::try_from(first)
        && i < sky_flags.len()
    {
        return sky_flags[i].map(|_| i);
    }
    if first == -1
        && let Ok(i) = usize::try_from(drawn)
        && sky_flags
            .get(i)
            .copied()
            .flatten()
            .is_some_and(|flags| flags & 16 == 0)
    {
        return Some(i);
    }
    bsp.filter(|&i| sky_flags.get(i).is_some_and(Option::is_some))
}

fn select_light<R: Read + Seek>(
    map: &mut Map<R>,
    bsp: &StructureRender,
    bsp_index: usize,
    index: usize,
) -> Result<(Light, Vec<bool>)> {
    let tag = map.scenario().context("no scenario")?.clone();
    let address = tag.location.context("no scenario payload")?.address;
    let bytes = map.read_tag_bytes(tag.id)?;
    let view = TagView::new(&bytes, address);
    let header = view.at(0, 0x218)?;
    let scenario_skies = skies_in(&bytes, address);
    let bsp_refs = view.block(header, 0x210, 0x44)?;
    let entry = bsp_refs.get(bsp_index).context("no BSP entry")?;
    let bsp_sky = usize::try_from(i16::from_le_bytes([entry[0x40], entry[0x41]]))
        .ok()
        .filter(|_| entry[0x3c] & 1 != 0);
    let mut skies = Vec::new();
    for &id in &scenario_skies {
        skies.push(if id == TagId::NULL {
            None
        } else {
            Some(Sky::read_from_map(map, id).with_context(|| format!("read sky {:#010x}", id.0))?)
        });
    }
    let sky_flags: Vec<_> = skies
        .iter()
        .map(|sky| sky.as_ref().map(|s| s.word_10))
        .collect();
    let cluster_skies: Vec<_> = bsp
        .clusters
        .iter()
        .map(|c| lighting_sky(c.raw[0x6c] as i8, c.raw[0x6e] as i8, bsp_sky, &sky_flags))
        .collect();
    let mut count = 0;
    for (sky_index, sky) in skies.iter().enumerate() {
        if !cluster_skies.contains(&Some(sky_index)) {
            continue;
        }
        let Some(sky) = sky else {
            continue;
        };
        for (within, light) in sky.lights.iter().enumerate() {
            if count == index {
                let params = light
                    .params
                    .as_ref()
                    .context("sky light has no parameters")?;
                ensure!(
                    light.yaw.is_finite()
                        && light.pitch.is_finite()
                        && params.cutoff_angle.is_finite(),
                    "nonfinite sky light"
                );
                let name = map
                    .tag(scenario_skies[sky_index])?
                    .name
                    .as_deref()
                    .unwrap_or("unnamed sky");
                let direction = receiver_to_sky(light.yaw, light.pitch);
                // The tag's cutoff is not identified as the sun's disc size.
                let cutoff = 0.27f32.to_radians();
                let description = format!(
                    "{name}, sky {sky_index}, record {within}, direction {direction:.4?} (lighting record), assumed disc radius 0.27°, authored cutoff {:.2}°",
                    params.cutoff_angle.to_degrees()
                );
                return Ok((
                    Light {
                        direction,
                        cutoff,
                        description,
                    },
                    cluster_skies
                        .iter()
                        .map(|&s| s == Some(sky_index))
                        .collect(),
                ));
            }
            count += 1;
        }
    }
    bail!("BSP {bsp_index} has {count} sky lights; requested zero-based ID {index}")
}

fn world_geometry<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
    bsp: &StructureRender,
    materials: &mut Materials,
) -> Result<Vec<Triangle>> {
    let shaders = bsp_shaders(map, bsp.tag)?;
    let mut world = Vec::new();
    for c in &bsp.clusters {
        if let Some(section) = c.geometry.load(&mut Raw(files), Some(shaders.len()))? {
            let v = section.vertices(None)?;
            add_section(
                map,
                files,
                materials,
                &section,
                &v,
                &shaders,
                false,
                &v.positions,
                &mut world,
            )?;
        }
    }
    let defs = bsp
        .instance_definitions
        .iter()
        .map(|d| {
            let Some(section) = d.geometry.load(&mut Raw(files), Some(shaders.len()))? else {
                return Ok(None);
            };
            let v = section.vertices(None)?;
            Ok(Some((section, v)))
        })
        .collect::<Result<Vec<_>>>()?;
    for (i, instance) in bsp.instances.iter().enumerate() {
        let def = usize::try_from(instance.definition).context("negative placement definition")?;
        let Some((section, v)) = defs
            .get(def)
            .with_context(|| format!("placement {i} has no definition {def}"))?
        else {
            continue;
        };
        // Match native_render::load's displayed instance transform (no scale field).
        let positions = v
            .positions
            .iter()
            .map(|&p| placed_position(instance, p))
            .collect::<Vec<_>>();
        add_section(
            map, files, materials, section, v, &shaders, false, &positions, &mut world,
        )?;
    }
    Ok(world)
}

#[allow(clippy::too_many_arguments)]
fn add_section<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
    materials: &mut Materials,
    s: &Section,
    v: &Vertices,
    shaders: &[TagId],
    object: bool,
    positions: &[Vec3],
    out: &mut Vec<Triangle>,
) -> Result<()> {
    for part in s.parts.iter().filter(|p| p.is_drawn()) {
        let shader = shaders
            .get(usize::from(part.material))
            .copied()
            .unwrap_or(TagId::NULL);
        let material = materials.index(
            map,
            files,
            shader,
            object,
            part.kind == scenario_edit::geometry::PART_TRANSPARENT,
        );
        match &materials.coverage[material] {
            Coverage::Empty => continue,
            Coverage::Mask(_) => ensure!(
                v.texcoords.len() == positions.len(),
                "masked part of {shader:?} has no material UV stream"
            ),
            Coverage::Solid => (),
        }
        for tri in s.part_triangles(part) {
            let indices = tri.map(usize::from);
            ensure!(
                indices.iter().all(|&i| i < positions.len()),
                "caster triangle has an out-of-bounds vertex"
            );
            out.push(Triangle {
                positions: indices.map(|i| positions[i]),
                uv: indices.map(|i| v.texcoords.get(i).copied().unwrap_or([0., 0.])),
                material,
            });
        }
    }
    Ok(())
}

// Select from the render model's regions by the object model variant's
// authored region and permutation names. The lightmap does not identify
// the outcome of a random permutation or a runtime animation.
fn caster_sections(
    model: &RenderModelGeometry,
    selection: &tags::object::ResolvedModel,
    warnings: &mut Vec<String>,
) -> BTreeSet<usize> {
    let name = model.name.as_deref().unwrap_or("unnamed scenery");
    if matches!(selection.variant, tags::object::VariantChoice::Missing(_)) {
        warnings.push(format!(
            "{name}: requested scenery variant missing, using default permutations."
        ));
    }
    if !selection.objects.is_empty() {
        warnings.push(format!("{name}: variant-attached child objects have no placement of their own and are absent from the scenery trace."));
    }
    if selection.animation.is_some() {
        warnings.push(format!(
            "{name}: scenery with an animation graph is traced in the mode's bind pose."
        ));
    }
    let mut out = BTreeSet::new();
    for region in &model.regions {
        let choices = selection
            .regions
            .iter()
            .find(|r| r.region.is_some() && r.region == region.name);
        let permutation = choices
            .and_then(|r| {
                if r.permutations.len() > 1 {
                    warnings.push(format!(
                        "{name}, region {:?}: using highest-probability scenery permutation.",
                        region.name
                    ));
                }
                let selected = r
                    .permutations
                    .iter()
                    .max_by(|a, b| a.probability.total_cmp(&b.probability))?;
                let name = selected.name.as_deref()?;
                let perm = region
                    .permutations
                    .iter()
                    .find(|p| p.name.as_deref() == Some(name));
                if perm.is_none() {
                    warnings.push(format!(
                        "{name}: missing scenery permutation in region {:?}.",
                        region.name
                    ));
                }
                perm
            })
            .or_else(|| region.permutations.first());
        if let Some(index) = permutation.and_then(|p| p.best_section()) {
            out.insert(index);
        }
    }
    out
}

fn scenery_geometry<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
    bsp_index: usize,
    materials: &mut Materials,
    tris: &mut Vec<Triangle>,
) -> Result<Vec<Scenery>> {
    let scenario = Scenario::read(map)?;
    let markers = markers(&scenario);
    let block = scenario.block(BlockKind::Scenery);
    let mut objects = Vec::new();
    if block.is_empty() {
        return Ok(objects);
    }
    let ctx = tags::object::Context::read(map)?;
    let mut models = HashMap::new();
    let mut modes = HashMap::new();
    for m in markers.iter().filter(|m| m.block == BlockKind::Scenery) {
        let row = block
            .entry(m.index)
            .context("scenery placement outside block")?;
        let id: [u8; 8] = row[0x28..0x30].try_into()?;
        let in_bsp = super::placement_in_bsp(row, bsp_index)
            .with_context(|| format!("scenery placement {}", m.index))?;
        if !in_bsp || row[4] & (1 << 6) != 0 {
            continue;
        }
        if row[0x24] & 1 != 0 {
            materials.warnings.push(format!("Scenery {} has a mirrored placement: excluded because its mirror axis is unverified.",m.index));
            continue;
        }
        let Some(tag) = m.object else {
            continue;
        };
        let variant = u32::from_le_bytes(row[0x34..0x38].try_into()?);
        let key = (tag, variant);
        if let std::collections::hash_map::Entry::Vacant(entry) = modes.entry(key) {
            let resolved = ctx
                .resolve_object(tag)
                .with_context(|| format!("scenery placement {}, object {tag:?}", m.index))?;
            let Some(mut selection) = resolved.model else {
                entry.insert(None);
                continue;
            };
            if variant != 0 {
                selection = ctx.resolve_model(selection.id, tags::wire::StringId(variant))?;
            }
            let Some(mode) = selection.render_model.as_ref().map(|m| m.id) else {
                entry.insert(None);
                continue;
            };
            if let std::collections::hash_map::Entry::Vacant(entry) = models.entry(mode) {
                entry.insert(
                    RenderModelGeometry::read_in(map, mode, &mut Raw(files), Some(&ctx.meta))
                        .with_context(|| {
                            format!("scenery placement {}, render model {mode:?}", m.index)
                        })?,
                );
            }
            let sections = caster_sections(&models[&mode], &selection, &mut materials.warnings);
            entry.insert(Some((mode, sections)));
        }
        let Some((mode, casters)) = &modes[&key] else {
            continue;
        };
        let model = &models[mode];
        let mut header = [0u8; tags::records::ModeGeometryHeader::SIZE];
        model.header.encode_into(&mut header)?;
        let checksum = u32::from_le_bytes(header[8..12].try_into()?);
        let shaders: Vec<_> = model.materials.iter().map(|m| m.shader.id).collect();
        let label = format!(
            "scenery {} ({})",
            m.index,
            model.name.as_deref().unwrap_or("unnamed")
        );
        let mut sections = Vec::new();
        for (index, s) in model.sections.iter().enumerate() {
            let position: Vec<_> = s
                .vertices
                .positions
                .iter()
                .map(|&p| model_to_world(m, p))
                .collect();
            if casters.contains(&index) {
                add_section(
                    map,
                    files,
                    materials,
                    &s.geometry,
                    &s.vertices,
                    &shaders,
                    true,
                    &position,
                    tris,
                )?;
            }
            let triangles = s
                .geometry
                .parts
                .iter()
                .filter(|p| p.is_drawn() && p.kind != scenario_edit::geometry::PART_TRANSPARENT)
                .flat_map(|p| s.geometry.part_triangles(p))
                .collect();
            let mut record = [0; tags::records::ModeSection::SIZE];
            s.wire.encode_into(&mut record)?;
            let opaque_count = usize::from(u16::from_le_bytes([record[0x10], record[0x11]]));
            sections.push(ScenerySection {
                index,
                position,
                triangles,
                opaque_count,
            });
        }
        objects.push(Scenery {
            id,
            label,
            sections,
            checksum,
        });
    }
    Ok(objects)
}

#[cfg(test)]
pub(super) fn test_roof(x_edge: f32, z: f32) -> impl Fn(Vec3, Vec3, f32) -> [bool; 2] {
    let corners = [
        [-10., -10., z],
        [x_edge, -10., z],
        [x_edge, 10., z],
        [-10., 10., z],
    ];
    let triangles = [[0, 1, 2], [2, 3, 0]].map(|indices| Triangle {
        positions: indices.map(|i| corners[i]),
        uv: [[0.; 2]; 3],
        material: 0,
    });
    let scene = Scene {
        world: Bvh::new(vec![]),
        scenery: Bvh::new(Vec::from(triangles)),
        coverage: vec![Coverage::Solid],
        light: None,
        selected_clusters: vec![],
        objects: vec![],
        warnings: vec![],
        world_triangles: 0,
        scenery_triangles: 2,
    };
    move |origin, dir, limit| scene.visible_to(origin, dir, limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lighting_skies_keep_the_signed_direction_and_use_only_permitted_fallbacks() {
        let d = receiver_to_sky(std::f32::consts::FRAC_PI_2, -std::f32::consts::FRAC_PI_4);
        assert!(d[0].abs() < 1e-6 && d[1] > 0.70 && d[2] < -0.70);
        let flags = [Some(16), Some(0), None];
        assert_eq!(lighting_sky(0, 1, None, &flags), Some(0));
        assert_eq!(lighting_sky(-1, 0, Some(1), &flags), Some(1));
        assert_eq!(lighting_sky(-1, 0, None, &flags), None);
        assert_eq!(lighting_sky(-1, 1, None, &flags), Some(1));
        assert_eq!(lighting_sky(-1, -1, Some(1), &flags), Some(1));
        assert_eq!(lighting_sky(2, -1, Some(1), &flags), None);
    }

    #[test]
    fn shadow_rays_cross_cutouts_and_end_at_the_light() {
        let panel = |x: f32, z: f32, material| Triangle {
            positions: [[x, 0., z], [x + 1., 0., z], [x, 1., z]],
            uv: [[0., 0.], [1., 0.], [0., 1.]],
            material,
        };
        // More than one leaf. The first world triangle is a cutout.
        let world = Bvh::new(
            (0..12)
                .map(|i| panel(i as f32, 1., if i == 0 { 1 } else { 0 }))
                .collect(),
        );
        let scene = Scene {
            world,
            scenery: Bvh::new(vec![panel(0., 2., 0)]),
            coverage: vec![Coverage::Solid, mask::invented_cutout()],
            world_triangles: 12,
            scenery_triangles: 1,
            light: None,
            selected_clusters: vec![],
            objects: vec![],
            warnings: vec![],
        };
        let up = [0., 0., 1.];
        assert_eq!(
            scene.visible_to([0.1, 0.1, 0.], up, 2.),
            [true, true],
            "finite segment ends on the scenery"
        );
        assert_eq!(
            scene.visible_to([0.1, 0.1, 0.], up, 3.),
            [true, false],
            "only scenery blocks through the cutout"
        );
        assert_eq!(
            scene.visible_to([0.75, 0.1, 0.], up, 3.),
            [false, false],
            "opaque texel of the same triangle"
        );
        assert_eq!(
            scene.visible_to([11.1, 0.1, 0.], up, f32::INFINITY),
            [false, false],
            "far BVH leaf"
        );
        assert_eq!(
            scene.visible_to([11.1, 0.1, 2.], [0., 0., -1.], 3.),
            [false, false],
            "back face also occludes"
        );
    }
}

/// Geometric shadow caster for the forward model's independent area fixture.
