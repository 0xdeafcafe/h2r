//! The paired scenery tables in the lightmap group. The object info at
//! +0x58 carries the scenario identifier and mode checksum (Assembly's
//! public Halo2/ltmp schema). The draw selects the +0x60 reference by
//! object row, then its first-vertex offset by mode section index. See
//! docs/graphics/native-world.md, "What a world pass reads".
use super::{Bucket, Inputs, Scenery, vertex_run};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};
use tags::{geometry::TagView, structure::LightmapGroup};

type ObjectId = [u8; 8];

#[derive(Clone, Copy)]
struct ObjectInfo {
    id: ObjectId,
    checksum: u32,
}

impl ObjectInfo {
    fn read(row: &[u8]) -> Self {
        // The caller reads rows of exactly 12 bytes through the tag view.
        Self {
            id: row[..8].try_into().unwrap(),
            checksum: u32::from_le_bytes(row[8..12].try_into().unwrap()),
        }
    }

    fn match_placement<'a>(
        &self,
        placements: &BTreeMap<ObjectId, Option<&'a Scenery>>,
    ) -> Result<&'a Scenery> {
        let object = placements
            .get(&self.id)
            .context("no posed scenario scenery matches the placement identifier")?
            .context("multiple scenario placements have this identifier")?;
        ensure!(
            object.checksum == self.checksum,
            "lightmap expects mode checksum {:#010x}, {} has {:#010x}",
            self.checksum,
            object.label,
            object.checksum
        );
        Ok(object)
    }
}

/// Both instance and object offsets index the same bucket. A bad model
/// count could fit the stream but cross into the next owner's colours.
pub(super) fn offsets(group: &LightmapGroup) -> Vec<BTreeSet<usize>> {
    let mut starts = vec![BTreeSet::new(); group.buckets.len()];
    for row in group
        .instance_bucket_refs
        .iter()
        .chain(&group.cluster_bucket_refs)
    {
        // Lightmap::decode checks the bucket index.
        starts[usize::from(row.bucket)].extend(
            row.section_offsets
                .iter()
                .flatten()
                .copied()
                .map(usize::from),
        );
    }
    starts
}

