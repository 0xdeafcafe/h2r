//! Layer-3 object dispatches after the guest has selected render records
//! and sorted the layer. See docs/graphics/native-renderer.md.

use std::collections::HashMap;
use std::ops::Range;

use crate::GuestRead;
use crate::layout::object_nodes::{MODEL_RECORD, MODEL_SECTIONS, Row, SECTION_STRIDE, TAG_INSTANCES};
use crate::layout::objects::{self, ItemKey, RecordOwner};
use crate::layout::skip::{
    GEOMETRY_BUILD, GEOMETRY_DRAW, ITEM_SIZE, LIGHTMAP_LAYER, RENDER_RECORDS, RENDER_RECORD_SIZE,
};

const MAX_LAYER_ENTRIES: u32 = 2560;
const MAX_RENDER_RECORDS: u32 = 0x352;
const RENDER_RECORD_COUNT: u32 = 0x004c_0b78;
const LIST_CAPACITY: u32 = 0;
const LIST_END: u32 = 4;
const LIST_FIRST: u32 = 0xc;
const LIST_ITEMS: u32 = 0x14;
const ENTRY_IMPLEMENTATION: u32 = 4;
const ENTRY_CLASSIFICATION: u32 = 8;
const ENTRY_PROGRAM: u32 = 0x10;
const PROGRAM_VERTEX_SHADER: u32 = 0x100;
const PROGRAM_FLAGS: u32 = 0x118;
const HANDLER_COUNTER: u32 = 0x28;
const RECORD_DRAW_FLAGS: u32 = 0x1a;
const SECTION_COUNT: u32 = MODEL_SECTIONS - 4;
const SECTION_INFLUENCES: u32 = 0x14;
const SECTION_CLASSIFICATION: u32 = 0x18;
const SECTION_GEOMETRY: u32 = 0x34;
const VERTEX_SHADER_CLASS_COUNT: u32 = 4;
const SECTION_PART_COUNT: u32 = 0;
const SECTION_PARTS: u32 = 4;
const SHADER_FLAGS: u32 = 0x16;
const NO_STRUCTURE_LIGHTMAP: u32 = 0x7ff;
const PART_SIZE: u32 = 0x48;
const PART_FLAGS: u32 = 2;
const PART_FIRST_INDEX: u32 = 6;
const PART_INDEX_COUNT: u32 = 8;
// Handler state changes: stencil marking, conditional stream ranges and
// the auxiliary render-state toggle in the geometry draw's prologue/epilogue.
const SPECIAL_RECORD_FLAGS: u32 = (1 << 6) | (1 << 9) | (1 << 11) | (1 << 14);

