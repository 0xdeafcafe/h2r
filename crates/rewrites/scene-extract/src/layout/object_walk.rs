//! Layer-3 object dispatches after the guest has selected render records
//! and sorted the layer. See docs/graphics/native-renderer.md.
//!
//! ## What a render record's flags make the geometry draw do
//!
//! The geometry draw `0x0004d0b0` (an item's `+4`) reads four bits of the
//! render record's `+0` in layer 3 (**Code**, docs/graphics/framescene.md,
//! slice 3). A part is in the scene with the bits the native pass draws
//! as [`PartFlags`]; the others refuse it, each by its own cause
//! ([`Refusal`]).
//!
//! - **Bit 14, stencil marking** ([`PartFlags::STENCIL_MARK`]). The
//!   prologue `0x0004d640`, in layers 1 and 3, sets stencil mode 2 through
//!   `0x00015370` (`ALWAYS`, reference `0xff`, masks `0xff`, `REPLACE` on
//!   pass) and the epilogue `0x0004d720` sets mode 1 (the same with
//!   reference 0). With either flag byte of `skip::STENCIL_OFF_FLAGS` set
//!   the prologue sets mode 0 for every item instead, and the bit changes
//!   nothing. Layer 16 skips the item's draw entirely (`0x0004d13e`).
//! - **Bit 9, strip topology** ([`PartFlags::STRIP`]). The whole-part
//!   emitter `0x0004daa0` draws `5 + strip` (list or strip), `strip` being
//!   bit 9 and the part's flag `8` (triangle list) clear (`0x0004d5e2`).
//!   Without bit 9 it draws a list whatever the part's flag says; the
//!   native models decode a part without the flag as a strip, so that case
//!   refuses ([`Refusal::Topology`]).
//! - **Bit 6, conditional stream ranges** ([`PartFlags::SUBPARTS`]). With
//!   the record's `+0x14` (a pointer to a bitmask of visible subparts,
//!   bit `s` in word `s >> 5`) and the section geometry's `+8` (its
//!   subpart count) both nonzero, the emitter draws only the visible
//!   subparts of the part (part `+0xa` first subpart, `+0xc` count, both
//!   signed; geometry `+0xc` the subparts, 8 bytes each: first index,
//!   index count): each as its own strip with strip topology
//!   (`0x0004daec`), runs of index-contiguous ones as one list otherwise
//!   (`0x0004db68`). Otherwise it draws the whole part, as without the
//!   bit. The part is in the scene with the mask read over its own
//!   subparts ([`Subparts`]); a part with more than [`Subparts::MAX`]
//!   subparts, a negative first subpart or a mask that does not read
//!   refuses ([`Refusal::Subparts`]).
//! - **Bit 11, the auxiliary state toggle.** The prologue calls
//!   `0x00012d50` with BL 1 and the epilogue with BL 0: it rewrites the
//!   device's depth scale (device `+0x1c10`, from the scale at
//!   `0x00485adc` times one of two constants, flagged dirty in
//!   `+0x1c18`) and, when the cached byte `0x0055e6bc` differs, the
//!   transform it builds from the view at `0x0048568c` with its first rows
//!   scaled (`0x003f7280`). The native object programs do not take
//!   either, so it refuses ([`Refusal::AuxState`]).

use std::collections::HashMap;
use std::ops::Range;

use frame_scene::{PartFlags, Subparts};

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
const PART_FIRST_SUBPART: u32 = 0xa;
const PART_SUBPART_COUNT: u32 = 0xc;
const PART_INDEX_COUNT: u32 = 8;
/// Record `+0` bits the geometry draw reads in layer 3 (the module notes).
pub const RECORD_SUBPARTS: u32 = 1 << 6;
pub const RECORD_STRIP: u32 = 1 << 9;
pub const RECORD_AUX_STATE: u32 = 1 << 11;
pub const RECORD_STENCIL_MARK: u32 = 1 << 14;
/// The record's visible-subpart mask pointer.
const RECORD_SUBPART_MASK: u32 = 0x14;
/// The section geometry's subpart count.
const GEOMETRY_SUBPART_COUNT: u32 = 8;
/// The part's flag that its indices are a triangle list.
const PART_TRIANGLE_LIST: u16 = 8;

