//! Model parts for the light passes, built at load
//! (docs/graphics/native-lights.md, "Gathered groups in the scene"): what
//! a light layer drawn from its scene group alone needs to draw an object
//! entry without the guest's geometry handler (`0x0004d0b0`).
//!
//! Per model section: its parts' triangles in one index list (the part's
//! raw indices decoded as their authored topology and, for strips, also
//! as lists for records that clear the strip bit), its decompression rows,
//! and its vertices once per (vertex shader, classification) its parts'
//! materials' shadow and light implementations name,
//! gathered through the declaration that program's input semantics give
//! over the section's streams ([`crate::object_streams::declaration`],
//! [`crate::object_streams::gather`]). The pass itself is compiled with
//! that declaration's registers and fetch
//! ([`crate::light_look::compile_light_model`],
//! [`crate::shadow_look::compile_shadow_model`]).
//!
//! An entry finds its part by its model, section and part
//! ([`LightModels::part`]); its raw range must match and it must use an
//! available topology, with a sorted vertex classification a buffer was built
//! for ([`LightSection::buffer`]). Anything else is not resident, and the
//! layer keeps its handlers (`light_scene::Refusal::Stream`).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;

use frame_scene::{LightEntry, Row, Topology};
use map::TagId;

use crate::light_gpu::ModelLayout;
use crate::object_program_gpu::attributes;
use crate::object_streams::{
    Element, SLOT, StreamData, StreamFormats, declaration, gather, registers,
};

/// One mode section as the light build reads it.
#[derive(Clone)]
pub struct ModelSection<'a> {
    pub index: u16,
    /// Its geometry classification at `+0x18`.
    pub geometry: u16,
    pub decompression: [Row; 3],
    /// Its streams by descriptor slot: (stream type, bytes).
    pub streams: Vec<Option<(u8, StreamData<'a>)>>,
    pub vertex_count: usize,
    pub parts: Vec<SectionPart<'a>>,
}

/// One part of a section: its index among the section's parts, its raw
/// index range, whether that range is a list, and the
/// vertex shaders its material's lit passes name.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionPart<'a> {
    pub part: u8,
    /// The section influence byte selected by this part's kind.
    pub influences: u8,
    pub first_index: u32,
    pub list: bool,
    pub raw_indices: &'a [u16],
    pub vertex_shaders: Vec<TagId>,
}

/// What a vertex shader gives a section: the classification its program
/// takes there and that program's input semantics; `None` when it has
/// none.
pub type Semantics<'s> = dyn Fn(TagId, u16, u8) -> Option<(u32, Vec<u16>)> + 's;

/// A buffer the device holds once: its bytes, uploaded at the first
/// draw on a device and kept for it.
pub struct DeviceBuffer {
    bytes: Vec<u8>,
    usage: wgpu::BufferUsages,
    label: &'static str,
    held: Mutex<Option<(wgpu::Device, wgpu::Buffer)>>,
}

impl DeviceBuffer {
    pub fn new(bytes: Vec<u8>, usage: wgpu::BufferUsages, label: &'static str) -> Self {
        Self {
            bytes,
            usage,
            label,
            held: Mutex::new(None),
        }
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// The buffer on `device`, uploaded the first time.
    pub fn get(&self, device: &wgpu::Device) -> wgpu::Buffer {
        use wgpu::util::DeviceExt;
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((owner, buffer)) = held.as_ref()
            && owner == device
        {
            return buffer.clone();
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(self.label),
            contents: &self.bytes,
            usage: self.usage,
        });
        *held = Some((device.clone(), buffer.clone()));
        buffer
    }
}

/// A section's vertices through one program's declaration.
pub struct LightBuffer {
    pub vertex_shader: TagId,
    pub classification: u32,
    pub declaration: Vec<Element>,
    pub layout: ModelLayout,
    pub vertices: DeviceBuffer,
}

impl LightBuffer {
    /// The registers the declaration supplies (bit `n`: `v<n>`).
    pub fn provided(&self) -> u16 {
        registers(&self.declaration)
    }

