//! Static measurements for the lightmap split. All coordinates are full-resolution
//! page coordinates and retail world units. No pixel data is retained.
use super::{Cache, Raw};
use anyhow::{Context, Result, ensure};
use assets::ResourceFiles;
use map::TagId;
use std::collections::{BTreeMap, BTreeSet};
use tags::{
    DecodedTag, TagData,
    geometry::{RawSource, Section, unpack_normpacked3},
    lighting::{Atmosphere, Sky},
    structure::{
        GeometryHeader, InstancedGeometryInstance, Lightmap, LightmapGroup, LightmapRenderInfo,
        StructureRender, load_bucket,
    },
};

type Vec3 = [f32; 3];
type Uv = [f32; 2];
type Size = [u16; 2];

struct Sun {
    direction: Vec3,
    flags: Option<u32>,
}

fn suns(sky: Sky) -> Vec<Sun> {
    sky.lights
        .into_iter()
        .map(|light| {
            // The object-lighting query at 0x000d1b1c forms this vector from
            // the authored angles, independently of the sky model's rotation.
            let [yaw, pitch] = light.yaw_pitch;
            let (sy, cy) = yaw.sin_cos();
            let (sp, cp) = pitch.sin_cos();
            Sun {
                direction: [cp * cy, cp * sy, sp],
                flags: light.parameter_flags,
            }
        })
        .collect()
}

struct Skies {
    palette: Vec<Option<TagId>>,
    by_id: BTreeMap<u32, Vec<Sun>>,
    default_first: bool,
}

impl Skies {
    fn read(map: &mut Cache) -> Result<Self> {
        let ids = crate::live_sky::scenario_skies(map)?;
        let location = map
            .scenario()
            .and_then(|s| s.location)
            .context("no scenario header")?;
        ensure!(location.size >= 0x13, "short scenario header");
        let flags = map.read_file_range(location.file_offset()? + 0x12, 1)?[0];
        let palette: Vec<_> = ids
            .into_iter()
            .map(|id| (id != TagId::NULL).then_some(id))
            .collect();
        let mut by_id = BTreeMap::new();
        for id in palette
            .iter()
            .flatten()
            .map(|id| id.0)
            .collect::<BTreeSet<_>>()
        {
            let Atmosphere::Sky(sky) = Atmosphere::read(map, TagId(id))? else {
                anyhow::bail!("scenario sky {id} has wrong class");
            };
            by_id.insert(id, suns(sky));
        }
        Ok(Self {
            palette,
            by_id,
            default_first: flags & 2 != 0,
        })
    }

    fn cluster(&self, index: i8) -> Option<TagId> {
        // 0x0014b38d substitutes sky zero for -1 only with the scenario
        // flag. Out-of-range indices and null palette entries select no sky.
        let i = if index == -1 && self.default_first {
            0
        } else {
            usize::try_from(index).ok()?
        };
        self.palette.get(i).copied().flatten()
    }
}

#[derive(Default)]
struct Streams {
    uv: Vec<Uv>,
    incident: Vec<Vec3>,
}

fn incident(section: &Section) -> Result<Vec<Vec3>> {
    let Some(vb) = section.vertex_buffers.iter().find(|b| b.type_id == 0x30) else {
        return Ok(Vec::new());
    };
    ensure!(vb.stride == 4, "invalid incident stream stride");
    Ok(section
        .data(vb)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|&b| unpack_normpacked3(u32::from_le_bytes(b)))
        .collect())
}

struct Mesh {
    positions: Vec<Vec3>,
    opaque: Vec<[u16; 3]>,
    own: Streams,
}

impl Mesh {
    fn load(
        header: &GeometryHeader,
        raw: &mut dyn RawSource,
        materials: usize,
    ) -> Result<Option<Self>> {
        let Some(section) = header.load(raw, Some(materials))? else {
            return Ok(None);
        };
        let vertices = section.vertices(None)?;
        let opaque: Vec<_> = section
            .parts
            .iter()
            .filter(|p| p.is_drawn() && p.kind != 4)
            .flat_map(|p| section.part_triangles(p))
            .collect();
        ensure!(
            opaque.is_empty() || vertices.positions.len() == section.vertex_count(),
            "opaque geometry has no position stream"
        );
        Ok(Some(Self {
            positions: vertices.positions,
            opaque,
            own: Streams {
                uv: vertices.lightmap_texcoords,
                incident: incident(&section)?,
            },
        }))
    }
}