/// Why a part an object item names is not in the scene, when its record
/// asks for something the native pass does not draw (the module notes).
/// Any other refusal is [`Refusal::Other`]: the item's layout, a check
/// against the guest's choice, or memory that does not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Bit 11: the depth-scale toggle around the draw.
    AuxState,
    /// Bit 6 with a subpart mask and subparts, whose visible subparts
    /// cannot be read: more than [`Subparts::MAX`], a negative first one,
    /// or a mask that does not read.
    Subparts,
    /// No bit 9 on a part that is not a triangle list.
    Topology,
    Other,
}

/// Parts refused by each named cause in a walk (items, not objects).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Causes {
    pub aux_state: usize,
    pub subparts: usize,
    pub topology: usize,
}

impl Causes {
    fn add(&mut self, why: Refusal) {
        match why {
            Refusal::AuxState => self.aux_state += 1,
            Refusal::Subparts => self.subparts += 1,
            Refusal::Topology => self.topology += 1,
            Refusal::Other => {}
        }
    }
}

/// The [`PartFlags`] of a part whose record has `flags` at `+0`, or the
/// cause that refuses it: `triangle_list` the part's flag, `subparts`
/// whether the record's subpart mask and the section's subpart count are
/// both nonzero. Bit 11 refuses first, then the topology.
pub fn record_flags(flags: u32, triangle_list: bool, subparts: bool) -> Result<PartFlags, Refusal> {
    if flags & RECORD_AUX_STATE != 0 {
        return Err(Refusal::AuxState);
    }
    let mut out = PartFlags::NONE;
    if flags & RECORD_SUBPARTS != 0 && subparts {
        out |= PartFlags::SUBPARTS;
    }
    if !triangle_list {
        if flags & RECORD_STRIP == 0 {
            return Err(Refusal::Topology);
        }
        out |= PartFlags::STRIP;
    }
    if flags & RECORD_STENCIL_MARK != 0 {
        out |= PartFlags::STENCIL_MARK;
    }
    Ok(out)
}

/// The subparts of a part (its first subpart `first` and count `count`,
/// the part's signed `+0xa` and `+0xc`) that a visible-subpart mask at
/// `mask` lets the emitter draw: bit `s & 31` of word `s >> 5` for the
/// section's subpart `s` (`0x0004db08`..`0x0004db17`). A count of zero or
/// less draws none.
pub fn visible_subparts(mem: &dyn GuestRead, mask: u32, first: i16, count: i16) -> Result<Subparts, Refusal> {
    if first < 0 || count > i16::from(Subparts::MAX) {
        return Err(Refusal::Subparts);
    }
    let count = count.max(0) as u8;
    let mut visible = 0u64;
    for n in 0..count {
        let s = first as u32 + u32::from(n);
        let w = word(mem, mask, 4 * (s >> 5)).ok_or(Refusal::Subparts)?;
        visible |= u64::from(w >> (s & 31) & 1) << n;
    }
    Ok(Subparts { first: first as u16, count, visible })
}

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
    /// What its record makes the draw do beyond its material.
    pub flags: PartFlags,
    /// With [`PartFlags::SUBPARTS`], the subparts the emitter draws.
    pub subparts: Option<Subparts>,
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
    /// The parts refused by a named cause.
    pub causes: Causes,
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
    let mut causes = Causes::default();
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
            read_part(mem, slot, item, render, program, packed).inspect_err(|&why| causes.add(why)).ok()
        } else {
            None
        };
        entries.push(Entry::Object { handle, part });
    }
    Some(SortedObjects { base, current: current as usize, counter, entries, causes })
}

fn read_part(
    mem: &dyn GuestRead,
    slot: u32,
    item: u32,
    record: u32,
    program: u32,
    packed: u32,
) -> Result<Part, Refusal> {
    let (part, flags, conditional) = read_part_layout(mem, slot, item, record, program, packed).ok_or(Refusal::Other)?;
    let flags = record_flags(flags, part.triangle_list, conditional.is_some())?;
    let subparts = match conditional {
        Some(c) if flags.contains(PartFlags::SUBPARTS) => Some(visible_subparts(mem, c.mask, c.first, c.count)?),
        _ => None,
    };
    Ok(Part { flags, subparts, ..part })
}

/// A part whose record draws only its visible subparts: the mask's
/// address and the part's first subpart and count.
#[derive(Clone, Copy)]
struct Conditional {
    mask: u32,
    first: i16,
    count: i16,
}