    /// Each register's fetch ([`crate::object_program_gpu::input_fetch`]).
    pub fn fetch(&self) -> [shaders::vsh::InputFetch; 16] {
        crate::object_program_gpu::input_fetch(&self.declaration)
    }
}

/// A part as the light passes hold it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightPart {
    /// For a host light without a sorted classification: the class shared
    /// by the resident programs this part uses.
    pub vertex_class: Option<u16>,
    pub first_index: u32,
    pub index_count: u32,
    pub topology: Topology,
    /// The part drawn in its authored topology, also used by host lights.
    pub indices: Range<u32>,
    /// The part drawn as a list, as a light record can request for a strip.
    pub as_list: Range<u32>,
}

/// One section's parts, decompression and buffers.
pub struct LightSection {
    pub decompression: [Row; 3],
    pub parts: HashMap<u8, LightPart>,
    pub indices: DeviceBuffer,
    pub buffers: Vec<LightBuffer>,
}

impl LightSection {
    /// The buffer built for `vertex_shader`'s program of classification
    /// `classification`.
    pub fn buffer(&self, vertex_shader: TagId, classification: u32) -> Option<&LightBuffer> {
        self.buffers
            .iter()
            .find(|b| b.vertex_shader == vertex_shader && b.classification == classification)
    }
}

/// One model's sections.
#[derive(Default)]
pub struct LightModel {
    pub sections: HashMap<u16, LightSection>,
}

/// The layout id of `declaration`: its registers and formats.
pub fn layout_id(declaration: &[Element]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for e in declaration {
        (e.register, e.format).hash(&mut h);
    }
    h.finish()
}

/// The light build of `sections`: each section's parts, and a buffer per
/// (vertex shader, classification) its parts name, built from the
/// declaration `semantics` gives; a vertex shader without a program
/// there, or whose declaration does not gather, has no buffer (its
/// entries are not resident). Why each such was left out, as text.
pub fn light_model(
    sections: &[ModelSection<'_>],
    formats: &StreamFormats,
    semantics: &Semantics<'_>,
) -> (LightModel, Vec<String>) {
    let mut out = LightModel::default();
    let mut why = Vec::new();
    for s in sections {
        let mut list: Vec<u32> = Vec::new();
        let mut parts = HashMap::new();
        let mut shaders = Vec::new();
        for p in &s.parts {
            let start = list.len() as u32;
            let as_list = tags::geometry::list_triangles(p.raw_indices);
            let as_strip = (!p.list).then(|| tags::geometry::strip_triangles(p.raw_indices));
            let authored = as_strip.as_ref().unwrap_or(&as_list);
            list.extend(authored.iter().flatten().map(|&i| u32::from(i)));
            let indices = start..list.len() as u32;
            let list_range = if authored == &as_list {
                indices.clone()
            } else {
                let start = indices.end;
                list.extend(as_list.iter().flatten().map(|&i| u32::from(i)));
                start..list.len() as u32
            };
            parts.insert(
                p.part,
                LightPart {
                    vertex_class: None,
                    first_index: p.first_index,
                    index_count: p.raw_indices.len() as u32,
                    topology: if p.list { Topology::List } else { Topology::Strip },
                    indices,
                    as_list: list_range,
                },
            );
            for &vs in &p.vertex_shaders {
                let key = (vs, p.influences);
                if !shaders.contains(&key) {
                    shaders.push(key);
                }
            }
        }
        if list.is_empty() {
            continue;
        }
        let streams = match s
            .streams
            .iter()
            .map(|t| t.map_or(Ok(Vec::new()), |(kind, _)| formats.elements(kind)))
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(x) => x,
            Err(e) => {
                why.push(format!("section {}: {e}", s.index));
                continue;
            }
        };
        let data: Vec<Option<StreamData<'_>>> =
            s.streams.iter().map(|t| t.map(|(_, d)| d)).collect();
        let mut buffers: Vec<LightBuffer> = Vec::new();
        let mut classes = HashMap::new();
        for (vs, influences) in shaders {
            let Some((classification, sem)) = semantics(vs, s.geometry, influences) else {
                why.push(format!(
                    "section {}: vertex shader {vs:?} has no program for its classification",
                    s.index
                ));
                continue;
            };
            if buffers.iter().any(|b| b.vertex_shader == vs && b.classification == classification) {
                classes.insert((vs, influences), classification);
                continue;
            }
            let decl = declaration(&sem, &streams);
            let vertices = match gather(&decl, &data, s.vertex_count) {
                Ok(v) => v,
                Err(e) => {
                    why.push(format!("section {}: {e}", s.index));
                    continue;
                }
            };
            let regs: Vec<u8> = decl.iter().map(|e| e.register).collect();
            let formats: Vec<_> = decl.iter().map(|e| e.format).collect();
            let layout = ModelLayout {
                id: layout_id(&decl),
                stride: (SLOT * decl.len()) as u64,
                attributes: attributes(&regs, &formats),
            };
            classes.insert((vs, influences), classification);
            buffers.push(LightBuffer {
                vertex_shader: vs,
                classification,
                declaration: decl,
                layout,
                vertices: DeviceBuffer::new(
                    bytemuck::cast_slice(&vertices).to_vec(),
                    wgpu::BufferUsages::VERTEX,
                    "native light model vertices",
                ),
            });
        }
        for part in &s.parts {
            let mut found = part.vertex_shaders.iter()
                .map(|&vs| classes.get(&(vs, part.influences)).copied());
            if let Some(Some(first)) = found.next()
                && found.all(|class| class == Some(first))
                && let Some(held) = parts.get_mut(&part.part)
            {
                held.vertex_class = u16::try_from(first).ok();
            }
        }
        out.sections.insert(
            s.index,
            LightSection {
                decompression: s.decompression,
                parts,
                indices: DeviceBuffer::new(
                    bytemuck::cast_slice(&list).to_vec(),
                    wgpu::BufferUsages::INDEX,
                    "native light model indices",
                ),
                buffers,
            },
        );
    }
    (out, why)
}

