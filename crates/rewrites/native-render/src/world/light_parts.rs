//! A gathered structure part identifies a range in the section's original
//! indices. Its record can draw strips as lists. Both forms share vertices.

use super::{Owner, SectionInput, WorldDraw, part_indices};
use crate::subpart_map::TriangleMap;
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
    /// The part's source indices, for its visible subparts' ranges.
    raw: Vec<u16>,
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
                raw: source.part_indices(part).to_vec(),
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
        let mut ranges = ResidentRanges {
            section: self.section, first: source.first, count: source.count,
            topology: source.topology, native, as_list, material, inputs, strip_map: None, list_map: None,
        };
        (ranges.strip_map, ranges.list_map) = ranges.maps(source, vertices);
        Some(ranges)
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
    /// The native strip's and the list's triangles per source position
    /// ([`TriangleMap`]); `None` where the map does not count the range.
    strip_map: Option<TriangleMap>,
    list_map: Option<TriangleMap>,
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

    /// The draws of the visible subpart spans `spans` (absolute source
    /// indices) of this part in the record's topology: one per native
    /// range, empty ones left out (`crate::subpart_map`).
    pub fn subpart_draws(&self, stream: &LightStream, spans: &[Range<u32>], draw: WorldDraw) -> Result<Vec<WorldDraw>, &'static str> {
        let whole = self.draw(stream, draw)?;
        let (map, start) = match stream.topology {
            Topology::List => (self.list_map.as_ref(), self.as_list.start),
            Topology::Strip => (self.strip_map.as_ref(), self.native.start),
        };
        let ranges = map
            .ok_or("structure part's triangles do not map its subparts")?
            .ranges(u32::from(self.first), start, spans)
            .ok_or("a visible subpart span off its part's triangles or an odd strip start")?;
        Ok(ranges.into_iter().map(|r| WorldDraw { first_index: r.start, index_count: r.end - r.start, ..whole }).collect())
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
            && self.maps(next, input.positions.len()) == (self.strip_map.clone(), self.list_map.clone())
    }

    /// The subpart maps of `source` over these ranges, each only where it
    /// counts the triangles its range holds.
    fn maps(&self, source: &SourcePart, vertices: usize) -> (Option<TriangleMap>, Option<TriangleMap>) {
        let map = |strip: bool, range: &Range<u32>| {
            Some(TriangleMap::new(&source.raw, strip, Some(vertices))).filter(|m| m.fits(range))
        };
        let strip = (source.topology == Topology::Strip).then(|| map(true, &self.native)).flatten();
        let list = map(false, if source.topology == Topology::List { &self.native } else { &self.as_list });
        (strip, list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{PartInput, Placement, WorldBuilder};

    #[test]
    fn structure_part_ids_use_all_eight_bits_without_wrapping() {
        let owner = Owner::Cluster(0);
        let part = PartInput { material: 0, transparent: false, triangles: vec![[0, 1, 2]] };
        let source = SourcePart { first: 0, count: 3, topology: Topology::List, as_list: None, raw: vec![0, 1, 2] };
        let mut input = SectionInput {
            positions: vec![[0.; 3]; 3],
            parts: vec![part; 257],
            source: Some(SourceSection { section: 0, parts: vec![source; 257] }),
            ..Default::default()
        };
        input.parts[255].transparent = true;
        input.parts[256].material = 1;
        let mut b = WorldBuilder::new();
        b.add_section(owner, &input, &Placement::IDENTITY, None);
        let world = b.finish();
        assert_eq!(world.draws.len(), 255);
        assert_eq!(world.transparents[0].part, 255);
        assert_eq!(world.light_parts.len(), 256);
        assert_eq!(world.light_parts[&(owner, 0)].material, 0);
        assert_eq!(world.light_parts[&(owner, 16)].native, 48..51);
        assert_eq!(world.light_parts[&(owner, 255)].native, 765..768);
    }

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
                raw: vec![5, 0, 1, 2, 3, 4],
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
        let got = |stream: &LightStream| -> Vec<u32> {
            let d = ranges.draw(stream, draw).unwrap();
            world.indices[d.first_index as usize..(d.first_index+d.index_count) as usize].to_vec()
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

    #[cfg(feature = "gpu")]
    #[test]
    fn visible_subparts_of_a_strip_with_degenerates_draw_their_own_native_triangles() {
        use tags::geometry::strip_triangles;
        let owner = Owner::Cluster(3);
        // Two sub-strips stitched by degenerates, one more inside the
        // second; vertex 11 is past the section's eleven.
        let raw: Vec<u16> = vec![0, 1, 2, 3, 3, 4, 4, 5, 6, 6, 7, 8, 9, 10, 11];
        let section = SectionInput {
            positions: vec![[0.; 3]; 11],
            parts: vec![PartInput { material: 0, transparent: false, triangles: strip_triangles(&raw) }],
            source: Some(SourceSection { section: 3, parts: vec![SourcePart {
                first: 40, count: raw.len() as u16, topology: Topology::Strip,
                as_list: Some(list_triangles(&raw)), raw: raw.clone(),
            }] }),
            ..Default::default()
        };
        let mut b = WorldBuilder::new();
        b.add_section(owner, &section, &Placement::IDENTITY, None);
        let world = b.finish();
        let draw = world.draws[0];
        let world = crate::world::keep_draws(world, &|_| false);
        let ranges = &world.light_parts[&(owner, 0)];
        let mut stream = LightStream {
            section: 3, part: 0, vertex_class: 0, first_index: 40, index_count: raw.len() as u32, topology: Topology::Strip,
        };
        let drawn = |stream: &LightStream, spans: &[Range<u32>]| -> Vec<u32> {
            ranges.subpart_draws(stream, spans, draw).unwrap().iter()
                .flat_map(|d| world.indices[d.first_index as usize..(d.first_index + d.index_count) as usize].to_vec())
                .collect()
        };
        // The sub-strip's own strip, its degenerates and far triangle gone.
        let own = |span: Range<usize>| -> Vec<u32> {
            strip_triangles(&raw[span]).into_iter().filter(|t| t.iter().all(|&i| i < 11))
                .flatten().map(u32::from).collect()
        };
        assert_eq!(drawn(&stream, &[40..44]), own(0..4));
        assert_eq!(drawn(&stream, &[46..55]), own(6..15));
        assert_eq!(drawn(&stream, &[40..44, 46..55]), [own(0..4), own(6..15)].concat());
        assert!(ranges.subpart_draws(&stream, &[41..44], draw).is_err(), "an odd start winds the other way");
        assert!(ranges.subpart_draws(&stream, &[46..56], draw).is_err(), "past the part");
        // The same spans as a list read the raw triangles 0..6 and 6..15.
        stream.topology = Topology::List;
        let list = |span: Range<usize>| -> Vec<u32> {
            list_triangles(&raw[span]).into_iter().filter(|t| t.iter().all(|&i| i < 11))
                .flatten().map(u32::from).collect()
        };
        assert_eq!(drawn(&stream, &[46..55]), list(6..15));
        assert!(ranges.subpart_draws(&stream, &[44..46], draw).is_err(), "off a triangle");
    }
}