/// The part an item draws (without its flags), its record's `+0` flags
/// and, when the record's bit 6, subpart mask and its section's subpart
/// count are all set, where its visible subparts are read from; `None`
/// when the item's layout or a check against the guest's choice fails.
fn read_part_layout(
    mem: &dyn GuestRead,
    slot: u32,
    item: u32,
    record: u32,
    program: u32,
    packed: u32,
) -> Option<(Part, u32, Option<Conditional>)> {
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
    // Read only under bit 6, as the guest does (`0x0004d5ff`).
    let mut conditional = None;
    if flags & RECORD_SUBPARTS != 0 {
        let mask = word(mem, record, RECORD_SUBPART_MASK)?;
        if mask != 0 && word(mem, geometry, GEOMETRY_SUBPART_COUNT)? != 0 {
            conditional = Some(Conditional {
                mask,
                first: half(mem, part, PART_FIRST_SUBPART)? as i16,
                count: half(mem, part, PART_SUBPART_COUNT)? as i16,
            });
        }
    }
    let read = Part {
        key,
        mode,
        program,
        vertex_shader,
        geometry_class,
        node_influences,
        triangle_list: half(mem, part, PART_FLAGS)? & PART_TRIANGLE_LIST != 0,
        first_index: half(mem, part, PART_FIRST_INDEX)?,
        index_count,
        flags: PartFlags::NONE,
        subparts: None,
    };
    Some((read, flags, conditional))
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
            flags: PartFlags::NONE,
            subparts: None,
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
        assert_eq!(sorted.causes, Causes::default());
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

    #[test]
    fn a_records_flags_become_part_flags_or_name_their_refusal() {
        // Bit 14 marks; bit 9 makes a non-list part a strip.
        assert_eq!(record_flags(0, true, false), Ok(PartFlags::NONE));
        assert_eq!(record_flags(RECORD_STENCIL_MARK, true, false), Ok(PartFlags::STENCIL_MARK));
        assert_eq!(record_flags(RECORD_STRIP, false, false), Ok(PartFlags::STRIP));
        assert_eq!(record_flags(RECORD_STRIP, true, false), Ok(PartFlags::NONE), "a list part ignores bit 9");
        assert_eq!(
            record_flags(RECORD_STRIP | RECORD_STENCIL_MARK, false, false),
            Ok(PartFlags::STRIP | PartFlags::STENCIL_MARK)
        );
        // Without bit 9 the guest draws a strip part's indices as a list.
        assert_eq!(record_flags(0, false, false), Err(Refusal::Topology));
        // Bit 6 chooses subparts only when there is a mask and subparts to
        // choose.
        assert_eq!(record_flags(RECORD_SUBPARTS, true, false), Ok(PartFlags::NONE));
        assert_eq!(record_flags(RECORD_SUBPARTS, true, true), Ok(PartFlags::SUBPARTS));
        assert_eq!(record_flags(RECORD_SUBPARTS | RECORD_STRIP, false, true), Ok(PartFlags::SUBPARTS | PartFlags::STRIP));
        assert_eq!(record_flags(RECORD_SUBPARTS, false, true), Err(Refusal::Topology));
        // Bit 11 first.
        assert_eq!(record_flags(RECORD_AUX_STATE | RECORD_SUBPARTS, false, true), Err(Refusal::AuxState));
    }

    #[test]
    fn a_mask_is_read_over_the_parts_own_subparts() {
        let mut mem = Mem::default();
        mem.w(0x100, 0b1010_0000);
        mem.w(0x104, 1);
        let got = visible_subparts(&mem, 0x100, 5, 4).unwrap();
        assert_eq!(got, Subparts { first: 5, count: 4, visible: 0b0101 });
        // Subpart 32 is bit 0 of the second word.
        assert_eq!(visible_subparts(&mem, 0x100, 31, 2).unwrap().visible, 0b10);
        assert_eq!(visible_subparts(&mem, 0x100, 0, -1).unwrap().count, 0, "a count below one draws none");
        assert_eq!(visible_subparts(&mem, 0x100, -1, 2), Err(Refusal::Subparts));
        assert_eq!(visible_subparts(&mem, 0x100, 0, 65), Err(Refusal::Subparts));
        assert_eq!(visible_subparts(&mem, 0x100, 60, 8), Err(Refusal::Subparts), "the third word does not read");
    }

    /// The record's `+0` in [`sorted_part`]'s memory, with `bits` added.
    fn with_record_bits(mem: &mut Mem, bits: u32) {
        let r = RENDER_RECORDS + RENDER_RECORD_SIZE;
        mem.w(r, 3 << 29 | bits);
    }

    #[test]
    fn each_former_special_record_flag_is_a_part_or_a_named_cause() {
        let at = 0x1000 + HANDLER_COUNTER + 4;
        let walk = |mem: &Mem| read(mem, 0x2000, 0x1000, at).unwrap();
        let (mut mem, handle) = sorted_part();

        // Stencil marking: in the scene, flagged.
        with_record_bits(&mut mem, RECORD_STENCIL_MARK);
        let sorted = walk(&mem);
        let marked = Part { flags: PartFlags::STENCIL_MARK, ..part() };
        assert_eq!(sorted.entries, [Entry::Object { handle, part: Some(marked) }]);
        assert_eq!(sorted.causes, Causes::default());

        // Strip topology on a part that is not a list: in the scene, flagged.
        with_record_bits(&mut mem, RECORD_STRIP);
        let list = 0x9000 + 2 * PART_SIZE;
        mem.w(list, 0);
        let strip = Part { flags: PartFlags::STRIP, triangle_list: false, ..part() };
        assert_eq!(walk(&mem).entries, [Entry::Object { handle, part: Some(strip) }]);
        // The same part without bit 9: the guest draws it as a list.
        with_record_bits(&mut mem, 0);
        let sorted = walk(&mem);
        assert_eq!(sorted.entries, [Entry::Object { handle, part: None }]);
        assert_eq!(sorted.causes, Causes { topology: 1, ..Causes::default() });
        mem.w(list, 8 << 16);

        // Conditional ranges: the whole part without a mask, the visible
        // subparts with one.
        with_record_bits(&mut mem, RECORD_SUBPARTS);
        mem.w(RENDER_RECORDS + RENDER_RECORD_SIZE + RECORD_SUBPART_MASK, 0);
        mem.w(0x8000 + GEOMETRY_SUBPART_COUNT, 40);
        assert_eq!(walk(&mem).entries, [Entry::Object { handle, part: Some(part()) }]);
        let at_part = 0x9000 + 2 * PART_SIZE;
        // Subparts 30..35 of the section (first 30, count 5), across the
        // mask's first two words: 30 and 33 visible.
        mem.w(at_part + 8, 30 << 16 | 18);
        mem.w(at_part + PART_SUBPART_COUNT, 5);
        mem.w(RENDER_RECORDS + RENDER_RECORD_SIZE + RECORD_SUBPART_MASK, 0xd000);
        mem.w(0xd000, 1 << 30 | 1 << 29 | 1 << 2);
        mem.w(0xd004, 1 << 1 | 1 << 5);
        let sorted = walk(&mem);
        let chosen = Part {
            flags: PartFlags::SUBPARTS,
            subparts: Some(Subparts { first: 30, count: 5, visible: 0b0_1001 }),
            ..part()
        };
        assert_eq!(sorted.entries, [Entry::Object { handle, part: Some(chosen) }]);
        assert_eq!(sorted.causes, Causes::default());
        assert_eq!(chosen.subparts.unwrap().drawn().collect::<Vec<_>>(), [0, 3]);
        // A mask word that does not read, or more subparts than a mask
        // holds: refused by name.
        mem.0.remove(&0xd004);
        assert_eq!(walk(&mem).causes, Causes { subparts: 1, ..Causes::default() });
        mem.w(0xd004, 0);
        mem.w(at_part + PART_SUBPART_COUNT, 65);
        assert_eq!(walk(&mem).causes, Causes { subparts: 1, ..Causes::default() });
        // No subparts: drawn, as the guest draws it, with none.
        mem.w(at_part + PART_SUBPART_COUNT, 0);
        let none = Part { subparts: Some(Subparts { first: 30, count: 0, visible: 0 }), ..chosen };
        assert_eq!(walk(&mem).entries, [Entry::Object { handle, part: Some(none) }]);
        mem.w(0x8000 + GEOMETRY_SUBPART_COUNT, 0);
        assert_eq!(walk(&mem).entries, [Entry::Object { handle, part: Some(part()) }], "no subparts to choose");
        mem.w(at_part + 8, 18);

        // The auxiliary state toggle: refused by name.
        with_record_bits(&mut mem, RECORD_AUX_STATE);
        let sorted = walk(&mem);
        assert_eq!(sorted.entries, [Entry::Object { handle, part: None }]);
        assert_eq!(sorted.causes, Causes { aux_state: 1, ..Causes::default() });
    }
}
