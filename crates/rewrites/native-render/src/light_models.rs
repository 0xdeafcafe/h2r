//! Model parts for the light passes, built at load
//! (docs/graphics/native-lights.md, "Gathered groups in the scene"): what
//! a light layer drawn from its scene group alone needs to draw an object
//! entry without the guest's geometry handler (`0x0004d0b0`).
//!
//! Per model section: its parts' triangles in one index list (the part's
//! raw range decoded in its own topology, as the object path decodes it,
//! `tags::geometry::Geometry::part_triangles`), its decompression rows,
//! and its vertices once per (vertex shader, classification) its parts'
//! materials' layer-12 and layer-13 implementations name, gathered
//! through the declaration that program's input semantics give over the
//! section's streams ([`crate::object_streams::declaration`],
//! [`crate::object_streams::gather`]). The pass itself is compiled per
//! light inputs at run time with that declaration's registers and fetch
//! ([`crate::light_look::compile_light_model`]).
//!
//! An entry finds its part by its model, section and part
//! ([`LightModels::part`]); its raw range and topology must be the
//! part's, and its sorted vertex classification one a buffer was built
//! for ([`LightSection::buffer`]). Anything else is not resident, and the
//! layer keeps its handlers (`light_scene::Refusal::Stream`).

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;

use frame_scene::{LightEntry, Row, Topology};
use map::TagId;

use crate::light_gpu::ModelLayout;
use crate::object_program_gpu::attributes;
use crate::object_streams::{Element, SLOT, StreamData, StreamFormats, declaration, gather, registers};

/// One mode section as the light build reads it.
#[derive(Clone)]
pub struct ModelSection<'a> {
    pub index: u16,
    /// Its geometry classification (`+0x18`) and node influences
    /// (`+0x14`), which pick a program's classification
    /// ([`crate::object_streams::classification`]).
    pub geometry: u16,
    pub influences: u8,
    pub decompression: [Row; 3],
    /// Its streams by descriptor slot: (stream type, bytes).
    pub streams: Vec<Option<(u8, StreamData<'a>)>>,
    pub vertex_count: usize,
    pub parts: Vec<SectionPart>,
}

/// One part of a section: its index among the section's parts, its raw
/// index range, whether that range is a list, its triangles, and the
/// vertex shaders its material's lit passes name.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionPart {
    pub part: u8,
    pub first_index: u32,
    pub index_count: u32,
    pub list: bool,
    pub triangles: Vec<[u16; 3]>,
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
        Self { bytes, usage, label, held: Mutex::new(None) }
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
    pub first_index: u32,
    pub index_count: u32,
    pub topology: Topology,
    /// Its triangles in the section's index list.
    pub indices: Range<u32>,
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
        self.buffers.iter().find(|b| b.vertex_shader == vertex_shader && b.classification == classification)
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
pub fn light_model(sections: &[ModelSection<'_>], formats: &StreamFormats, semantics: &Semantics<'_>) -> (LightModel, Vec<String>) {
    let mut out = LightModel::default();
    let mut why = Vec::new();
    for s in sections {
        let mut list: Vec<u32> = Vec::new();
        let mut parts = HashMap::new();
        let mut shaders: Vec<TagId> = Vec::new();
        for p in &s.parts {
            let start = list.len() as u32;
            list.extend(p.triangles.iter().flatten().map(|&i| u32::from(i)));
            parts.insert(p.part, LightPart {
                first_index: p.first_index,
                index_count: p.index_count,
                topology: if p.list { Topology::List } else { Topology::Strip },
                indices: start..list.len() as u32,
            });
            for vs in &p.vertex_shaders {
                if !shaders.contains(vs) {
                    shaders.push(*vs);
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
        let data: Vec<Option<StreamData<'_>>> = s.streams.iter().map(|t| t.map(|(_, d)| d)).collect();
        let mut buffers: Vec<LightBuffer> = Vec::new();
        for vs in shaders {
            let Some((classification, sem)) = semantics(vs, s.geometry, s.influences) else {
                why.push(format!("section {}: vertex shader {vs:?} has no program for its classification", s.index));
                continue;
            };
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
            let layout = ModelLayout { id: layout_id(&decl), stride: (SLOT * decl.len()) as u64, attributes: attributes(&regs, &formats) };
            buffers.push(LightBuffer {
                vertex_shader: vs,
                classification,
                declaration: decl,
                layout,
                vertices: DeviceBuffer::new(bytemuck::cast_slice(&vertices).to_vec(), wgpu::BufferUsages::VERTEX, "native light model vertices"),
            });
        }
        out.sections.insert(s.index, LightSection {
            decompression: s.decompression,
            parts,
            indices: DeviceBuffer::new(bytemuck::cast_slice(&list).to_vec(), wgpu::BufferUsages::INDEX, "native light model indices"),
            buffers,
        });
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
    /// raw range and topology are the part's.
    pub fn part(&self, entry: &LightEntry) -> Option<(&LightSection, &LightPart)> {
        let section = self.models.get(&entry.model)?.sections.get(&entry.stream.section)?;
        let part = section.parts.get(&entry.stream.part)?;
        (part.first_index == entry.stream.first_index
            && part.index_count == entry.stream.index_count
            && part.topology == entry.stream.topology
            && !part.indices.is_empty())
        .then_some((section, part))
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
            influences: 0,
            decompression: [[1.0; 4], [0.0; 4], [1.0, 1.0, 0.0, 0.0]],
            streams: vec![Some((1, StreamData { bytes, stride: 20 }))],
            vertex_count: 4,
            parts: vec![
                SectionPart {
                    part: 0,
                    first_index: 0,
                    index_count: 6,
                    list: true,
                    triangles: vec![[0, 1, 2], [2, 1, 3]],
                    vertex_shaders: vec![TagId(0xe000_0010)],
                },
                SectionPart {
                    part: 1,
                    first_index: 6,
                    index_count: 4,
                    list: false,
                    triangles: vec![[0, 1, 2], [1, 3, 2]],
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
            stream: LightStream { section: 2, part, vertex_class: 0, first_index, index_count, topology },
            pose: LightPose::Rows(0..3),
            binding: LightBinding { shader: 0xe200_0001, selector: 0, variant: 0, implementation: 0 },
        }
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
        assert_eq!(section.indices.len(), 12 * 4, "both parts' triangles");
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
        assert!(section.buffer(TagId(0xe000_0010), 1).is_none(), "another classification");

        let mut models = LightModels::default();
        models.models.insert(0xe100_0007, model);
        assert!(models.part(&entry(0, 0, 6, Topology::List)).is_some());
        assert!(models.part(&entry(1, 6, 4, Topology::Strip)).is_some());
        // Drawn as a list by its record, or another range: not this part.
        assert!(models.part(&entry(1, 6, 4, Topology::List)).is_none());
        assert!(models.part(&entry(0, 0, 3, Topology::List)).is_none());
        assert!(models.part(&entry(5, 0, 6, Topology::List)).is_none());
        assert_ne!(LightModels::stream(&entry(0, 0, 6, Topology::List)), 0, "never the structure's stream");
    }
}
