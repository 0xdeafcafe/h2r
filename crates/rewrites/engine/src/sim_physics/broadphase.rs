//! The world broadphase batch update's arm (0x2e0350): the prediction of
//! every register and every byte the call writes, compared with the guest's
//! answer and, once `parity` has proven the arm, applied in its place.
//! Each batch owns its endpoint/node arrays. Only array growth, which calls
//! the guest allocator, requires dropping the prediction.

use crate::sim_native::spans::Spans;
use runtime::{CpuAbi, Guest, guest::Access};
use sim_havok::broadphase::{Crossing, Endpoint, Node, Sweep};
use std::collections::BTreeMap;

const NODE_ARRAY: u32 = 0x40;
const AXIS_ARRAYS: [u32; 3] = [0x4c, 0x58, 0x64];
const MARKER_COUNT: u32 = 0x70;
const MARKERS: u32 = 0x78;
// Read all float coefficients through the guest. The bias is added *after*
// clamping and the resulting bits select the integer endpoint.
const QUANTIZE_BIAS: u32 = 0x0041_25c0;
const QUANTIZE_UPPER: u32 = 0x0041_25d0;
const QUANTIZE_LOWER: u32 = 0x0041_25e0;

#[derive(Clone, Copy)]
struct Array {
    header: u32,
    data: u32,
    count: u32,
    capacity: u32,
}

impl Array {
    fn read(g: &Guest, header: u32) -> Option<Self> {
        let data = g.u32(header).ok()?;
        let count = g.u32(header.checked_add(4)?).ok()?;
        let capacity = g.u32(header.checked_add(8)?).ok()? & 0x7fff_ffff;
        (count <= capacity).then_some(Self { header, data, count, capacity })
    }

    fn bytes(self, g: &Guest, width: u32) -> Option<Vec<u8>> {
        // Node and endpoint indices are 16 bits. This also bounds work
        // before reserving memory for an unrecognised guest array.
        if self.count > 1 << 16 { return None; }
        let len = self.count.checked_mul(width)?;
        self.data.checked_add(len)?;
        let mut out = vec![0;len as usize];
        if len != 0 { g.read_bytes(self.data, &mut out, 1).ok()?; }
        Some(out)
    }
}

fn word(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at],bytes[at+1]])
}

fn nodes(bytes: &[u8]) -> Vec<Node> {
    bytes.chunks_exact(16).map(|b| Node {
        lower: [word(b,8),word(b,0),word(b,2)],
        upper: [word(b,10),word(b,4),word(b,6)],
        handle: u32::from_le_bytes(b[12..16].try_into().unwrap()),
    }).collect()
}

fn node_bytes(nodes: &[Node]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nodes.len()*16);
    for node in nodes {
        for value in [node.lower[1],node.lower[2],node.upper[1],node.upper[2],node.lower[0],node.upper[0]] {
            out.extend(value.to_le_bytes());
        }
        out.extend(node.handle.to_le_bytes());
    }
    out
}

fn vector(g: &Guest, p:u32) -> Option<u128> {
    if p & 15 != 0 { return None; }
    Some(u128::from_le_bytes(g.read(p).ok()?))
}

struct Quantizer {
    offsets: [u128;2],
    scale: u128,
    bounds: [u128;2],
    bias: u128,
}

impl Quantizer {
    fn read(g: &Guest, world:u32) -> Option<Self> {
        Some(Self {
            offsets: [vector(g,world.checked_add(0x10)?)?,vector(g,world.checked_add(0x20)?)?],
            scale:vector(g,world.checked_add(0x30)?)?,
            bounds:[vector(g,QUANTIZE_LOWER)?,vector(g,QUANTIZE_UPPER)?],
            bias:vector(g,QUANTIZE_BIAS)?,
        })
    }

    fn corner(&self, input:u128, high:bool, mxcsr:&mut u32) -> Option<(u128,[u16;3])> {
        let shifted = simd::addps(mxcsr,input,self.offsets[high as usize]).ok()?;
        let scaled = simd::mulps(mxcsr,shifted,self.scale).ok()?;
        let capped = simd::minps(mxcsr,scaled,self.bounds[1]).ok()?;
        let floored = simd::maxps(mxcsr,capped,self.bounds[0]).ok()?;
        let encoded = simd::addps(mxcsr,floored,self.bias).ok()?;
        let ends = std::array::from_fn(|i| {
            let coord = (encoded >> (32*i+7)) as u16;
            if high { coord | 1 } else { coord & !1 }
        });
        Some((encoded,ends))
    }
}