/// The `vrtx` classification `0x0001cb20` picks for a section of geometry
/// classification `geometry` (the section's u16 at `+0x18`) and node
/// influences `influences` (its byte `+0x14`), of a tag with `count`
/// classifications: 0 and 1 and 2 as they are, 3 (weighted nodes) the
/// influences plus 2 when that is below `count`, else 0; `None` past 3
/// (the jump table has four entries). The layer-3 draw reads `+0x14`; the
/// draw modes 4 and 5 read `+0x15` instead (`0x0001cabd`), not taken here.
pub fn classification(geometry: u16, influences: u8, count: u32) -> Option<u32> {
    Some(match geometry {
        0..=2 => u32::from(geometry),
        3 => {
            let c = u32::from(influences) + 2;
            if c < count { c } else { 0 }
        }
        _ => return None,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Part {
    pub key: ItemKey,
    pub mode: u32,
    pub program: u32,
    pub vertex_shader: u32,
    pub geometry_class: u16,
    pub node_influences: u8,
    pub triangle_list: bool,
    pub first_index: u16,
    pub index_count: u16,
}

impl Part {
    /// The exact uploaded palette the original draw would use. These rows
    /// already incorporate the selected section's remap and skinning pose.
    pub fn rows(self, mem: &dyn GuestRead) -> Option<Vec<Row>> {
        let rows = objects::record_rows(mem, self.key.record).ok()?;
        rows.iter().flatten().all(|v| v.is_finite()).then_some(rows)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    Excluded,
    Other,
    Object { handle: u32, part: Option<Part> },
}

pub struct SortedObjects {
    pub base: u32,
    pub current: usize,
    pub counter: u32,
    pub entries: Vec<Entry>,
}

fn word(mem: &dyn GuestRead, base: u32, field: u32) -> Option<u32> {
    mem.read_u32(base.checked_add(field)?)
}

fn half(mem: &dyn GuestRead, base: u32, field: u32) -> Option<u16> {
    Some((word(mem, base, field & !3)? >> (8 * (field & 3))) as u16)
}

/// At the sorted geometry handler, the caller's saved loop counter is
/// immediately before the entry buffer. Read earlier entries too so an
/// object whose first part already ran can never be taken here.
pub fn read(mem: &dyn GuestRead, list: u32, esp: u32, esi: u32) -> Option<SortedObjects> {
    if word(mem, esp, 16)? != LIGHTMAP_LAYER || word(mem, esp, 28)? != esi || list == 0 {
        return None;
    }
    let counter = esp.checked_add(HANDLER_COUNTER)?;
    let base = counter.checked_add(4)?;
    let distance = esi.checked_sub(base)?;
    let current = distance / ITEM_SIZE;
    let remaining = word(mem, counter, 0)?;
    let count = current.checked_add(remaining)?;
    if distance % ITEM_SIZE != 0 || remaining == 0 || count > MAX_LAYER_ENTRIES {
        return None;
    }
    let first = word(mem, list, LIST_FIRST)?;
    let end = word(mem, list, LIST_END)?;
    let table = word(mem, list, LIST_ITEMS)?;
    if first > end || end > word(mem, list, LIST_CAPACITY)? || table == 0 {
        return None;
    }
    // The allocator increments the counter even when its bounded record
    // array is full.
    let record_count = word(mem, RENDER_RECORD_COUNT, 0)?.min(MAX_RENDER_RECORDS);
    let mut entries = Vec::with_capacity(count as usize);
    for n in 0..count {
        let slot = base.checked_add(n * ITEM_SIZE)?;
        let program = word(mem, slot, ENTRY_PROGRAM)?;
        if word(mem, program, PROGRAM_VERTEX_SHADER)? == u32::MAX {
            entries.push(Entry::Excluded);
            continue;
        }
        let index = u32::from(half(mem, slot, 0)?);
        if !(first..end).contains(&index) {
            return None;
        }
        let item = table.checked_add(index.checked_mul(ITEM_SIZE)?)?;
        let build = word(mem, item, 0)?;
        let draw = word(mem, item, 4)?;
        if build != GEOMETRY_BUILD && draw != GEOMETRY_DRAW {
            entries.push(Entry::Other);
            continue;
        }
        // An unknown owner could be another part of a recognized object.
        // Refuse the walk before classifying any object as complete.
        let packed = word(mem, item, 0xc)?;
        let record = (packed >> 8) as u16;
        if u32::from(record) >= record_count {
            return None;
        }
        let handle = match objects::record_owner(mem, record)? {
            RecordOwner::World => {
                entries.push(Entry::Other);
                continue;
            }
            RecordOwner::Object(handle) => handle,
            RecordOwner::Unresolved => return None,
        };
        let render = RENDER_RECORDS + RENDER_RECORD_SIZE * u32::from(record);
        let part = if build == GEOMETRY_BUILD && draw == GEOMETRY_DRAW {
            read_part(mem, slot, item, render, program, packed)
        } else {
            None
        };
        entries.push(Entry::Object { handle, part });
    }
    Some(SortedObjects { base, current: current as usize, counter, entries })
}

fn read_part(
    mem: &dyn GuestRead,
    slot: u32,
    item: u32,
    record: u32,
    program: u32,
    packed: u32,
) -> Option<Part> {
    let key = objects::item_key(mem, item)?;
    let record_key = word(mem, record, 0xc)?;
    let flags = word(mem, record, 0)?;
    let mode = word(mem, record, objects::RECORD_MODEL)?;
    let tag_table = word(mem, TAG_INSTANCES, 0)?;
    let model = word(mem, tag_table, 16 * (mode & 0xffff) + 8)?;
    if u32::from(key.section) >= word(mem, model, SECTION_COUNT)? {
        return None;
    }
    let sections = word(mem, model, MODEL_SECTIONS)?;
    let section = sections.checked_add(SECTION_STRIDE * u32::from(key.section))?;
    let geometry_class = half(mem, section, SECTION_CLASSIFICATION)?;
    let node_influences = half(mem, section, SECTION_INFLUENCES)? as u8;
    let vertex_shader = word(mem, program, PROGRAM_VERTEX_SHADER)?;
    let vertex_tag = word(mem, tag_table, 16 * (vertex_shader & 0xffff) + 8)?;
    let classes = word(mem, vertex_tag, VERTEX_SHADER_CLASS_COUNT)?;
    let class = classification(geometry_class, node_influences, classes)?;
    if class >= classes || u32::from(half(mem, slot, ENTRY_CLASSIFICATION)?) != class {
        return None;
    }
    let geometry = word(mem, section, SECTION_GEOMETRY)?;
    if u32::from(key.part) >= word(mem, geometry, SECTION_PART_COUNT)? {
        return None;
    }
    let parts = word(mem, geometry, SECTION_PARTS)?;
    let shader = word(mem, tag_table, 16 * (key.shader & 0xffff) + 8)?;
    let part = parts.checked_add(PART_SIZE * u32::from(key.part))?;
    let index_count = half(mem, part, PART_INDEX_COUNT)?;
    if record_key & 0xe000_0000 != MODEL_RECORD
        || flags & SPECIAL_RECORD_FLAGS != 0
        || (((record_key >> 18) & NO_STRUCTURE_LIGHTMAP) != NO_STRUCTURE_LIGHTMAP
            && !matches!(key.implementation, 2 | 3))
        || (half(mem, record, RECORD_DRAW_FLAGS)? as u8) != u8::MAX
        || (packed >> 24) != 0
        || packed as u8 != key.part
        || half(mem, item, 0x14)? != 0
        || word(mem, tag_table, 16 * (key.shader & 0xffff))? != u32::from_be_bytes(*b"shad")
        || half(mem, shader, SHADER_FLAGS)? & 1 != 0
        || word(mem, program, PROGRAM_FLAGS)? & (1 << 20) != 0
        || usize::from(half(mem, slot, ENTRY_IMPLEMENTATION)? as u8) != key.implementation
        || index_count == 0
    {
        return None;
    }
    Some(Part {
        key,
        mode,
        program,
        vertex_shader,
        geometry_class,
        node_influences,
        triangle_list: half(mem, part, PART_FLAGS)? & 8 != 0,
        first_index: half(mem, part, PART_FIRST_INDEX)?,
        index_count,
    })
}

/// A balanced span of sorted dispatches. Every object touched by the
/// span has all its active layer-3 entries inside it. Keep the smallest
/// such spans so an unsupported object leaves unrelated spans available.
#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    pub slots: Range<usize>,
    pub objects: usize,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Decisions {
    pub runs: Vec<Run>,
    pub unsupported: usize,
    pub interleaved: usize,
}

pub fn decide(entries: &[Entry], current: usize) -> Decisions {
    #[derive(Clone, Copy)]
    struct Span {
        first: usize,
        last: usize,
        supported: bool,
    }
    let mut spans = HashMap::<u32, Span>::new();
    for (slot, entry) in entries.iter().enumerate() {
        if let Entry::Object { handle, part } = entry {
            let span = spans.entry(*handle).or_insert(Span { first: slot, last: slot, supported: true });
            span.last = slot;
            span.supported &= part.is_some();
        }
    }
    let mut result = Decisions::default();
    let mut start = 0;
    while start < entries.len() {
        if !matches!(entries[start], Entry::Object { .. }) {
            start += 1;
            continue;
        }
        let mut end = start;
        let mut slot = start;
        let mut unsupported = 0;
        let mut object_count = 0;
        let mut other = false;
        while slot <= end {
            match entries[slot] {
                Entry::Object { handle, .. } => {
                    let span = spans[&handle];
                    end = end.max(span.last);
                    if span.first == slot {
                        object_count += 1;
                        unsupported += usize::from(!span.supported);
                    }
                }
                Entry::Excluded => {}
                Entry::Other => other = true,
            }
            slot += 1;
        }
        if unsupported == 0 && !other && start >= current {
            result.runs.push(Run { slots: start..slot, objects: object_count });
        } else {
            result.unsupported += unsupported;
            result.interleaved += object_count - unsupported;
        }
        start = slot;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;
    use crate::layout::object_nodes::RECORD_MATRIX;
    use crate::testing::{lighting, object, record};

    const SHADER: u32 = 0xe500_0012;
    const VERTEX_SHADER: u32 = 0xe100_003e;

    fn part() -> Part {
        Part {
            key: ItemKey { record: 1, shader: SHADER, section: 9, part: 2, implementation: 3 },
            mode: 0xe100_0001,
            program: 0x4000,
            vertex_shader: VERTEX_SHADER,
            geometry_class: 3,
            node_influences: 2,
            triangle_list: true,
            first_index: 60,
            index_count: 18,
        }
    }

    fn supported(handle: u32) -> Entry {
        Entry::Object { handle, part: Some(part()) }
    }

    #[test]
    fn interleaved_object_parts_are_taken_together_in_sorted_order() {
        let entries = [
            Entry::Other, supported(8), supported(9), Entry::Excluded,
            supported(8), supported(9), supported(10), Entry::Other,
        ];
        let decisions = decide(&entries, 1);
        assert_eq!(decisions.runs, [Run { slots: 1..6, objects: 2 }, Run { slots: 6..7, objects: 1 }]);
        assert_eq!(decisions.unsupported + decisions.interleaved, 0);
        let decisions = decide(&entries, 2);
        assert_eq!(decisions.runs, [Run { slots: 6..7, objects: 1 }]);
        assert_eq!(decisions.interleaved, 2, "a part of the first span already ran");
    }

    #[test]
    fn one_unsupported_part_keeps_its_whole_object_on_the_guest() {
        let entries = [
            supported(7), Entry::Object { handle: 8, part: None },
            supported(8), supported(9), Entry::Other, supported(9), supported(10),
        ];
        let decisions = decide(&entries, 0);
        assert_eq!(decisions.runs, [Run { slots: 0..1, objects: 1 }, Run { slots: 6..7, objects: 1 }]);
        assert_eq!(decisions.unsupported, 1);
        assert_eq!(decisions.interleaved, 1);
    }

    /// A section from a lower LOD, using a nonzero part and implementation
    /// and a render-time matrix distinct from the body's raw pose.
    fn sorted_part() -> (Mem, u32) {
        let (esp, list, items, program) = (0x1000, 0x2000, 0x3000, 0x4000);
        let (table, model, sections, geometry, parts, shader, matrix) =
            (0x5000, 0x6000, 0x7000, 0x8000, 0x9000, 0xa000, 0xb000);
        let expected = part();
        let slot = esp + HANDLER_COUNTER + 4;
        let mut mem = Mem::default();
        let owner = object(&mut mem, 1, 0xe000_0100, 3);
        record(&mut mem, 1, Some(lighting(8)));
        let r = RENDER_RECORDS + RENDER_RECORD_SIZE;
        for (at, value) in [
            (esp + 16, LIGHTMAP_LAYER),
            (esp + 28, slot),
            (esp + HANDLER_COUNTER, 1),
            (list + LIST_CAPACITY, 1),
            (list + LIST_END, 1),
            (list + LIST_FIRST, 0),
            (list + LIST_ITEMS, items),
            (slot, 0),
            (slot + ENTRY_PROGRAM, program),
            (slot + ENTRY_IMPLEMENTATION, 3),
            (slot + ENTRY_CLASSIFICATION, 4),
            (program + PROGRAM_VERTEX_SHADER, VERTEX_SHADER),
            (program + PROGRAM_FLAGS, 0),
            (items, GEOMETRY_BUILD),
            (items + 4, GEOMETRY_DRAW),
            (items + 8, SHADER),
            (items + 0xc, 1 << 8 | 2),
            (items + 0x14, 0),
            (RENDER_RECORD_COUNT, 2),
            (r, 3 << 29),
            (r + 0xc, MODEL_RECORD | 9 << 9 | 2),
            (r + 0x18, 0xff << 16),
            (r + RECORD_MATRIX, matrix),
            (TAG_INSTANCES, table),
            (table + 16 * (expected.mode & 0xffff) + 8, model),
            (table + 16 * (SHADER & 0xffff), u32::from_be_bytes(*b"shad")),
            (table + 16 * (SHADER & 0xffff) + 8, shader),
            (table + 16 * (VERTEX_SHADER & 0xffff) + 8, 0xc000),
            (0xc000 + VERTEX_SHADER_CLASS_COUNT, 5),
            (shader + 0x14, 0),
            (model + SECTION_COUNT, 10),
            (model + MODEL_SECTIONS, sections),
            (sections + 9 * SECTION_STRIDE + SECTION_GEOMETRY, geometry),
            (sections + 9 * SECTION_STRIDE + SECTION_CLASSIFICATION, 3),
            (sections + 9 * SECTION_STRIDE + SECTION_INFLUENCES, 2),
            (geometry + SECTION_PART_COUNT, 3),
            (geometry + SECTION_PARTS, parts),
            (parts + 2 * PART_SIZE, 8 << 16),
            (parts + 2 * PART_SIZE + 4, 60 << 16),
            (parts + 2 * PART_SIZE + 8, 18),
        ] {
            mem.w(at, value);
        }
        for n in 1..=12 {
            mem.f(matrix + 4 * n, n as f32);
        }
        (mem, owner)
    }

    #[test]
    fn sorted_walk_uses_the_live_lod_and_node_rows() {
        let (mut mem, handle) = sorted_part();
        let at = 0x1000 + HANDLER_COUNTER + 4;
        let sorted = read(&mem, 0x2000, 0x1000, at).unwrap();
        assert_eq!(sorted.entries, [Entry::Object { handle, part: Some(part()) }]);
        assert_eq!(part().rows(&mem).unwrap(), [
            [1.0, 4.0, 7.0, 10.0],
            [2.0, 5.0, 8.0, 11.0],
            [3.0, 6.0, 9.0, 12.0],
        ]);
        mem.f(0xb000 + 4, f32::NAN);
        assert!(part().rows(&mem).is_none());

        mem.w(RENDER_RECORD_COUNT, MAX_RENDER_RECORDS + 1);
        assert!(read(&mem, 0x2000, 0x1000, at).is_some());
        mem.w(at + ENTRY_CLASSIFICATION, 0);
        assert_eq!(
            read(&mem, 0x2000, 0x1000, at).unwrap().entries,
            [Entry::Object { handle, part: None }]
        );
        mem.w(at + ENTRY_CLASSIFICATION, 4);
        mem.w(at + ENTRY_IMPLEMENTATION, 2);
        assert_eq!(read(&mem, 0x2000, 0x1000, at).unwrap().entries, [
            Entry::Object { handle, part: None }
        ]);
        mem.w(RENDER_RECORD_COUNT, 1);
        assert!(read(&mem, 0x2000, 0x1000, at).is_none());
    }
}