pub(super) fn append_objects(
    group: &LightmapGroup,
    view: &TagView<'_>,
    buckets: &mut [Bucket],
    objects: &[Scenery],
    selected: &dyn Fn([f32; 3]) -> bool,
    inputs: &mut Inputs,
) -> Result<()> {
    let rows = view.block(&group.raw, 0x58, 12)?;
    if rows.len() != group.cluster_bucket_refs.len() {
        inputs.warnings.push(format!(
            "{} scenery identifiers for {} bucket references; all scenery colour runs unresolved",
            rows.len(),
            group.cluster_bucket_refs.len()
        ));
        return Ok(());
    }
    let mut placements = BTreeMap::new();
    for object in objects {
        placements
            .entry(object.id)
            .and_modify(|candidate| *candidate = None)
            .or_insert(Some(object));
    }

    let starts = offsets(group);
    for (index, (row, reference)) in rows.iter().zip(&group.cluster_bucket_refs).enumerate() {
        let bucket_index = usize::from(reference.bucket);
        let bucket = buckets
            .get_mut(bucket_index)
            .context("missing decoded scenery bucket")?;
        if bucket.colors.is_empty() || reference.section_offsets.iter().all(Option::is_none) {
            continue;
        }
        let info = ObjectInfo::read(row);
        let object = match info.match_placement(&placements) {
            Ok(object) => object,
            Err(e) => {
                inputs.warnings.push(format!(
                    "Scenery row {index}, bucket {bucket_index}, identifier {:02x?}: {e}",
                    info.id
                ));
                continue;
            }
        };
        for (section_index, first) in reference.section_offsets.iter().enumerate() {
            let Some(first) = first.map(usize::from) else {
                continue;
            };
            let label = format!(
                "{} (scenery row {index}), section {section_index}, bucket {bucket_index} vertex {first}",
                object.label
            );
            let Some(section) = object.sections.iter().find(|s| s.index == section_index) else {
                inputs
                    .warnings
                    .push(format!("{label}: missing posed section"));
                continue;
            };
            let count = section.opaque_count;
            if count == 0 {
                continue;
            }
            let Some(positions) = section.position.get(..count) else {
                inputs.warnings.push(format!(
                    "{label}: {count} opaque vertices exceed posed vertex count {}",
                    section.position.len()
                ));
                continue;
            };
            let next = starts[bucket_index].range((first + 1)..).next().copied();
            if next.is_some_and(|end| count > end - first) {
                inputs.warnings.push(format!(
                    "{label}: {count} opaque vertices overlap the next bucket run"
                ));
                continue;
            }
            let faces: Vec<_> = section
                .triangles
                .iter()
                .copied()
                .filter(|f| f.iter().all(|v| usize::from(*v) < count))
                .collect();
            for (kind, colors) in &bucket.colors {
                let label = format!("{label}, stream {kind:02x}");
                let Some(retail) = colors.get(first..first + count) else {
                    inputs.warnings.push(format!(
                        "{label}: {count} opaque vertices exceed stream length {}",
                        colors.len()
                    ));
                    continue;
                };
                match vertex_run(label.clone(), positions, &faces, retail, selected) {
                    Ok(run) => {
                        inputs.vertices.push(run);
                        bucket.coverage[first..first + count].fill(true);
                    }
                    Err(e) => inputs.warnings.push(format!("{label}: {e}")),
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::ScenerySection;
    use super::*;
    use map::TagId;
    use tags::structure::Lightmap;

    // Two independently ordered tables, and a mode with geometry only for
    // section 1. Both rows have the same low 32-bit id; only all eight
    // bytes pick the right scenery and its placement.
    fn fixture() -> (Vec<u8>, LightmapGroup, Vec<Scenery>, Vec<Bucket>) {
        let mut payload = vec![0u8; 0x240];
        let put = |p: &mut [u8], offset: usize, n: u32| {
            p[offset..offset + 4].copy_from_slice(&n.to_le_bytes());
        };
        let g = 0x104;
        for (field, n) in [
            (0x80, 1),
            (0x84, g),
            (g + 0x1c, u32::MAX as usize),
            (g + 0x40, 1),
            (g + 0x44, 0x170),
            (0x17c, u32::MAX as usize),
            (g + 0x58, 2),
            (g + 0x5c, 0x1b0),
            (g + 0x60, 2),
            (g + 0x64, 0x1d0),
            (0x1d4, 2),
            (0x1d8, 0x1f0),
            (0x1e0, 2),
            (0x1e4, 0x1f4),
        ] {
            put(&mut payload, field, n as u32);
        }
        payload[0x170] = 3;
        let mut objects = Vec::new();
        for i in 0..2usize {
            let id = [11, 0, 0, 0, i as u8, 0, 6, 1];
            let row = 0x1b0 + i * 12;
            payload[row..row + 8].copy_from_slice(&id);
            put(&mut payload, row + 8, 101 + i as u32);
            let table = 0x1f0 + i * 4;
            payload[table..table + 2].copy_from_slice(&u16::MAX.to_le_bytes());
            payload[table + 2..table + 4].copy_from_slice(&(3 * i as u16).to_le_bytes());
            let z = 20.0 * i as f32;
            objects.push(Scenery {
                id,
                checksum: 101 + i as u32,
                label: format!("fixture {i}"),
                sections: vec![ScenerySection {
                    index: 1,
                    opaque_count: 3,
                    position: vec![[0., 0., z], [1., 0., z], [0., 1., z], [0., 0., z + 1.]],
                    triangles: vec![[0, 1, 2], [0, 1, 3]],
                }],
            });
        }
        objects.reverse();
        let group = Lightmap::decode(TagId(1), &TagView::new(&payload, 0))
            .unwrap()
            .groups
            .remove(0);
        let buckets = vec![Bucket {
            colors: vec![
                (0x2e, vec![[10, 30, 110]; 6]),
                (0x2f, vec![[55, 70, 140]; 6]),
            ],
            coverage: vec![false; 6],
            ..Default::default()
        }];
        (payload, group, objects, buckets)
    }

    fn receivers(
        payload: &[u8],
        group: &LightmapGroup,
        objects: &[Scenery],
        buckets: &mut [Bucket],
    ) -> Inputs {
        let mut result = Inputs {
            pages: vec![],
            vertices: vec![],
            warnings: vec![],
        };
        append_objects(
            group,
            &TagView::new(payload, 0),
            buckets,
            objects,
            &|_| true,
            &mut result,
        )
        .unwrap();
        result
    }

    #[test]
    fn opaque_section_prefix_uses_the_matching_placed_model_in_both_streams() {
        let (payload, group, objects, mut buckets) = fixture();
        let out = receivers(&payload, &group, &objects, &mut buckets);
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_eq!(out.vertices.len(), 4);
        assert_eq!(out.vertices[3].vertices.len(), 3);
        assert_eq!(out.vertices[0].vertices[0].retail, [10, 30, 110]);
        assert_eq!(out.vertices[3].vertices[0].retail, [55, 70, 140]);
        assert_eq!(out.vertices[0].vertices[0].position[2], 0.);
        assert_eq!(out.vertices[3].vertices[0].position[2], 20.);
        assert!(buckets[0].coverage.iter().all(|b| *b));
    }

    #[test]
    fn a_stale_checksum_and_a_run_overlapping_another_owner_are_refused_separately() {
        let (payload, group, mut objects, mut buckets) = fixture();
        objects[0].checksum += 1;
        objects[1].sections[0].opaque_count = 4;
        let out = receivers(&payload, &group, &objects, &mut buckets);
        assert!(out.vertices.is_empty());
        assert!(out.warnings.iter().any(|s| s.contains("overlap")));
        assert!(out.warnings.iter().any(|s| s.contains("checksum")));
        assert!(buckets[0].coverage.iter().all(|b| !b));
    }
}