fn bucket_streams(g: &LightmapGroup, raw: &mut dyn RawSource) -> Result<Vec<Streams>> {
    g.buckets
        .iter()
        .map(|bucket| {
            if bucket.is_patched()
                && let Some(s) = load_bucket(bucket, raw)?
            {
                return Ok(Streams {
                    uv: s.vertices(None)?.lightmap_texcoords,
                    incident: incident(&s)?,
                });
            }
            Ok(Streams::default())
        })
        .collect()
}

fn page(info: Option<&LightmapRenderInfo>, pages: &[Size]) -> Result<Option<Size>> {
    let Some(r) = info.filter(|r| r.bitmap_index >= 0 && r.palette_index >= 0) else {
        return Ok(None);
    };
    Ok(Some(
        *pages
            .get(r.bitmap_index as usize)
            .context("lightmap page outside bitmap")?,
    ))
}

#[derive(Default)]
struct Measure {
    texel_area: f64,
    world_area: f64,
    aligned: u64,
    vectors: u64,
    zero: u64,
    missing: u64,
}

impl Measure {
    fn vertex(&mut self, vector: Option<Vec3>, skies: &[&Sun]) {
        let Some(v) = vector else {
            self.missing += 1;
            return;
        };
        self.vectors += 1;
        let dot = |a: Vec3, b: Vec3| a.iter().zip(b).map(|(a, b)| a * b).sum::<f32>();
        let len = dot(v, v).sqrt();
        if len == 0.0 {
            self.zero += 1;
        } else if skies
            .iter()
            .any(|sun| dot(v, sun.direction) >= len * 15.0_f32.to_radians().cos())
        {
            self.aligned += 1;
        }
    }

    fn triangle(&mut self, xyz: [Vec3; 3], uv: [Uv; 3], size: Size) {
        let [a, b, c] = xyz.map(|p| p.map(f64::from));
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cross = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let world = cross.iter().map(|n| n * n).sum::<f64>().sqrt();
        let [p, q, r] = uv.map(|p| p.map(f64::from));
        let area = ((q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0])).abs()
            * f64::from(size[0])
            * f64::from(size[1]);
        if world > 0.0 && area > 0.0 {
            // Both areas are doubled, which cancels in the density.
            self.world_area += world;
            self.texel_area += area;
        }
    }

    fn mesh(
        &mut self,
        mesh: &Mesh,
        uv: &[Uv],
        vectors: &[Vec3],
        page: Option<Size>,
        instance: Option<&InstancedGeometryInstance>,
        skies: &[&Sun],
    ) -> Result<()> {
        ensure!(instance.is_none_or(|p| p.is_rigid()), "non-rigid placement");
        let mut seen = vec![false; mesh.positions.len()];
        for tri in &mesh.opaque {
            let ix = tri.map(usize::from);
            for i in ix {
                if !std::mem::replace(&mut seen[i], true) {
                    // The rigid layer-3 program rotates its tangent frame
                    // before taking dot products with v5. v5 is world-space
                    // on a placement as well as on a cluster.
                    self.vertex(vectors.get(i).copied(), skies);
                }
            }
            if let Some(size) = page
                && !uv.is_empty()
            {
                let [a, b, c] = ix;
                let get_uv = |i| {
                    uv.get(i)
                        .copied()
                        .context("opaque index outside lightmap UV run")
                };
                // A placement is rendered with rigid rows; translation and
                // rotation preserve area. The unused +0 scale is not applied.
                self.triangle(
                    ix.map(|i| mesh.positions[i]),
                    [get_uv(a)?, get_uv(b)?, get_uv(c)?],
                    size,
                );
            }
        }
        Ok(())
    }

    fn density(&self) -> Option<f64> {
        (self.world_area > 0.0).then(|| (self.texel_area / self.world_area).sqrt())
    }
}

