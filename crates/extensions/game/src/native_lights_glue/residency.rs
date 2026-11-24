//! Geometry for light passes over the structure. Each pass reads the shared
//! vertex and index buffers, even when layer 3 leaves the part to the guest.

use super::{ClusterParts, LitMaterial};
use map::TagId;
use native_render::look::{RIGID_CLASSIFICATION, WORLD_CLASSIFICATION};
use native_render::world::{Owner, World, WorldDraw};
use std::collections::HashMap;

pub(super) fn classification(owner: Owner) -> u16 {
    match owner {
        Owner::Cluster(_) => WORLD_CLASSIFICATION as u16,
        Owner::Instance(_) => RIGID_CLASSIFICATION as u16,
    }
}

pub(super) fn structure_parts(
    world: &World,
    lit: &HashMap<u16, TagId>,
    materials: &HashMap<u16, LitMaterial>,
) -> (ClusterParts, usize) {
    let mut parts = ClusterParts::new();
    let mut unread = 0;
    for draw in world.draws.iter().copied().chain(world.transparents.iter().map(|d| WorldDraw {
        owner: d.owner,
        part: d.part,
        material: d.material,
        lightmap: d.lightmap,
        first_index: d.first_index,
        index_count: d.index_count,
        bounds: d.bounds,
        batch: u32::MAX, // Light draws bind by material and have no world batch.
        inputs: d.inputs,
    })) {
        if let Some(&shader) = lit.get(&draw.material) {
            parts.insert((draw.owner, draw.part), (shader, draw));
        } else if !materials.contains_key(&draw.material) {
            unread += 1;
        }
    }
    (parts, unread)
}

#[cfg(test)]
mod tests {
    use super::*;
    use native_render::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder, keep_draws};

    #[test]
    fn light_parts_still_name_the_uploaded_indices_with_no_base_draws() {
        let mut builder = WorldBuilder::new();
        let section = SectionInput {
            positions: vec![[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            uvs: vec![[0.; 2]; 4],
            parts: vec![
                PartInput { material: 7, transparent: false, triangles: vec![[0, 1, 2]] },
                PartInput { material: 8, transparent: true, triangles: vec![[3, 2, 1]] },
                PartInput { material: 9, transparent: false, triangles: vec![[3, 0, 1]] },
            ],
            ..Default::default()
        };
        builder.add_section(Owner::Cluster(5), &section, &Placement::IDENTITY, None);
        builder.add_section(Owner::Instance(6), &section, &Placement::IDENTITY, None);
        let world = builder.finish();
        let lit = HashMap::from([(7, TagId(0x1234_0007)), (8, TagId(0xabcd_0008))]);
        let (parts, unread) = structure_parts(&world, &lit, &HashMap::new());
        let uploaded = keep_draws(world, &|_| false);
        assert!(uploaded.draws.is_empty());
        assert_eq!((parts.len(), unread), (4, 2));
        for (owner, base) in [(Owner::Cluster(5), 0), (Owner::Instance(6), 4)] {
            for (part, material, triangle) in [(0, 7, [0, 1, 2]), (1, 8, [3, 2, 1])] {
                let (shader, draw) = parts[&(owner, part)];
                assert_eq!(shader, lit[&material]);
                assert_eq!((draw.owner, draw.part, draw.material, draw.inputs), (owner, part, material, 3));
                let first = draw.first_index as usize;
                let end = first + draw.index_count as usize;
                assert_eq!(&uploaded.indices[first..end], &triangle.map(|i| base + i));
            }
        }
    }
}