/// Every model's light parts, by `mode` datum.
#[derive(Default)]
pub struct LightModels {
    pub models: HashMap<u32, LightModel>,
}

impl LightModels {
    /// `entry`'s section and part, when the entry is an object's whose
    /// raw range matches, and the triangles for its record's topology.
    pub fn part(&self, entry: &LightEntry) -> Option<(&LightSection, Range<u32>)> {
        let section = self
            .models
            .get(&entry.model)?
            .sections
            .get(&entry.stream.section)?;
        let part = section.parts.get(&entry.stream.part)?;
        if part.first_index != entry.stream.first_index || part.index_count != entry.stream.index_count {
            return None;
        }
        let indices = match entry.stream.topology {
            Topology::List => &part.as_list,
            Topology::Strip if part.topology == Topology::Strip => &part.indices,
            _ => return None,
        };
        (!indices.is_empty()).then(|| (section, indices.clone()))
    }

    /// The stream an entry's part is in, for [`crate::light_scene`]: its
    /// model and section.
    pub fn stream(entry: &LightEntry) -> crate::light_scene::StreamId {
        // Below the structure's stream 0: a model datum's index and the
        // section, never 0 for a model with a datum.
        (entry.model & 0xffff) << 9 | u32::from(entry.stream.section & 0x1ff) | 1 << 31
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{InstanceKey, LayerOwner, LightBinding, LightPose, LightStream};

    /// Type 1: a 3-float position (semantic 0), a 2-float coordinate
    /// (semantic 3).
    fn formats() -> StreamFormats {
        let size = crate::object_streams::STREAM_FORMAT_SIZE;
        let mut t = vec![0u8; 2 * size];
        t[size..size + 6].copy_from_slice(&[1, 0, 2, 3, 1, 0xff]);
        StreamFormats::from_bytes(&t)
    }

    fn section(bytes: &[u8]) -> ModelSection<'_> {
        ModelSection {
            index: 2,
            geometry: 0,
            decompression: [[1.0; 4], [0.0; 4], [1.0, 1.0, 0.0, 0.0]],
            streams: vec![Some((1, StreamData { bytes, stride: 20 }))],
            vertex_count: 4,
            parts: vec![
                SectionPart {
                    part: 0,
                    influences: 0,
                    first_index: 0,
                    list: true,
                    raw_indices: &[0, 1, 2, 2, 1, 3],
                    vertex_shaders: vec![TagId(0xe000_0010)],
                },
                SectionPart {
                    part: 1,
                    influences: 0,
                    first_index: 6,
                    list: false,
                    raw_indices: &[0, 1, 2, 3],
                    vertex_shaders: vec![TagId(0xe000_0010), TagId(0xe000_0011)],
                },
            ],
        }
    }

