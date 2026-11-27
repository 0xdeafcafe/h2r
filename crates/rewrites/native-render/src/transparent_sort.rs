//! The back-to-front sort of the transparent items on the GPU
//! (docs/graphics/native-transparents.md, "The sort on the GPU").
//!
//! The guest sorts the live entries by `(class, depth, tie)` with
//! `0x0013df80` (comparator `0x0001e700`; the Rust twin is
//! `render_bridge::native_scene_lists::transparents`). Here the same key
//! is formed as three unsigned words and sorted by a stable least
//! significant bit radix sort in one compute workgroup
//! (`transparent_sort.wgsl`). Only the bits that differ between items are
//! passed, so a list whose depths are all different in the low mantissa
//! costs about as many passes as it has varying bits.
//!
//! The result is the item indices in ascending key order, equal keys in
//! input order, and a flag when any two neighbours are equal. The guest's
//! quicksort leaves equal keys in an order of its own, so a caller that
//! must match it falls back to the CPU sort when [`Sorted::tied`] is set,
//! and for the items [`form_keys`] refuses: those with a plane (the plane
//! comparator is not a total order), a non-finite depth, or a class or tie
//! whose guest subtraction could wrap.

/// The most items one sort takes (the guest list's capacity, `0x400`).
pub const CAPACITY: usize = 1024;

/// One entry's key as the guest holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SortItem {
    /// `+0`, the sort class.
    pub class: i32,
    /// `+4`, the view depth.
    pub depth: f32,
    /// `+8`, the tie word.
    pub tie: i32,
    /// `+0xc` is set: the entry compares against a plane.
    pub plane: bool,
}

/// Why the GPU does not sort a list; the metric is `scene_sort_gpu_refused_<cause>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// An item with a plane.
    Plane,
    /// A NaN or infinite depth.
    NonFinite,
    /// A class or tie so large the guest's wrapping subtraction could differ from the compare.
    Range,
    /// More items than [`CAPACITY`].
    Count,
    /// Two items have the same key and the guest's order of them is its own.
    Ties,
}

impl Refusal {
    /// The counter this cause is counted under.
    pub const fn metric(self) -> &'static str {
        match self {
            Self::Plane => "scene_sort_gpu_refused_plane",
            Self::NonFinite => "scene_sort_gpu_refused_non_finite",
            Self::Range => "scene_sort_gpu_refused_range",
            Self::Count => "scene_sort_gpu_refused_count",
            Self::Ties => "scene_sort_gpu_refused_ties",
        }
    }
}

/// Keys as the shader reads them, and the bit passes that matter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    /// Per item `[tie, depth, class, 0]`, each ascending as an unsigned word.
    pub words: Vec<[u32; 4]>,
    /// `(word, bit)` pairs, least significant first, that differ between items.
    pub passes: Vec<[u32; 2]>,
}

const BIAS: u32 = 0x8000_0000;
/// A guest compare is `a.wrapping_sub(b) > 0`; inside this range it is `a > b`.
const SAFE: i32 = 1 << 30;

/// The unsigned word whose order is the float's `>` order (`-0.0` is `0.0`).
fn depth_word(depth: f32) -> u32 {
    let bits = if depth == 0.0 { 0 } else { depth.to_bits() };
    if bits & BIAS != 0 { !bits } else { bits | BIAS }
}

/// Forms the sort keys of `items`, or says why it cannot do so exactly.
pub fn form_keys(items: &[SortItem]) -> Result<Keys, Refusal> {
    if items.len() > CAPACITY {
        return Err(Refusal::Count);
    }
    let mut words = Vec::with_capacity(items.len());
    for item in items {
        if item.plane {
            return Err(Refusal::Plane);
        }
        if !item.depth.is_finite() {
            return Err(Refusal::NonFinite);
        }
        if item.class.unsigned_abs() >= SAFE as u32 || item.tie.unsigned_abs() >= SAFE as u32 {
            return Err(Refusal::Range);
        }
        words.push([item.tie as u32 ^ BIAS, depth_word(item.depth), item.class as u32 ^ BIAS, 0]);
    }
    let mut passes = Vec::new();
    if let Some(first) = words.first() {
        for word in 0..3 {
            let varying = words.iter().fold(0, |acc, w| acc | (w[word] ^ first[word]));
            passes.extend((0..32).filter(|bit| varying >> bit & 1 != 0).map(|bit| [word as u32, bit]));
        }
    }
    Ok(Keys { words, passes })
}