struct Marker {
    array: Array,
    len: usize,
    // Keep words beyond the logical end after swap-remove. They are still
    // in guest memory and may have been written earlier in the same batch.
    words: Vec<u16>,
}

impl Marker {
    fn read(g:&Guest, header:u32) -> Option<Self> {
        let array = Array::read(g,header)?;
        let words = array.bytes(g,2)?.chunks_exact(2).map(|b| word(b,0)).collect();
        Some(Self { array, len:array.count as usize, words })
    }

    fn update(&mut self, added:bool, node:u16) -> Option<()> {
        if added {
            if self.len >= self.array.capacity as usize { return None; }
            if self.len == self.words.len() { self.words.push(node); } else { self.words[self.len] = node; }
            self.len += 1;
        } else {
            let at = self.words[..self.len].iter().position(|&word| word == node)?;
            self.len -= 1;
            self.words[at] = self.words[self.len];
        }
        Some(())
    }
}

pub struct Prediction {
    cpu: CpuAbi,
    spans: Vec<(u32, Vec<u8>)>,
}

impl Prediction {
    /// The differential tests' check: the guest's answer equals this one.
    #[cfg(test)]
    pub fn matches(&self,cpu:&CpuAbi,g:&Guest) -> bool {
        let mut expected = self.cpu;
        expected.eip = cpu.eip; // The hook dispatcher owns the return PC.
        if &expected != cpu { return false; }
        self.spans.iter().all(|(start,bytes)| {
            let mut observed = vec![0;bytes.len()];
            g.read_bytes(*start, &mut observed, 1).is_ok() && observed == *bytes
        })
    }

    /// Answer the call in the guest's place: every predicted span written,
    /// every register the prediction holds (the whole `CpuAbi` but `eip`,
    /// which the dispatcher owns). Refused, with nothing written, when a
    /// span is not plain writable RAM (an access trap, a read-only page;
    /// pages are looked up one by one) or when two spans share
    /// physical bytes, whose final value would then depend on the write
    /// order the prediction does not model.
    pub fn apply(&self, cpu: &mut CpuAbi, g: &mut Guest) -> bool {
        let mut physical: Vec<Spans> = Vec::with_capacity(self.spans.len());
        for (at, bytes) in &self.spans {
            // One lookup per page: a span can cross page mappings.
            let Some(range) = Spans::checked(&*g, *at, bytes.len(), Access::Write, 1) else { return false };
            if physical.iter().any(|r| r.overlaps(&range)) {
                return false;
            }
            physical.push(range);
        }
        for (at, bytes) in &self.spans {
            if g.write_bytes(*at, bytes, 1).is_err() {
                // Checked writable above; unreachable short of a racing remap.
                return false;
            }
        }
        let eip = cpu.eip;
        *cpu = self.cpu;
        cpu.eip = eip;
        true
    }

    fn span(&mut self,at:u32, bytes:Vec<u8>) -> Option<()> {
        at.checked_add(u32::try_from(bytes.len()).ok()?)?;
        if !bytes.is_empty() { self.spans.push((at,bytes)); }
        Some(())
    }

    fn word(&mut self,at:u32, value:u32) -> Option<()> {
        self.span(at,value.to_le_bytes().to_vec())
    }
}