    fn entry(part: u8, first_index: u32, index_count: u32, topology: Topology) -> LightEntry {
        LightEntry {
            slot: 0,
            record: 4,
            owner: LayerOwner::Object(InstanceKey(0xe1e1_0001)),
            model: 0xe100_0007,
            stream: LightStream {
                section: 2,
                part,
                vertex_class: 0,
                first_index,
                index_count,
                topology,
            },
            pose: LightPose::Rows(0..3),
            binding: LightBinding {
                shader: 0xe200_0001,
                selector: 0,
                variant: 0,
                implementation: 0,
            },
        }
    }

    #[test]
    fn weighted_parts_with_the_same_shader_can_select_different_vertex_buffers() {
        let bytes = [0; 80];
        let mut section = section(&bytes);
        section.geometry = 3;
        section.parts[0].influences = 1;
        section.parts[1].influences = 3;
        section.parts[1].part = 23;
        let vs = section.parts[0].vertex_shaders[0];
        section.parts[1].vertex_shaders = vec![vs];
        let semantics = |_, geom, influences| {
            assert_eq!(geom, 3);
            let registers = if influences == 1 { vec![0] } else { vec![0, 3] };
            Some((u32::from(influences) + 2, registers))
        };
        let (model, why) = light_model(&[section], &formats(), &semantics);
        assert!(why.is_empty(), "{why:?}");
        let section = &model.sections[&2];
        assert_eq!(section.parts[&23].indices, 6..12);
        assert_eq!(section.parts[&0].vertex_class, Some(3));
        assert_eq!(section.parts[&23].vertex_class, Some(5));
        assert_eq!(section.buffer(vs, 3).unwrap().provided(), 1);
        assert_eq!(section.buffer(vs, 5).unwrap().provided(), 3);
    }

    #[test]
    fn record_topology_preserves_winding_and_never_crosses_a_part_boundary() {
        let bytes = [0; 80];
        let mut s = section(&bytes);
        let mut adjacent = s.parts[1].clone();
        s.parts[1].raw_indices = &[0, 1, 2, 2, 3, 1];
        adjacent.part = 2;
        adjacent.first_index = 12;
        adjacent.raw_indices = &[0, 0, 1, 2];
        s.parts.push(adjacent);
        let (model, why) = light_model(&[s], &formats(), &|_, _, _| Some((0, vec![0])));
        assert!(why.is_empty(), "{why:?}");
        let mut models = LightModels::default();
        models.models.insert(0xe100_0007, model);
        let values = |part, first, count, topology| {
            let (section, range) = models.part(&entry(part, first, count, topology))?;
            let bytes = &section.indices.bytes[4 * range.start as usize..4 * range.end as usize];
            Some(bytes.chunks_exact(4).map(|b| u32::from_ne_bytes(b.try_into().unwrap())).collect::<Vec<_>>())
        };
        assert_eq!(values(1, 6, 6, Topology::List).unwrap(), [0, 1, 2, 2, 3, 1]);
        assert_eq!(values(1, 6, 6, Topology::Strip).unwrap(), [0, 1, 2, 2, 1, 3]);
        assert_eq!(values(2, 12, 4, Topology::Strip).unwrap(), [0, 2, 1]);
        assert_eq!(values(2, 12, 4, Topology::List), None);
        assert_eq!(values(1, 7, 6, Topology::List), None);
        assert_eq!(values(1, 6, 5, Topology::List), None);
    }

    #[test]
    fn an_unsorted_part_needs_its_own_programs_to_agree_on_a_class() {
        let bytes = [0; 80];
        let section = section(&bytes);
        let semantics = |vs: TagId, _, _| {
            let class = if vs.0 == 0xe000_0010 { 0 } else { 3 };
            Some((class, vec![0]))
        };
        let (model, why) = light_model(&[section], &formats(), &semantics);
        assert!(why.is_empty(), "{why:?}");
        assert_eq!(model.sections[&2].parts[&0].vertex_class, Some(0));
        assert_eq!(model.sections[&2].parts[&1].vertex_class, None);
    }