fn run<'a, T>(stream: &'a [T], first: usize, count: usize, fallback: &'a [T]) -> Result<&'a [T]> {
    if stream.is_empty() {
        return Ok(fallback);
    }
    stream
        .get(first..first + count)
        .context("lightmap bucket run outside stream")
}

fn measure(
    bsp: &StructureRender,
    g: &LightmapGroup,
    pages: &[Size],
    raw: &mut dyn RawSource,
    skies: &[&Sun],
) -> Result<Measure> {
    let mut m = Measure::default();
    let materials = bsp.material_count as usize;
    for (i, c) in bsp.clusters.iter().enumerate() {
        if let Some(mesh) = Mesh::load(&c.geometry, raw, materials)? {
            m.mesh(
                &mesh,
                &mesh.own.uv,
                &mesh.own.incident,
                page(g.cluster_render_info.get(i), pages)?,
                None,
                skies,
            )?;
        }
    }
    let definitions = bsp
        .instance_definitions
        .iter()
        .map(|d| Mesh::load(&d.geometry, raw, materials))
        .collect::<Result<Vec<_>>>()?;
    let buckets = bucket_streams(g, raw)?;
    for (i, inst) in bsp.instances.iter().enumerate() {
        let di = usize::try_from(inst.definition).context("negative placement definition")?;
        let Some(mesh) = definitions
            .get(di)
            .context("placement definition out of range")?
        else {
            continue;
        };
        let mut uv = mesh.own.uv.as_slice();
        let mut vectors = mesh.own.incident.as_slice();
        if let Some(r) = g.instance_bucket_refs.get(i)
            && let Some(&Some(first)) = r.section_offsets.first()
        {
            let b = buckets
                .get(usize::from(r.bucket))
                .context("placement bucket outside group")?;
            let n = usize::from(bsp.instance_definitions[di].geometry.info.unknown_0c);
            uv = run(&b.uv, first.into(), n, uv)?;
            vectors = run(&b.incident, first.into(), n, vectors)?;
        }
        m.mesh(
            mesh,
            uv,
            vectors,
            page(g.instance_render_info.get(i), pages)?,
            Some(inst),
            skies,
        )?;
    }
    Ok(m)
}