/// The sort's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sorted {
    /// Item indices, ascending by key, equal keys in input order.
    pub order: Vec<u32>,
    /// Two neighbours in `order` have the same key.
    pub tied: bool,
}

#[cfg(feature = "gpu")]
pub use device::GpuSort;

#[cfg(feature = "gpu")]
mod device {
    use super::{Keys, Sorted};
    use bytemuck::{Pod, Zeroable};
    use wgpu::util::DeviceExt;

    #[repr(C)]
    #[derive(Clone, Copy, Pod, Zeroable)]
    struct Params {
        n: u32,
        passes: u32,
        pad: [u32; 2],
    }

    /// The sort's pipeline.
    pub struct GpuSort {
        pipeline: wgpu::ComputePipeline,
    }

    impl GpuSort {
        pub fn new(device: &wgpu::Device) -> Self {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native transparent sort"),
                source: wgpu::ShaderSource::Wgsl(include_str!("transparent_sort.wgsl").into()),
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("native transparent sort"),
                layout: None,
                module: &module,
                entry_point: Some("sort"),
                compilation_options: Default::default(),
                cache: None,
            });
            Self { pipeline }
        }

        /// Records the sort of `keys` and returns the buffer `order` holds on
        /// the GPU: `n` item indices, then the tie flag. The renderer's
        /// indirect draws read it in place.
        pub fn encode(&self, device: &wgpu::Device, enc: &mut wgpu::CommandEncoder, keys: &Keys) -> wgpu::Buffer {
            let n = keys.words.len();
            let storage = |label, bytes: &[u8]| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    // A binding cannot be empty.
                    contents: if bytes.is_empty() { &[0; 16] } else { bytes },
                    usage: wgpu::BufferUsages::STORAGE,
                })
            };
            let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("native transparent sort params"),
                contents: bytemuck::bytes_of(&Params { n: n as u32, passes: keys.passes.len() as u32, pad: [0; 2] }),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let words = storage("native transparent sort keys", bytemuck::cast_slice(&keys.words));
            let bits = storage("native transparent sort bits", bytemuck::cast_slice(&keys.passes));
            let order = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("native transparent sort order"),
                size: (n as u64 + 1) * 4,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: words.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: bits.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: order.as_entire_binding() },
                ],
            });
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
            drop(pass);
            order
        }

        /// Sorts and reads the answer back (tests, and the shadow compare).
        pub fn sort(&self, device: &wgpu::Device, queue: &wgpu::Queue, keys: &Keys) -> Sorted {
            let n = keys.words.len();
            let mut enc = device.create_command_encoder(&Default::default());
            let order = self.encode(device, &mut enc, keys);
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: order.size(),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            enc.copy_buffer_to_buffer(&order, 0, &read, 0, order.size());
            queue.submit([enc.finish()]);
            read.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let words: Vec<u32> = bytemuck::cast_slice(&read.slice(..).get_mapped_range().unwrap()).to_vec();
            Sorted { order: words[..n].to_vec(), tied: words[n] != 0 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(class: i32, depth: f32) -> SortItem {
        SortItem { class, depth, tie: 0, plane: false }
    }

    #[test]
    fn depth_words_order_like_the_floats_and_zero_signs_tie() {
        let v = [-1e30, -2.5, -0.0, 0.0, 1e-30, 2.5, 1e30];
        for a in v {
            for b in v {
                assert_eq!(depth_word(a) < depth_word(b), a < b, "{a} {b}");
                assert_eq!(depth_word(a) == depth_word(b), a == b, "{a} {b}");
            }
        }
    }

    #[test]
    fn items_the_key_cannot_hold_are_refused_by_cause() {
        let plane = SortItem { plane: true, ..item(0, 1.0) };
        assert_eq!(form_keys(&[plane]), Err(Refusal::Plane));
        assert_eq!(form_keys(&[item(0, f32::NAN)]), Err(Refusal::NonFinite));
        assert_eq!(form_keys(&[item(i32::MAX, 1.0)]), Err(Refusal::Range));
        assert_eq!(form_keys(&vec![item(0, 1.0); CAPACITY + 1]), Err(Refusal::Count));
        assert_eq!(Refusal::Ties.metric(), "scene_sort_gpu_refused_ties");
    }

    #[test]
    fn only_varying_bits_become_passes() {
        assert!(form_keys(&[item(1, 2.0); 3]).unwrap().passes.is_empty());
        let keys = form_keys(&[item(1, 2.0), item(2, 2.0)]).unwrap();
        assert!(keys.passes.iter().all(|p| p[0] == 2));
    }
}