    #[test]
    fn a_program_missing_from_one_part_cannot_use_its_other_parts_class() {
        let bytes = [0; 80];
        let mut s = section(&bytes);
        s.parts[0].influences = 1;
        s.parts[1].influences = 2;
        let other_shader = s.parts[1].vertex_shaders[1];
        s.parts[0].vertex_shaders.push(other_shader);
        let semantics = |vs, _, influences| {
            if vs == other_shader && influences == 1 {
                return None;
            }
            Some((3, vec![0]))
        };
        let (model, _) = light_model(&[s], &formats(), &semantics);
        let section = &model.sections[&2];
        assert!(section.buffer(other_shader, 3).is_some());
        assert_eq!(section.parts[&0].vertex_class, None);
        assert_eq!(section.parts[&1].vertex_class, Some(3));
    }

    #[test]
    fn a_sections_parts_share_one_index_list_and_a_buffer_per_program() {
        let mut bytes = Vec::new();
        for v in 0..4u32 {
            for x in [v as f32, 0.0, 0.0, 0.5, 0.25] {
                bytes.extend(x.to_le_bytes());
            }
        }
        // 0x10 reads position and coordinate (v0, v1); 0x11 the position
        // only; 0x12 has no program for the section.
        let semantics = |vs: TagId, _: u16, _: u8| match vs.0 {
            0xe000_0010 => Some((0, vec![0u16, 3])),
            0xe000_0011 => Some((0, vec![0u16])),
            _ => None,
        };
        let mut s = section(&bytes);
        s.parts[0].vertex_shaders.push(TagId(0xe000_0012));
        let (model, why) = light_model(&[s], &formats(), &semantics);
        assert_eq!(why.len(), 1, "{why:?}");
        let section = &model.sections[&2];
        assert_eq!(section.indices.len(), 15 * 4, "one list and both forms of the strip");
        assert_eq!(section.parts[&0].indices, 0..6);
        assert_eq!(section.parts[&1].indices, 6..12);
        assert_eq!(section.parts[&1].topology, Topology::Strip);
        let both = section.buffer(TagId(0xe000_0010), 0).expect("built");
        assert_eq!(both.provided(), 0b11);
        assert_eq!(both.layout.stride, 2 * SLOT as u64);
        assert_eq!(both.vertices.len(), 4 * 2 * SLOT);
        let position = section.buffer(TagId(0xe000_0011), 0).expect("built");
        assert_eq!(position.provided(), 0b1);
        assert_ne!(both.layout.id, position.layout.id);
        assert!(
            section.buffer(TagId(0xe000_0010), 1).is_none(),
            "another classification"
        );

        let mut models = LightModels::default();
        models.models.insert(0xe100_0007, model);
        assert!(models.part(&entry(0, 0, 6, Topology::List)).is_some());
        assert!(models.part(&entry(1, 6, 4, Topology::Strip)).is_some());
        let (section, as_list) = models.part(&entry(1, 6, 4, Topology::List)).unwrap();
        assert_eq!(as_list, 12..15);
        // The list ignores the incomplete last triangle and never reads the adjacent part.
        let words: Vec<_> = section.indices.bytes.chunks_exact(4)
            .map(|b| u32::from_ne_bytes(b.try_into().unwrap())).collect();
        assert_eq!(&words[6..12], &[0, 1, 2, 1, 3, 2]);
        assert_eq!(&words[12..15], &[0, 1, 2]);
        assert!(models.part(&entry(0, 0, 6, Topology::Strip)).is_none());
        assert!(models.part(&entry(1, 7, 4, Topology::List)).is_none());
        assert!(models.part(&entry(0, 0, 3, Topology::List)).is_none());
        assert!(models.part(&entry(5, 0, 6, Topology::List)).is_none());
        assert_ne!(
            LightModels::stream(&entry(0, 0, 6, Topology::List)),
            0,
            "never the structure's stream"
        );
    }
}