pub fn capture(cpu:&CpuAbi, g:&Guest) -> Option<Prediction> {
    let stack = cpu.esp;
    let handles = g.u32(stack.checked_add(4)?).ok()?;
    let boxes = g.u32(stack.checked_add(8)?).ok()?;
    let n = g.u32(stack.checked_add(12)?).ok()?;
    let end = handles.checked_add(n.checked_mul(4)?)?;
    let mut answer = Prediction { cpu:*cpu, spans:Vec::new() };
    answer.cpu.esp = stack.checked_add(24)?;
    answer.cpu.eax = end;
    answer.cpu.ecx = end;
    super::arithmetic(&mut answer.cpu,end,end,true);
    if n == 0 { return Some(answer); }
    if n > 1 << 16 { return None; }

    let world = cpu.ecx;
    let node_array = Array::read(g,world.checked_add(NODE_ARRAY)?)?;
    let node_rows = nodes(&node_array.bytes(g,16)?);
    let mut axes = [Vec::new(),Vec::new(),Vec::new()];
    let mut axis_data = [0;3];
    for (i,offset) in AXIS_ARRAYS.into_iter().enumerate() {
        let a = Array::read(g,world.checked_add(offset)?)?;
        axis_data[i] = a.data;
        axes[i] = a.bytes(g,4)?.chunks_exact(4).map(|b| Endpoint { value:word(b,0), node:word(b,2) }).collect();
    }
    let mut sweep = Sweep { axes, nodes:node_rows };
    let added = Array::read(g,g.u32(stack.checked_add(16)?).ok()?)?;
    let removed = Array::read(g,g.u32(stack.checked_add(20)?).ok()?)?;
    let arrays = [removed,added];
    let mut pairs = [Vec::new(),Vec::new()];
    let mut markers: BTreeMap<u32,Marker> = BTreeMap::new();
    let quantizer = Quantizer::read(g,world)?;
    let mut aabb = boxes;
    let mut upper_bits = 0;
    for i in 0..n {
        let handle = g.u32(handles.checked_add(4*i)?).ok()?;
        let node = u16::try_from(g.u32(handle).ok()?).ok()?;
        let (_,lower) = quantizer.corner(vector(g,aabb)?,false,&mut answer.cpu.mxcsr)?;
        let (encoded,upper) = quantizer.corner(vector(g,aabb.checked_add(16)?)?,true,&mut answer.cpu.mxcsr)?;
        upper_bits = encoded;
        aabb = aabb.checked_add(32)?;
        sweep.update(node,lower,upper, |crossing| match crossing {
            Crossing::Pair { added, handles } => {
                let (array,list) = (arrays[added as usize], &mut pairs[added as usize]);
                if list.len()/8 >= (array.capacity-array.count) as usize { return None; }
                for value in handles { list.extend(value.to_le_bytes()); }
                Some(())
            }
            Crossing::Marker { added, offset, node } => {
                use std::collections::btree_map::Entry;
                let marker = match markers.entry(offset) {
                    Entry::Occupied(o) => o.into_mut(),
                    Entry::Vacant(v) => {
                        if offset % 16 != 0 || offset/16 >= g.u32(world.checked_add(MARKER_COUNT)?).ok()? { return None; }
                        let p = g.u32(world.checked_add(MARKERS)?).ok()?.checked_add(offset)?;
                        v.insert(Marker::read(g,p.checked_add(4)?)?)
                    }
                };
                marker.update(added,node)
            }
        })?;
    }

    answer.cpu.edx = removed.header;
    for (dst,value) in answer.cpu.xmm.iter_mut().zip([quantizer.scale,quantizer.bias,quantizer.bounds[0],quantizer.bounds[1],quantizer.offsets[1],upper_bits]) {
        *dst = value.to_le_bytes();
    }
    answer.word(stack.checked_add(8)?,aabb)?;
    answer.span(node_array.data,node_bytes(&sweep.nodes))?;
    for (base,ends) in axis_data.into_iter().zip(sweep.axes) {
        let mut bytes = Vec::with_capacity(4*ends.len());
        for end in ends { bytes.extend(end.value.to_le_bytes()); bytes.extend(end.node.to_le_bytes()); }
        answer.span(base,bytes)?;
    }
    for (array,list) in arrays.into_iter().zip(pairs) {
        let count = array.count.checked_add(u32::try_from(list.len()/8).ok()?)?;
        answer.word(array.header.checked_add(4)?,count)?;
        answer.span(array.data.checked_add(array.count.checked_mul(8)?)?,list)?;
    }
    for marker in markers.into_values() {
        answer.word(marker.array.header.checked_add(4)?,marker.len as u32)?;
        answer.span(marker.array.data,marker.words.into_iter().flat_map(u16::to_le_bytes).collect())?;
    }
    Some(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const STACK: u32 = 0x0020_0000;
    const HANDLES: u32 = 0x0035_0000;
    const BOXES: u32 = 0x0036_0000;
    /// The moved object: the batch lists its address, its first word is the node id.
    const OBJECT: u32 = 0x0035_1000;
    /// The removed and added pair arrays' data, in `Array::bytes` order.
    const PAIRS: [u32; 2] = [0x0037_1000, 0x0037_0000];
    const ADDED: u32 = 0x0038_1000;
    const REMOVED: u32 = 0x0038_0000;
    const WORLD: u32 = 0x0030_0000;
    const NODES: u32 = 0x0031_0000;
    const AXES: [u32; 3] = [0x0032_0000, 0x0033_0000, 0x0034_0000];

    /// A f32 whose quantized endpoint is `end`: bits 7..23 carry it.
    fn coord(end: u16) -> u32 {
        u32::from(end) << 7
    }

    fn vector(g: &mut Guest, at: u32, lanes: [u32; 4]) {
        let packed: u128 = lanes.iter().rev().fold(0, |p, &w| p << 32 | u128::from(w));
        g.write_bytes(at, &packed.to_le_bytes(), 1).unwrap();
    }

    fn node(g: &mut Guest, i: usize, lower: [u16; 3], upper: [u16; 3], handle: u32) {
        let at = NODES + 16 * i as u32;
        for (word, value) in [
            (at, lower[1]),
            (at + 2, lower[2]),
            (at + 4, upper[1]),
            (at + 6, upper[2]),
            (at + 8, lower[0]),
            (at + 10, upper[0]),
        ] {
            g.write_bytes(word, &value.to_le_bytes(), 1).unwrap();
        }
        g.write_bytes(at + 12, &handle.to_le_bytes(), 1).unwrap();
    }

    /// One axis array: its header at `W + 0x4c + 0xc*i`, its endpoints at
    /// `AXES[i]`, each four bytes (value, node).
    fn axis(g: &mut Guest, i: usize, ends: &[(u16, u16)]) {
        let at = AXES[i];
        for (slot, &(value, node)) in ends.iter().enumerate() {
            g.write_bytes(at + 4 * slot as u32, &value.to_le_bytes(), 1).unwrap();
            g.write_bytes(at + 4 * slot as u32 + 2, &node.to_le_bytes(), 1).unwrap();
        }
        let header = WORLD + 0x4c + 0xc * i as u32;
        g.set_u32(header, at).unwrap();
        g.set_u32(header + 4, ends.len() as u32).unwrap();
        g.set_u32(header + 8, 16).unwrap();
    }

    fn guest() -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0x0020_0000, 0x1000, Permissions::RW).unwrap();
        g.map_zeroed(0x0030_0000, 0x9_0000, Permissions::RW).unwrap();
        g.map_zeroed(0x0041_2000, 0x1000, Permissions::RW).unwrap();
        // The quantizer: no offset, unit scale, unbounded clamp, no bias.
        vector(&mut g, WORLD + 0x10, [0; 4]);
        vector(&mut g, WORLD + 0x20, [0; 4]);
        vector(&mut g, WORLD + 0x30, [1f32.to_bits(); 4]);
        vector(&mut g, QUANTIZE_BIAS, [0; 4]);
        vector(&mut g, QUANTIZE_UPPER, [f32::INFINITY.to_bits(); 4]);
        vector(&mut g, QUANTIZE_LOWER, [f32::NEG_INFINITY.to_bits(); 4]);
        g.set_u32(WORLD + NODE_ARRAY, NODES).unwrap();
        g.set_u32(WORLD + NODE_ARRAY + 4, 4).unwrap();
        g.set_u32(WORLD + NODE_ARRAY + 8, 8).unwrap();
        // Node 1 spans 2..5 everywhere; nodes 2 and 3 sit beyond it on X
        // and share its span on Y and Z, so a move on X crosses both.
        let x: &[(u16, u16)] = &[(0, 0), (2, 1), (5, 1), (6, 2), (9, 2), (10, 3), (13, 3), (99, 0)];
        let flat: &[(u16, u16)] = &[(0, 0), (2, 1), (2, 2), (2, 3), (5, 1), (5, 2), (5, 3), (99, 0)];
        let axes = [x, flat, flat];
        for (i, ends) in axes.iter().enumerate() {
            axis(&mut g, i, ends);
        }
        // A node row holds the *slots* of its endpoints in each axis array,
        // not their values (`sim_havok::broadphase::Node`). Odd values are
        // upper ends.
        for id in 0..4u16 {
            let slot = |a: usize, upper: bool| {
                let at = axes[a].iter().position(|&(v, n)| n == id && (v & 1 != 0) == upper).unwrap();
                at as u16
            };
            let lower = std::array::from_fn(|a| slot(a, false));
            let upper = std::array::from_fn(|a| slot(a, true));
            node(&mut g, usize::from(id), lower, upper, u32::from(id) * 8);
        }
        for (at, data) in [(ADDED, PAIRS[1]), (REMOVED, PAIRS[0])] {
            g.set_u32(at, data).unwrap();
            g.set_u32(at + 4, 0).unwrap();
            g.set_u32(at + 8, 16).unwrap();
        }
        g
    }

    fn cpu(g: &mut Guest, n: u32, min: [u16; 3], max: [u16; 3]) -> CpuAbi {
        g.set_u32(STACK + 4, HANDLES).unwrap();
        g.set_u32(STACK + 8, BOXES).unwrap();
        g.set_u32(STACK + 12, n).unwrap();
        g.set_u32(STACK + 16, ADDED).unwrap();
        g.set_u32(STACK + 20, REMOVED).unwrap();
        g.set_u32(HANDLES, OBJECT).unwrap();
        g.set_u32(OBJECT, 1).unwrap(); // the batch moves node 1
        vector(g, BOXES, [coord(min[0]), coord(min[1]), coord(min[2]), 0]);
        vector(g, BOXES + 16, [coord(max[0]), coord(max[1]), coord(max[2]), 0]);
        CpuAbi { ecx: WORLD, esp: STACK, ..CpuAbi::default() }
    }

    /// The guest performs the predicted writes and returns with the
    /// predicted registers; one flipped byte then breaks the comparison.
    fn round_trips(n: u32, min: [u16; 3], max: [u16; 3], pairs: u32) {
        let mut g = guest();
        let c = cpu(&mut g, n, min, max);
        let prediction = capture(&c, &g).unwrap();
        let end = HANDLES + 4 * n;
        assert_eq!((prediction.cpu.eax, prediction.cpu.ecx, prediction.cpu.edx), (end, end, REMOVED));
        assert_eq!(prediction.cpu.esp, STACK + 24);
        for (at, bytes) in &prediction.spans {
            g.write_bytes(*at, bytes, 1).unwrap();
        }
        for side in [REMOVED, ADDED] {
            assert_eq!(g.u32(side + 4).unwrap(), pairs, "{side:#x}");
        }
        let mut after = prediction.cpu;
        after.eip = 0x999;
        assert!(prediction.matches(&after, &g));
        let (at, bytes) = prediction.spans.last().unwrap();
        g.write_bytes(*at, &[bytes[0] ^ 1], 1).unwrap();
        assert!(!prediction.matches(&after, &g));
    }

    #[test]
    fn a_still_batch_predicts_no_pair_writes() {
        round_trips(1, [2, 2, 2], [5, 5, 5], 0);
    }

    #[test]
    fn a_moved_box_predicts_both_pair_lists_in_crossing_order() {
        // Node 1 jumps past nodes 2 and 3 on X: both pairs are added at the
        // new place and removed at the old one, in crossing order.
        round_trips(1, [14, 2, 2], [17, 5, 5], 2);
        let mut g = guest();
        let c = cpu(&mut g, 1, [14, 2, 2], [17, 5, 5]);
        let prediction = capture(&c, &g).unwrap();
        let expected: Vec<u8> = [8u32, 16, 8, 24].into_iter().flat_map(u32::to_le_bytes).collect();
        for at in PAIRS {
            let bytes = prediction.spans.iter().find(|(start, _)| *start == at).unwrap();
            assert_eq!(bytes.1, expected, "{at:#x}");
        }
    }

    /// Applied, the prediction leaves the bytes and registers the guest's
    /// own answer leaves: the pair lists in crossing order, the popped
    /// frame, and `eip` untouched.
    #[test]
    fn an_applied_prediction_leaves_the_guests_answer() {
        let mut g = guest();
        let entry = cpu(&mut g, 1, [14, 2, 2], [17, 5, 5]);
        let prediction = capture(&entry, &g).unwrap();
        // The guest's answer, as the comparison sees it at the return.
        let mut by_guest = guest();
        cpu(&mut by_guest, 1, [14, 2, 2], [17, 5, 5]);
        for (at, bytes) in &prediction.spans {
            by_guest.write_bytes(*at, bytes, 1).unwrap();
        }
        let mut applied = CpuAbi { eip: 0x2e_0350, ..entry };
        assert!(prediction.apply(&mut applied, &mut g));
        assert_eq!(applied.eip, 0x2e_0350, "the dispatcher owns the return");
        assert_eq!((applied.esp, applied.eax, applied.edx), (STACK + 24, HANDLES + 4, REMOVED));
        let mut expected = prediction.cpu;
        expected.eip = applied.eip;
        assert!(expected == applied);
        assert!(prediction.matches(&applied, &g));
        let expected: Vec<u8> = [8u32, 16, 8, 24].into_iter().flat_map(u32::to_le_bytes).collect();
        for at in PAIRS {
            let mut got = vec![0; expected.len()];
            g.read_bytes(at, &mut got, 1).unwrap();
            assert_eq!(got, expected, "{at:#x}");
        }
        for (start, len) in [(0x0020_0000, 0x1000), (0x0030_0000, 0x9_0000)] {
            let (mut a, mut b) = (vec![0; len], vec![0; len]);
            g.read_bytes(start, &mut a, 1).unwrap();
            by_guest.read_bytes(start, &mut b, 1).unwrap();
            assert!(a == b, "{start:#x}: the applied bytes differ from the guest's");
        }
    }

    /// Two spans on the same physical bytes, or one on read-only memory:
    /// the apply refuses and writes nothing.
    #[test]
    fn an_apply_refuses_aliased_or_unwritable_spans_untouched() {
        let mut g = guest();
        let entry = cpu(&mut g, 1, [14, 2, 2], [17, 5, 5]);
        let mut prediction = capture(&entry, &g).unwrap();
        let (at, bytes) = prediction.spans[0].clone();
        prediction.spans.push((at, bytes.iter().map(|b| b ^ 0xff).collect()));
        let before = g.u32(STACK + 8).unwrap();
        let mut c = entry;
        assert!(!prediction.apply(&mut c, &mut g));
        assert!(c == entry);
        assert_eq!(g.u32(STACK + 8).unwrap(), before);

        let mut g = guest();
        let entry = cpu(&mut g, 1, [14, 2, 2], [17, 5, 5]);
        let mut prediction = capture(&entry, &g).unwrap();
        g.map_zeroed(0x0050_0000, 0x1000, Permissions::READ).unwrap();
        prediction.spans.push((0x0050_0000, vec![1; 4]));
        let mut c = entry;
        assert!(!prediction.apply(&mut c, &mut g));
        assert!(c == entry);
        assert_eq!(g.u32(STACK + 8).unwrap(), before);
    }

    #[test]
    fn an_empty_batch_and_a_misaligned_box_decline_cleanly() {
        let mut g = guest();
        let c = cpu(&mut g, 0, [2, 2, 2], [5, 5, 5]);
        let prediction = capture(&c, &g).unwrap();
        assert_eq!(prediction.spans.len(), 0);
        let mut after = prediction.cpu;
        after.eip = 0x999;
        assert!(prediction.matches(&after, &g));
        // The first box must be vector-aligned.
        g.set_u32(STACK + 12, 1).unwrap();
        g.set_u32(STACK + 8, BOXES + 8).unwrap();
        assert!(capture(&c, &g).is_none());
        // A node id the sweep does not know.
        g.set_u32(STACK + 8, BOXES).unwrap();
        g.set_u32(OBJECT, 9).unwrap();
        assert!(capture(&c, &g).is_none());
    }
}