pub(super) fn report(
    name: &str,
    map: &mut Cache,
    files: &mut ResourceFiles,
    pairs: &[(TagId, Option<TagId>)],
) -> Result<()> {
    let skies = Skies::read(map)?;
    for (index, &(id, ltmp)) in pairs.iter().enumerate() {
        if map.tag(id)?.location.is_none() {
            println!("{name} BSP {index}: no local BSP payload");
            continue;
        }
        let bsp = StructureRender::read(map, id)?;
        let mut active = BTreeSet::new();
        let mut assigned = 0;
        for c in &bsp.clusters {
            if let Some(id) = skies.cluster(c.sky_index) {
                active.insert(id.0);
                assigned += 1;
            }
        }
        let mut lights = Vec::new();
        let mut flags = Vec::new();
        for sky in active {
            for light in &skies.by_id[&sky] {
                lights.push(light);
                flags.push(
                    light
                        .flags
                        .map_or_else(|| "absent".into(), |f| format!("{f:#x}")),
                );
            }
        }
        let description = format!(
            "{name} BSP {index}: clusters_with_sky={assigned}/{} sky_lights={} flags=[{}]",
            bsp.clusters.len(),
            lights.len(),
            flags.join(",")
        );
        let Some(lid) = ltmp else {
            println!("{description} no lightmap");
            continue;
        };
        if map.tag(lid)?.location.is_none() {
            println!("{description} no local lightmap payload");
            continue;
        }
        let l = Lightmap::read(map, lid)?;
        let Some(g) = l.groups.first() else {
            println!("{description} no lightmap group");
            continue;
        };
        let pages = if let Some(bitmap) = g.bitmap {
            let TagData::Bitmap(b) = DecodedTag::read(map, bitmap)?.data else {
                anyhow::bail!("BSP {index} lightmap bitmap has wrong class");
            };
            b.images
                .iter()
                .map(|im| [im.dimensions[0], im.dimensions[1]])
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let group = group_report(&bsp, g, &pages, &mut Raw(files), &lights)
            .with_context(|| format!("BSP {index} geometry census"))?;
        println!("{description} {group}");
    }
    Ok(())
}

fn group_report(
    bsp: &StructureRender,
    g: &LightmapGroup,
    pages: &[Size],
    raw: &mut dyn RawSource,
    lights: &[&Sun],
) -> Result<String> {
    let mut sizes = BTreeMap::new();
    for &size in pages {
        *sizes.entry(size).or_insert(0usize) += 1;
    }
    let sizes = sizes
        .iter()
        .map(|([w, h], n)| format!("{w}x{h}:{n}"))
        .collect::<Vec<_>>()
        .join(",");
    let mut header = format!("group=0 page_sizes=[{sizes}]");
    if g.bitmap.is_none() {
        header.push_str(" bitmap=absent");
    }
    // Two scenario/lightmap pairs on retail have different checksums.
    // The page table belongs to the tag. Its placement and cluster
    // indices cannot be used with the paired BSP.
    if g.structure_checksum != bsp.checksum {
        return Ok(format!(
            "{header} geometry=unmeasured(checksum-mismatch) texels_per_world_unit=n/a aligned_15deg=n/a"
        ));
    }
    let m = measure(bsp, g, pages, raw, lights)?;
    let density = m.density().map_or("n/a".into(), |d| format!("{d:.3}"));
    let pct = if m.vectors == 0 {
        "n/a".into()
    } else {
        format!("{:.2}%", 100.0 * m.aligned as f64 / m.vectors as f64)
    };
    Ok(format!(
        "{header} texels_per_world_unit={density} aligned_15deg={}/{} ({pct}) zero_vectors={} missing_vectors={}",
        m.aligned, m.vectors, m.zero, m.missing
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mismatched_group_reports_its_pages_and_a_matched_bitmapless_group_is_measured() {
        struct EmptyRaw;
        impl RawSource for EmptyRaw {
            fn read_raw(&mut self, _: map::RawPointer, _: u32) -> tags::Result<Vec<u8>> {
                panic!("these groups must not read raw resources")
            }
        }
        let mut bsp_bytes = vec![0; 0x148];
        bsp_bytes[8..12].copy_from_slice(&77u32.to_le_bytes());
        let bsp = StructureRender::decode(TagId(7), &tags::geometry::TagView::new(&bsp_bytes, 0))
            .unwrap();
        let mut bytes = vec![0; 0x104 + 0x68];
        bytes[0x80..0x84].copy_from_slice(&1u32.to_le_bytes());
        bytes[0x84..0x88].copy_from_slice(&0x104u32.to_le_bytes());
        bytes[0x108..0x10c].copy_from_slice(&88u32.to_le_bytes());
        bytes[0x11c..0x120].copy_from_slice(&map::FourCC(*b"bitm").as_u32().to_le_bytes());
        bytes[0x120..0x124].copy_from_slice(&1u32.to_le_bytes());
        let read = |bytes: &[u8]| {
            Lightmap::decode(TagId(9), &tags::geometry::TagView::new(bytes, 0))
                .unwrap()
                .groups
                .remove(0)
        };
        let row = group_report(&bsp, &read(&bytes), &[[32, 64]; 2], &mut EmptyRaw, &[]).unwrap();
        assert!(row.contains("page_sizes=[32x64:2]"));
        assert!(row.contains("geometry=unmeasured(checksum-mismatch)"));
        assert!(row.contains("aligned_15deg=n/a"));
        bytes[0x108..0x10c].copy_from_slice(&77u32.to_le_bytes());
        bytes[0x120..0x124].copy_from_slice(&u32::MAX.to_le_bytes());
        let row = group_report(&bsp, &read(&bytes), &[], &mut EmptyRaw, &[]).unwrap();
        assert!(row.contains("page_sizes=[] bitmap=absent"));
        assert!(row.contains("texels_per_world_unit=n/a aligned_15deg=0/0 (n/a)"));
    }

    #[test]
    fn authored_sky_angles_select_a_light_without_hemisphere_folding() {
        let mut bytes = vec![0; 0xac + 0x34 + 0x28];
        let mut put = |at: usize, word: u32| bytes[at..at + 4].copy_from_slice(&word.to_le_bytes());
        put(4, u32::MAX);
        put(0x78, 1);
        put(0x7c, 0xac);
        put(0x80, 1.57f32.to_bits()); // sky model's rotation
        put(0xac + 0xc, std::f32::consts::FRAC_PI_2.to_bits());
        put(0xac + 0x10, (-std::f32::consts::FRAC_PI_4).to_bits());
        put(0xac + 0x18, u32::MAX);
        put(0xac + 0x2c, 1);
        put(0xac + 0x30, 0xe0);
        put(0xe0, 0x43);
        let view = tags::geometry::TagView::new(&bytes, 0);
        let Atmosphere::Sky(s) = Atmosphere::decode(map::FourCC(*b"sky "), &view).unwrap() else {
            panic!()
        };
        let lights = suns(s);
        assert_eq!(lights[0].flags, Some(0x43));
        let other = Sun {
            direction: [-1., 0., 0.],
            flags: None,
        };
        let mut m = Measure::default();
        m.vertex(Some([0., 0.5, -0.5]), &[&other, &lights[0]]);
        assert_eq!(m.aligned, 1);
    }

    #[test]
    fn sky_selection_preserves_empty_slots_and_applies_the_scenario_flag() {
        let mut skies = Skies {
            palette: vec![Some(TagId(16)), None, Some(TagId(42))],
            by_id: BTreeMap::new(),
            default_first: false,
        };
        assert_eq!(skies.cluster(-1), None);
        assert_eq!(skies.cluster(1), None);
        assert_eq!(skies.cluster(2), Some(TagId(42)));
        skies.default_first = true;
        assert_eq!(skies.cluster(-1), Some(TagId(16)));
        assert_eq!(skies.cluster(3), None);
        assert_eq!(skies.cluster(-2), None);
        skies.palette.clear();
        assert_eq!(skies.cluster(-1), None);
    }

    #[test]
    fn measured_geometry_uses_page_dimensions_and_unique_vertices() {
        let mesh = Mesh {
            positions: vec![[0., 0., 0.], [2., 0., 0.], [2., 4., 0.], [0., 4., 0.]],
            opaque: vec![[0, 1, 2], [0, 2, 3]],
            own: Streams::default(),
        };
        let uv = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
        let incident = [[0.5, 0., 0.], [1., 0.26, 0.], [1., 0.28, 0.], [0., 0., 0.]];
        let sun = Sun {
            direction: [1., 0., 0.],
            flags: None,
        };
        let mut m = Measure::default();
        m.mesh(&mesh, &uv, &incident, Some([32, 64]), None, &[&sun])
            .unwrap();
        assert_eq!(m.density(), Some(16.));
        assert_eq!((m.aligned, m.vectors, m.zero, m.missing), (2, 4, 1, 0));
        // Rotating the mesh changes neither its world-area density nor the
        // world-space incident vector stored in its own lightmap bucket.
        let inst = InstancedGeometryInstance {
            scale: 20.,
            basis: [[0., 1., 0.], [-1., 0., 0.], [0., 0., 1.]],
            position: [90., 80., 70.],
            definition: 0,
            raw: vec![],
        };
        m.mesh(&mesh, &uv, &incident, Some([32, 64]), Some(&inst), &[&sun])
            .unwrap();
        assert_eq!(m.density(), Some(16.));
        assert_eq!((m.aligned, m.vectors), (4, 8));
        m.mesh(&mesh, &[], &[], None, None, &[]).unwrap();
        assert_eq!(m.missing, 4);
        m.mesh(&mesh, &[], &incident, None, None, &[]).unwrap();
        assert_eq!((m.aligned, m.vectors), (4, 12));
    }

    #[test]
    fn a_bucket_run_is_limited_to_the_placements_opaque_vertices() {
        let stream = [0, 1, 2, 3, 4, 5];
        assert_eq!(run(&stream, 2, 3, &[]).unwrap(), &[2, 3, 4]);
        assert!(run(&stream, 4, 3, &[]).is_err());
        assert_eq!(run(&[], 0, 3, &stream).unwrap(), &stream);
    }
}
