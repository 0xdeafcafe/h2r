//! A gathered structure part identifies a range in the section's original
//! indices. Its record can draw strips as lists. Both forms share vertices.

use super::{Owner, SectionInput, WorldDraw, part_indices};
use frame_scene::{LightStream, Topology};
use std::{collections::BTreeMap, ops::Range};
#[cfg(feature = "gpu")]
use tags::geometry::{Section, list_triangles};

#[derive(Clone, Debug)]
pub struct SourceSection {
    section: u16,
    parts: Vec<SourcePart>,
}

#[derive(Clone, Debug)]
struct SourcePart {
    first: u16,
    count: u16,
    topology: Topology,
    // A strip with more than three indices can produce different triangles
    // when the gather clears the record's strip bit.
    as_list: Option<Vec<[u16; 3]>>,
}

impl SourceSection {
    #[cfg(feature = "gpu")]
    pub(crate) fn from_section(section: u16, source: &Section) -> Self {
        let parts = source.parts.iter().map(|part| {
            let list = part.is_triangle_list();
            SourcePart {
                first: part.first_index,
                count: part.index_count,
                topology: if list { Topology::List } else { Topology::Strip },
                as_list: (!list && part.is_drawn() && part.index_count > 3)
                    .then(|| list_triangles(source.part_indices(part))),
            }
        }).collect();
        Self { section, parts }
    }

    pub(super) fn append(
        &self, index: u8, indices: &mut Vec<u32>, native: Range<u32>,
        vertices: usize, base: u32, material: u16, inputs: u16,
    ) -> Option<ResidentRanges> {
        let source = self.parts.get(usize::from(index))?;
        let as_list = if let Some(alt) = &source.as_list {
            let (list, _) = part_indices(alt, vertices, base);
            if &indices[native.start as usize..native.end as usize] == list.as_slice() {
                native.clone()
            } else {
                let start = indices.len() as u32;
                indices.extend(list);
                start..indices.len() as u32
            }
        } else {
            native.clone()
        };
        Some(ResidentRanges {
            section: self.section, first: source.first, count: source.count,
            topology: source.topology, native, as_list, material, inputs,
        })
    }
}

pub type Ranges = BTreeMap<(Owner, u8), ResidentRanges>;

#[derive(Clone, Debug)]
pub struct ResidentRanges {
    section: u16,
    first: u16,
    count: u16,
    topology: Topology,
    native: Range<u32>,
    as_list: Range<u32>,
    material: u16,
    inputs: u16,
}

impl ResidentRanges {
    pub(super) fn inputs(&self) -> u16 { self.inputs }

    /// The uploaded triangles for this exact part and the record's topology.
    pub fn draw(&self, stream: &LightStream, draw: WorldDraw) -> Result<WorldDraw, &'static str> {
        if stream.section != self.section || stream.first_index != u32::from(self.first)
            || stream.index_count != u32::from(self.count) {
            return Err("gathered structure section or source index range changed");
        }
        let range = match stream.topology {
            Topology::List => &self.as_list,
            Topology::Strip if self.topology == Topology::Strip => &self.native,
            _ => return Err("gathered structure topology differs from its part"),
        };
        Ok(WorldDraw { first_index: range.start, index_count: range.end - range.start, ..draw })
    }

    /// A live section edit can keep these ranges if it changes only vertex values.
    pub(super) fn unchanged(
        &self, part: u8, input: &SectionInput, indices: &[u32], base: u32, inputs: u16,
    ) -> bool {
        let Some(section) = &input.source else { return false };
        let Some(next) = section.parts.get(usize::from(part)) else { return false };
        let Some(draw) = input.parts.get(usize::from(part)) else { return false };
        let equal = |range: &Range<u32>, triangles: &[[u16; 3]]| {
            let (new, _) = part_indices(triangles, input.positions.len(), base);
            indices.get(range.start as usize..range.end as usize) == Some(new.as_slice())
        };
        self.section == section.section && self.first == next.first && self.count == next.count
            && self.topology == next.topology && self.material == draw.material && self.inputs == inputs
            && equal(&self.native, &draw.triangles)
            && next.as_list.as_ref().is_none_or(|list| equal(&self.as_list, list))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{PartInput, Placement, WorldBuilder};

    #[test]
    fn a_gather_can_draw_the_same_raw_strip_as_a_list_after_base_filtering() {
        let owner = Owner::Cluster(9);
        let section = SectionInput {
            positions: vec![[1., 2., 3.]; 6],
            parts: vec![PartInput {
                material: 2, transparent: false, triangles: vec![[5, 0, 1], [0, 2, 1], [1, 2, 3], [2, 4, 3]],
            }],
            source: Some(SourceSection { section: 9, parts: vec![SourcePart {
                first: 15, count: 6, topology: Topology::Strip, as_list: Some(vec![[5, 0, 1], [2, 3, 4]]),
            }] }),
            ..Default::default()
        };
        let mut b = WorldBuilder::new();
        b.add_section(owner, &section, &Placement::IDENTITY, None);
        let world = b.finish();
        let draw = world.draws[0];
        let mut world = crate::world::keep_draws(world, &|_| false);
        let ranges = &world.light_parts[&(owner, 0)];
        let mut stream = LightStream { section: 9, part: 0, vertex_class: 0, first_index: 15, index_count: 6, topology: Topology::List };
        let got = |stream| {
            let d = ranges.draw(stream, draw).unwrap();
            &world.indices[d.first_index as usize..(d.first_index+d.index_count) as usize]
        };
        assert_eq!(got(&stream), [5, 0, 1, 2, 3, 4]);
        stream.topology = Topology::Strip;
        assert_eq!(got(&stream), [5, 0, 1, 0, 2, 1, 1, 2, 3, 2, 4, 3]);
        stream.first_index += 1;
        assert!(ranges.draw(&stream, draw).is_err());
        let mut edited = section.clone();
        edited.source.as_mut().unwrap().parts[0].as_list.as_mut().unwrap().reverse();
        assert!(world.replace_section(owner, &edited).is_err());
        assert!(world.replace_section(owner, &section).is_ok());
    }
}
