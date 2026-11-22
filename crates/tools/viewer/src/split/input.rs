//! The pages and vertex colours the structure draw actually binds. A page
//! is keyed by both image and palette. The BSP's first lightmap group owns
//! those bindings (docs/graphics/native-world.md, "What a world pass reads").
use super::{
    Raw,
    solve::{Triangle, Vertex},
};
use anyhow::{Context, Result, ensure};
use assets::ResourceFiles;
use map::{Map, TagId};
use std::{
    cell::Cell,
    collections::BTreeMap,
    io::{Read, Seek},
};
use tags::{
    DecodedTag, PixelFormat, TagData, TextureKind,
    geometry::{RawSource, Section, TagView},
    lighting::{VertexLighting, palette_rgba, vertex_lighting},
    structure::{
        InstancedGeometryInstance, Lightmap, LightmapGroup, LightmapRenderInfo, StructureRender,
        load_bucket,
    },
};

mod mesh;
mod objects;
use mesh::{ClusterLookup, append_triangles, vertex_run};

pub(super) struct Inputs {
    pub pages: Vec<Page>,
    pub vertices: Vec<VertexRun>,
    pub warnings: Vec<String>,
}

pub(super) struct Page {
    pub group: usize,
    pub image: usize,
    pub palette: usize,
    pub triangles: Vec<Triangle>,
    pub mips: Vec<Mip>,
}

pub(super) struct Mip {
    pub level: u32,
    pub width: u32,
    pub height: u32,
    pub retail: Vec<[u8; 3]>,
    pub alpha: Vec<u8>,
}

pub(super) struct VertexRun {
    pub label: String,
    pub vertices: Vec<Vertex>,
    pub edges: Vec<[usize; 2]>,
}

pub(super) struct Scenery {
    pub id: [u8; 8],
    pub checksum: u32,
    pub label: String,
    pub sections: Vec<ScenerySection>,
}

pub(super) struct ScenerySection {
    pub index: usize,
    pub opaque_count: usize,
    pub position: Vec<[f32; 3]>,
    pub triangles: Vec<[u16; 3]>,
}

#[derive(Default)]
struct Bucket {
    uv: Vec<[f32; 2]>,
    colors: Vec<(u8, Vec<[u8; 3]>)>,
    coverage: Vec<bool>,
}

impl Bucket {
    fn read(s: &Section) -> Result<Self> {
        let mut colors = Vec::new();
        for vb in &s.vertex_buffers {
            if matches!(vb.type_id, 0x2e | 0x2f)
                && let Some(VertexLighting::Color(rgb)) = vertex_lighting(s, vb)?
            {
                colors.push((
                    vb.type_id,
                    rgb.into_iter()
                        .map(|v| v.map(|c| (255.0 * c).round() as u8))
                        .collect(),
                ));
            }
        }
        let coverage = if colors.is_empty() {
            Vec::new()
        } else {
            vec![false; s.vertex_count()]
        };
        Ok(Self {
            uv: s.vertices(None)?.lightmap_texcoords,
            colors,
            coverage,
        })
    }
}

struct Mesh {
    positions: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    faces: Vec<[u16; 3]>,
}

impl Mesh {
    fn read(s: &Section) -> Result<Self> {
        let v = s.vertices(None)?;
        ensure!(
            v.positions.len() == s.vertex_count(),
            "lit section has no position stream"
        );
        let faces = s
            .parts
            .iter()
            .filter(|p| p.is_drawn() && p.kind != 4)
            .flat_map(|part| s.part_triangles(part))
            .collect();
        Ok(Self {
            positions: v.positions,
            uv: v.lightmap_texcoords,
            faces,
        })
    }
}

type PageKey = (usize, usize);

fn page_key(r: &LightmapRenderInfo) -> Result<Option<PageKey>> {
    ensure!(
        r.bitmap_index >= -1 && r.palette_index >= -1,
        "bad lightmap render info {r:?}"
    );
    Ok((r.bitmap_index >= 0 && r.palette_index >= 0)
        .then_some((r.bitmap_index as usize, r.palette_index as usize)))
}

fn mips(image: &tags::Image, palette: &[u32; 256], files: &mut ResourceFiles) -> Result<Vec<Mip>> {
    ensure!(
        image.format == PixelFormat::P8 && image.kind == TextureKind::D2,
        "expected a 2D P8 lightmap"
    );
    let chain = assets::decode_image(image, files)?;
    ensure!(
        chain.size_matches() && chain.lod == 0,
        "incomplete or mis-sized base LOD"
    );
    chain
        .levels
        .into_iter()
        .map(|level| {
            ensure!(
                level.face == 0 && level.depth == 1,
                "unexpected lightmap face or depth"
            );
            let (retail, alpha) = level
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| {
                    let [r, g, b, a] = palette_rgba(palette, p[0]);
                    ([r, g, b], a)
                })
                .unzip();
            Ok(Mip {
                level: level.level,
                width: level.width,
                height: level.height,
                retail,
                alpha,
            })
        })
        .collect()
}

pub(super) fn load<R: Read + Seek>(
    map: &mut Map<R>,
    files: &mut ResourceFiles,
    bsp: &StructureRender,
    ltmp: TagId,
    selected_clusters: &[bool],
    scenery: &[Scenery],
) -> Result<Inputs> {
    ensure!(
        selected_clusters.len() == bsp.clusters.len(),
        "sky/cluster count mismatch"
    );
    let address = map
        .tag(ltmp)?
        .location
        .context("lightmap has no payload")?
        .address;
    let bytes = map.read_tag_bytes(ltmp)?;
    let view = TagView::new(&bytes, address);
    let lightmap = Lightmap::decode(ltmp, &view)?;
    let group = lightmap
        .groups
        .first()
        .context("BSP lightmap has no group")?;
    ensure!(
        group.structure_checksum == bsp.checksum,
        "first lightmap group belongs to another BSP checksum"
    );
    let bitmap = group.bitmap.context("first lightmap group has no bitmap")?;
    let TagData::Bitmap(bitmap) = DecodedTag::read(map, bitmap)?.data else {
        anyhow::bail!("lightmap group refers to a non-bitmap");
    };
    ensure!(
        group.cluster_render_info.len() == bsp.clusters.len(),
        "cluster/render info count mismatch"
    );
    ensure!(
        group.instance_render_info.len() == bsp.instances.len(),
        "instance/render info count mismatch"
    );
    ensure!(
        group.instance_bucket_refs.is_empty()
            || group.instance_bucket_refs.len() == bsp.instances.len(),
        "instance/bucket reference count mismatch"
    );

    let mut inputs = Inputs {
        pages: Vec::new(),
        vertices: Vec::new(),
        warnings: Vec::new(),
    };
    let mut charts: BTreeMap<PageKey, Vec<Triangle>> = BTreeMap::new();
    // Include a referenced page even when every receiver is assigned to
    // another sky. Its full-size replacement then still contains retail.
    for info in group
        .cluster_render_info
        .iter()
        .chain(&group.instance_render_info)
    {
        if let Some(key) = page_key(info)? {
            charts.entry(key).or_default();
        }
    }
    let mixed_skies = selected_clusters.contains(&false) && selected_clusters.contains(&true);
    let lookup = if mixed_skies {
        Some(ClusterLookup::read(map, bsp.tag)?)
    } else {
        None
    };
    let unknown = Cell::new(0usize);
    let selected = |p: [f32; 3]| -> bool {
        let Some(lookup) = &lookup else {
            return selected_clusters.first().copied().unwrap_or(false);
        };
        match lookup.cluster(p).and_then(|i| selected_clusters.get(i)) {
            Some(&yes) => yes,
            None => {
                unknown.set(unknown.get() + 1);
                false
            }
        }
    };
    let mut raw = Raw(files);
    let mut next_chart = 0;
    for (i, c) in bsp.clusters.iter().enumerate() {
        if !selected_clusters[i] {
            continue;
        }
        let Some(key) = page_key(&group.cluster_render_info[i])? else {
            continue;
        };
        let Some(s) = c
            .geometry
            .load(&mut raw, Some(bsp.material_count as usize))?
        else {
            if c.geometry.info.total_vertex_count != 0 {
                inputs
                    .warnings
                    .push(format!("Cluster {i} has a page binding and no mesh."));
            }
            continue;
        };
        let mesh = Mesh::read(&s)?;
        if mesh.uv.is_empty() && !mesh.faces.is_empty() {
            inputs.warnings.push(format!(
                "Cluster {i} has a page binding and no lightmap UVs."
            ));
            continue;
        }
        append_triangles(
            charts.entry(key).or_default(),
            &mesh.positions,
            &mesh.uv,
            &mesh.faces,
            &mut next_chart,
        )?;
    }
    let mut buckets: Vec<_> = group
        .buckets
        .iter()
        .enumerate()
        .map(|(i, b)| -> Result<_> {
            if !b.is_patched() {
                return Ok(Bucket::default());
            }
            match load_bucket(b, &mut raw).with_context(|| format!("bucket {i}"))? {
                Some(s) => Bucket::read(&s),
                None => {
                    inputs
                        .warnings
                        .push(format!("Patched bucket {i} has no loaded section."));
                    Ok(Bucket::default())
                }
            }
        })
        .collect::<Result<_>>()?;
    append_instances(
        bsp,
        group,
        &mut raw,
        &mut buckets,
        &mut charts,
        &mut inputs,
        &mut next_chart,
        &selected,
    )?;
    objects::append_objects(
        group,
        &TagView::new(&bytes, address),
        &mut buckets,
        scenery,
        &selected,
        &mut inputs,
    )?;
    for (i, bucket) in buckets.iter().enumerate() {
        let unused = bucket.coverage.iter().filter(|&&matched| !matched).count();
        if unused > 0 {
            inputs.warnings.push(format!("Bucket {i} has {unused} colour entries with no posed receiver (unbound data or padding)."));
        }
    }
    if unknown.get() > 0 {
        inputs.warnings.push(format!("{} placement receiver samples fell outside a classified collision leaf. Their contribution stays in retail.", unknown.get()));
    }
    for ((image, palette), triangles) in charts {
        let levels = mips(
            bitmap
                .images
                .get(image)
                .context("lightmap image out of range")?,
            group
                .palettes
                .get(palette)
                .context("lightmap palette out of range")?,
            files,
        )
        .with_context(|| format!("image {image}, palette {palette}"))?;
        inputs.pages.push(Page {
            group: 0,
            image,
            palette,
            triangles,
            mips: levels,
        });
    }
    Ok(inputs)
}

/// As native_render::load::load_world and the structure draw (0x0004d830):
/// the shader receives the three rigid rows with their translation. The
/// separate scale field does not enter the uploaded model transform.
pub(super) fn placed_position(inst: &InstancedGeometryInstance, local: [f32; 3]) -> [f32; 3] {
    let rotated = inst.rotate(local);
    std::array::from_fn(|i| inst.position[i] + rotated[i])
}

#[allow(clippy::too_many_arguments)]
fn append_instances(
    bsp: &StructureRender,
    group: &LightmapGroup,
    raw: &mut dyn RawSource,
    buckets: &mut [Bucket],
    charts: &mut BTreeMap<PageKey, Vec<Triangle>>,
    inputs: &mut Inputs,
    next_chart: &mut u32,
    selected: &dyn Fn([f32; 3]) -> bool,
) -> Result<()> {
    let defs = bsp
        .instance_definitions
        .iter()
        .map(|d| {
            d.geometry
                .load(raw, Some(bsp.material_count as usize))?
                .as_ref()
                .map(Mesh::read)
                .transpose()
        })
        .collect::<Result<Vec<_>>>()?;
    let starts = objects::offsets(group);
    for (i, inst) in bsp.instances.iter().enumerate() {
        let di = inst.definition as usize;
        let Some(mesh) = defs
            .get(di)
            .context("missing instance definition")?
            .as_ref()
        else {
            if bsp.instance_definitions[di]
                .geometry
                .info
                .total_vertex_count
                != 0
            {
                inputs
                    .warnings
                    .push(format!("Instance {i} definition {di} has no mesh."));
            }
            continue;
        };
        ensure!(inst.is_rigid(), "instance {i} has non-rigid rows");
        let positions: Vec<_> = mesh
            .positions
            .iter()
            .map(|&p| placed_position(inst, p))
            .collect();
        let n = usize::from(bsp.instance_definitions[di].geometry.info.unknown_0c);
        ensure!(
            n <= positions.len(),
            "instance {i} opaque-vertex count exceeds position count"
        );
        let run = group.instance_bucket_refs.get(i).and_then(|r| {
            Some((
                usize::from(r.bucket),
                usize::from((*r.section_offsets.first()?)?),
            ))
        });
        // Clip to the definition's opaque count: six observed placements
        // overrun their bucket when the transparent tail is included.
        let mut uv = mesh.uv.as_slice();
        if let Some((bucket, first)) = run {
            let b = buckets.get_mut(bucket).context("missing instance bucket")?;
            let end = first + n;
            ensure!(
                starts[bucket]
                    .range((first + 1)..)
                    .next()
                    .is_none_or(|&next| end <= next),
                "instance {i} overlaps the next run in bucket {bucket}"
            );
            if !b.uv.is_empty() {
                uv = b
                    .uv
                    .get(first..end)
                    .with_context(|| format!("instance {i} UV range outside bucket {bucket}"))?;
            }
            for (stream, colors) in &b.colors {
                let rgb = colors.get(first..end).with_context(|| {
                    format!("instance {i} colour range outside bucket {bucket}")
                })?;
                if !rgb.is_empty() {
                    inputs.vertices.push(vertex_run(
                        format!("Instance {i} / bucket {bucket}, {stream:02x}, first {first}"),
                        &positions[..n],
                        &mesh.faces,
                        rgb,
                        selected,
                    )?);
                    b.coverage[first..end].fill(true);
                }
            }
        }
        let Some(key) = page_key(&group.instance_render_info[i])? else {
            continue;
        };
        if uv.is_empty() {
            if !mesh.faces.is_empty() {
                inputs.warnings.push(format!(
                    "Instance {i} has a page binding and no lightmap UVs."
                ));
            }
            continue;
        }
        // The chosen sky acts on placement faces in its clusters. One
        // centroid query per face also handles pieces spanning clusters.
        let faces = mesh
            .faces
            .iter()
            .copied()
            .filter(|t| {
                let [a, b, c] = t.map(|j| positions[usize::from(j)]);
                selected(std::array::from_fn(|j| (a[j] + b[j] + c[j]) / 3.0))
            })
            .collect::<Vec<_>>();
        append_triangles(
            charts.entry(key).or_default(),
            &positions,
            uv,
            &faces,
            next_chart,
        )
        .with_context(|| format!("instance {i}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use map::{RawPointer, ResourceFile};
    use tags::{
        records::BitmapImage,
        wire::{BinaryRecord, Flags16},
    };

    #[test]
    fn placements_apply_the_draw_rows_without_the_separate_scale() {
        let inst = InstancedGeometryInstance {
            scale: 7.0,
            basis: [[0., 1., 0.], [-1., 0., 0.], [0., 0., 1.]],
            position: [7., 8., 9.],
            definition: 0,
            raw: Vec::new(),
        };
        assert_eq!(placed_position(&inst, [1., 2., 3.]), [5., 9., 12.]);
    }

    #[test]
    fn expands_each_stored_mip_with_the_surfaces_own_palette() {
        let mut wire = BitmapImage::decode(&[0; BitmapImage::SIZE]).unwrap();
        wire.flags = Flags16(8);
        let image = tags::Image {
            wire,
            format: PixelFormat::P8,
            kind: TextureKind::D2,
            dimensions: [2, 2, 1],
            mip_count: 1,
            low_detail: None,
            lods: [
                Some(tags::ResourceRange {
                    pointer: RawPointer {
                        file: ResourceFile::Local,
                        offset: 0,
                    },
                    size: 128,
                }),
                None,
                None,
            ],
        };
        let mut files = ResourceFiles::new();
        let mut bytes = vec![0; 128];
        bytes[..5].copy_from_slice(&[0, 1, 1, 0, 2]);
        files.set_bytes(ResourceFile::Local, bytes.into());
        let mut palette = [0; 256];
        palette[1] = 0xa1b2_c3d4;
        palette[2] = 0x1020_3040;
        let levels = mips(&image, &palette, &mut files).unwrap();
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].retail[1], [0xb2, 0xc3, 0xd4]);
        assert_eq!(levels[0].alpha[1], 0xa1);
        assert_eq!(
            (levels[1].level, levels[1].width, levels[1].height),
            (1, 1, 1)
        );
        assert_eq!(levels[1].retail, [[0x20, 0x30, 0x40]]);
        assert_eq!(levels[1].alpha, [0x10]);
        palette[2] = 0xff00_ff00;
        assert_eq!(
            mips(&image, &palette, &mut files).unwrap()[1].retail,
            [[0, 255, 0]]
        );
    }
}
