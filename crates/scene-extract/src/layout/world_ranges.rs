//! Sorted BSP geometry arguments and the index slices submitted by the
//! ordinary geometry handler (0x4d0b0) through its emitter (0x4daa0).
//! Offsets in `indices` are in the section's original u16 index stream.

use std::ops::Range;

use crate::GuestRead;
use crate::layout::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, GEOMETRY_DRAW, ITEM_SIZE,
    RENDER_RECORDS, RENDER_RECORD_SIZE};
use frame_scene::Owner;

const RECORD_COUNT: u32 = 0x004c_0b78;
const RECORD_CAPACITY: u32 = 0x352;
const INSTANCE_RECORD: u32 = 1 << 12;
const USE_SUBPARTS: u32 = 1 << 6;
const CAN_DRAW_STRIP: u32 = 1 << 9;
const LIST_PART: u16 = 1 << 3;
const PART_SIZE: u32 = 0x48;
const SUBPART_SIZE: u32 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    OtherHandler,
    OtherLayer,
    Object,
    Memory,
    Record,
    Section,
    Part,
    Indices,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SortedPart {
    pub sorted_slot: u32,
    pub owner: Owner,
    pub part: u8,
    pub record: u16,
    pub record_addr: u32,
    pub section: u32,
    pub geometry: u32,
    pub part_addr: u32,
    pub shader: u32,
    pub layer_argument: u32,
    pub lod: u8,
    /// Same handler argument named `variant` in world_layers::Geometry.
    pub variant: u8,
    pub projected_size: u8,
    pub layer: u32,
    pub pass: u8,
    pub implementation: u8,
    pub program: u32,
    pub classification: u16,
    pub strip: bool,
    pub index_buffer: u32,
    pub indices: Vec<Range<u32>>,
}

fn add(base: u32, offset: u32) -> Result<u32, Refusal> {
    base.checked_add(offset).ok_or(Refusal::Memory)
}

fn word(m: &dyn GuestRead, base: u32, offset: u32) -> Result<u32, Refusal> {
    m.read_u32(add(base, offset)?).ok_or(Refusal::Memory)
}

fn half(m: &dyn GuestRead, base: u32, offset: u32) -> Result<u16, Refusal> {
    let value = word(m, base, offset & !3)?;
    Ok((value >> (8 * (offset & 3))) as u16)
}

fn element(m: &dyn GuestRead, base: u32, count_offset: u32, i: u32, stride: u32,
    reason: Refusal) -> Result<u32, Refusal> {
    if i >= word(m, base, count_offset)? {
        return Err(reason);
    }
    let array = word(m, base, count_offset + 4)?;
    if array == 0 {
        return Err(reason);
    }
    add(array, i.checked_mul(stride).ok_or(reason)?)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Arguments {
    shader: u32,
    layer_argument: u32,
    lod: u8,
    layer: u32,
    pass: u8,
    argument: u32,
}

impl Arguments {
    fn stack(m: &dyn GuestRead, esp: u32) -> Result<Self, Refusal> {
        Ok(Self {
            shader: word(m, esp, 4)?,
            layer_argument: word(m, esp, 8)?,
            lod: word(m, esp, 12)? as u8,
            layer: word(m, esp, 16)?,
            pass: word(m, esp, 20)? as u8,
            argument: word(m, esp, 24)?,
        })
    }
}

/// View into the caller's private sorted array at the standard handler.
/// Only the entry loop may advance this cursor. When replacing N entries,
/// leave it pointing to the last one so the loop increments it once.
#[derive(Debug)]
pub struct SortedRun {
    pub counter: u32,
    pub remaining: u32,
    pub slot: u32,
    list: u32,
    layer: u32,
    layer_argument: u32,
    first_item: u32,
    end_item: u32,
    items: u32,
}

impl SortedRun {
    pub fn read(m: &dyn GuestRead, list: u32, esp: u32, esi: u32) -> Result<Self, Refusal> {
        const COUNTER_OFFSET: u32 = 0x28;
        const SLOT_CAPACITY: u32 = 2560;
        let args = Arguments::stack(m, esp)?;
        if list == 0 || word(m, CURRENT_LIST, 0)? != list || args.layer != word(m, CURRENT_LAYER, 0)?
            || esi != word(m, esp, 28)? {
            return Err(Refusal::OtherHandler);
        }
        let counter = add(esp, COUNTER_OFFSET)?;
        let first_slot = add(counter, 4)?;
        let distance = esi.checked_sub(first_slot).ok_or(Refusal::OtherHandler)?;
        let current = distance / ITEM_SIZE;
        let remaining = word(m, counter, 0)?;
        if distance % ITEM_SIZE != 0 || current >= SLOT_CAPACITY || remaining == 0
            || remaining > SLOT_CAPACITY - current {
            return Err(Refusal::OtherHandler);
        }
        add(esi, remaining * ITEM_SIZE)?;
        let first_item = word(m, list, 0xc)?;
        let end_item = word(m, list, 4)?;
        let items = word(m, list, 0x14)?;
        if items == 0 || first_item >= end_item || end_item > word(m, list, 0)? {
            return Err(Refusal::OtherHandler);
        }
        let run = Self { counter, remaining, slot: esi, list, layer: args.layer,
            layer_argument: args.layer_argument, first_item, end_item, items };
        if run.arguments(m, esi)? != args {
            return Err(Refusal::OtherHandler);
        }
        Ok(run)
    }

    fn arguments(&self, m: &dyn GuestRead, slot: u32) -> Result<Arguments, Refusal> {
        let i = u32::from(half(m, slot, 0)?);
        if !(self.first_item..self.end_item).contains(&i) {
            return Err(Refusal::OtherHandler);
        }
        let item = add(self.items, i * ITEM_SIZE)?;
        if word(m, item, 0)? != GEOMETRY_BUILD || word(m, item, 4)? != GEOMETRY_DRAW {
            return Err(Refusal::OtherHandler);
        }
        let program = word(m, slot, 0x10)?;
        if program == 0 || word(m, program, 0x100)? == u32::MAX {
            return Err(Refusal::OtherHandler);
        }
        let selectors = half(m, item, 0x14)?;
        Ok(Arguments {
            shader: word(m, item, 8)?,
            layer_argument: self.layer_argument,
            lod: (selectors >> 8) as u8,
            layer: self.layer,
            pass: selectors as u8,
            argument: word(m, item, 12)?,
        })
    }

    /// Iterate by increasing offsets from the handler just entered. The
    /// first refusal ends this run, including slots the caller does not call.
    pub fn next(&self, m: &dyn GuestRead, bsp: u32, offset: u32) -> Result<SortedPart, Refusal> {
        if offset >= self.remaining || word(m, CURRENT_LIST, 0)? != self.list
            || word(m, CURRENT_LAYER, 0)? != self.layer
            || word(m, self.list, 0x14)? != self.items
            || word(m, self.list, 4)? != self.end_item
            || word(m, self.list, 0xc)? != self.first_item {
            return Err(Refusal::OtherHandler);
        }
        let slot = add(self.slot, offset * ITEM_SIZE)?;
        SortedPart::decode(m, bsp, slot, self.arguments(m, slot)?)
    }
}

/// The cluster or placement can remain identifiable when its definition,
/// geometry, part or index range prevents native emission.
struct SectionRecord {
    record: u16,
    record_addr: u32,
    flags: u32,
    owner: Owner,
    entry: u32,
}

impl SectionRecord {
    fn read(m: &dyn GuestRead, bsp: u32, argument: u32) -> Result<Self, Refusal> {
        let record = (argument >> 8) as u16;
        if u32::from(record) >= word(m, RECORD_COUNT, 0)?.min(RECORD_CAPACITY) {
            return Err(Refusal::Record);
        }
        let record_addr = RENDER_RECORDS + u32::from(record) * RENDER_RECORD_SIZE;
        if word(m, record_addr, 4)? != u32::MAX {
            return Err(Refusal::Object);
        }
        if bsp == 0 {
            return Err(Refusal::Section);
        }
        let flags = word(m, record_addr, 0)?;
        let packed = word(m, record_addr, 12)?;
        let (owner, entry) = if flags & INSTANCE_RECORD != 0 {
            let id = (packed >> 18) & 0x7ff;
            (Owner::Instance(id), element(m, bsp, 0x140, id, 0x58, Refusal::Section)?)
        } else {
            let id = (packed >> 9) & 511;
            (Owner::Cluster(id), element(m, bsp, 0x9c, id, 0xb0, Refusal::Section)?)
        };
        Ok(Self { record, record_addr, flags, owner, entry })
    }
}

impl SortedPart {
    /// Called at a validated sorted handler entry to attribute a refused
    /// section even when the rest of the part does not decode.
    pub fn owner_at(m: &dyn GuestRead, bsp: u32, esp: u32) -> Option<Owner> {
        Some(SectionRecord::read(m, bsp, word(m, esp, 24).ok()?).ok()?.owner)
    }

    /// Read at the standard handler's sorted call site, before any setup
    /// has modified the cache, stream descriptors or current render state.
    pub fn read(m: &dyn GuestRead, bsp: u32, esp: u32, slot: u32) -> Result<Self, Refusal> {
        if slot == 0 || word(m, esp, 28)? != slot {
            return Err(Refusal::OtherHandler);
        }
        Self::decode(m, bsp, slot, Arguments::stack(m, esp)?)
    }

    fn decode(m: &dyn GuestRead, bsp: u32, slot: u32, args: Arguments) -> Result<Self, Refusal> {
        let Arguments { shader, layer_argument, layer, lod, pass, argument } = args;
        if !matches!(layer, 1 | 3 | 5 | 7 | 8) {
            return Err(Refusal::OtherLayer);
        }
        let SectionRecord { record, record_addr, flags, owner, entry } = SectionRecord::read(m, bsp, argument)?;
        let section = match owner {
            Owner::Cluster(_) => entry,
            Owner::Instance(_) => {
                let definition = half(m, entry, 0x34)? as i16;
                let definition = u32::try_from(definition).map_err(|_| Refusal::Section)?;
                element(m, bsp, 0x138, definition, 0xc8, Refusal::Section)?
            }
        };
        let geometry = word(m, section, 0x50)?;
        if geometry == 0 {
            return Err(Refusal::Section);
        }
        let part = argument as u8;
        let part_addr = element(m, geometry, 0, u32::from(part), PART_SIZE, Refusal::Part)?;
        let strip = flags & CAN_DRAW_STRIP != 0 && half(m, part_addr, 2)? & LIST_PART == 0;
        let bitmap = word(m, record_addr, 0x14)?;
        let spans = emissions(m, geometry, part_addr, flags, bitmap, strip)?;
        let index_buffer = word(m, geometry, 0x24)?;
        let index_count = word(m, geometry, 0x20)?;
        if spans.iter().any(|r| r.end > index_count
            || (index_buffer == 0 && !r.is_empty())
            || index_buffer.checked_add(r.end * 2).is_none()) {
            return Err(Refusal::Indices);
        }
        let program = word(m, slot, 0x10)?;
        if program == 0 {
            return Err(Refusal::OtherHandler);
        }
        Ok(Self {
            sorted_slot: slot, owner, part, record, record_addr, section, geometry, part_addr,
            shader, layer_argument,
            lod, variant: lod, projected_size: (argument >> 24) as u8, layer,
            pass, implementation: word(m, slot, 4)? as u8,
            program, classification: half(m, slot, 8)?, strip, index_buffer, indices: spans,
        })
    }
}

/// Arguments for exactly one DrawIndexedVertices. Its index address and
/// count have already been evaluated when the emitter calls SetTexture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndexedRange {
    pub address: u32,
    pub count: u32,
}

impl IndexedRange {
    fn matches(self, base: u32, prepared: &Range<u32>) -> bool {
        base.checked_add(prepared.start * 2) == Some(self.address)
            && self.count == prepared.end - prepared.start
    }
}

#[derive(Clone, Copy)]
enum Position {
    Whole,
    First,
    Next(i32),
    // A non-contiguous list subpart caused a flush. Its address is saved
    // before SetTexture/DrawIndexedVertices; its fields are read afterward.
    Reseed { index: i32, descriptor: u32 },
    Done,
}

/// The ordinary emitter can enter guest callbacks between ranges. This
/// cursor retains only what was already selected at the last such call.
#[derive(Clone, Copy)]
pub struct Emitter {
    geometry: u32,
    part: u32,
    bitmap: u32,
    pub strip: bool,
    position: Position,
    pending: Option<(u32, u32)>,
}

impl Emitter {
    /// At the post-stencil boundary (0x4d5e2), sample the two topology
    /// flags, visibility mode and the bitmap pointer into the emitter args.
    pub fn start(m: &dyn GuestRead, part: &SortedPart) -> Result<Self, Refusal> {
        let flags = word(m, part.record_addr, 0)?;
        let strip = flags & CAN_DRAW_STRIP != 0 && half(m, part.part_addr, 2)? & LIST_PART == 0;
        let bitmap = word(m, part.record_addr, 0x14)?;
        let masked = flags & USE_SUBPARTS != 0 && bitmap != 0 && word(m, part.geometry, 8)? != 0;
        Ok(Self { geometry: part.geometry, part: part.part_addr, strip, bitmap,
            pending: None, position: if masked { Position::First } else { Position::Whole } })
    }

    fn range(&self, m: &dyn GuestRead, start: u32, count: u32) -> Result<IndexedRange, Refusal> {
        let base = word(m, self.geometry, 0x24)?;
        Ok(IndexedRange { address: base.wrapping_add(start.wrapping_mul(2)), count })
    }

    fn subpart(m: &dyn GuestRead, descriptor: u32) -> Result<(u32, u32), Refusal> {
        let word = word(m, descriptor, 0)?;
        Ok((word & 0xffff, word >> 16))
    }

    /// Sample through the next D3D call. Call again only after that draw
    /// (including its callbacks), or on a copy to preview an uninterrupted run.
    pub fn next(&mut self, m: &dyn GuestRead) -> Result<Option<IndexedRange>, Refusal> {
        let mut index = match std::mem::replace(&mut self.position, Position::Done) {
            Position::Done => return Ok(None),
            Position::Whole => {
                let first = u32::from(half(m, self.part, 6)?);
                return self.range(m, first, u32::from(half(m, self.part, 8)?)).map(Some);
            }
            Position::First => i32::from(half(m, self.part, 10)? as i16),
            Position::Next(i) => i,
            Position::Reseed { index, descriptor } => {
                self.pending = Some(Self::subpart(m, descriptor)?);
                index + 1
            }
        };
        let end = i32::from(half(m, self.part, 10)? as i16) + i32::from(half(m, self.part, 12)? as i16);
        let mut mask = None;
        while index < end {
            let word_index = index >> 5;
            let bits = match mask {
                Some((previous, bits)) if previous == word_index => bits,
                _ => {
                    let address = self.bitmap.wrapping_add_signed(word_index * 4);
                    let bits = word(m, address, 0)?;
                    mask = Some((word_index, bits));
                    bits
                }
            };
            if bits & (1 << (index & 31)) != 0 {
                let entries = word(m, self.geometry, 12)?;
                let descriptor = entries.wrapping_add_signed(index * SUBPART_SIZE as i32);
                let (start, count) = Self::subpart(m, descriptor)?;
                if self.strip {
                    self.position = Position::Next(index + 1);
                    return self.range(m, start, count).map(Some);
                }
                match self.pending {
                    None => self.pending = Some((start, count)),
                    Some((first, n)) if first + n == start => self.pending = Some((first, n + count)),
                    Some((first, n)) => {
                        self.pending = None;
                        self.position = Position::Reseed { index, descriptor };
                        return self.range(m, first, n).map(Some);
                    }
                }
            }
            index += 1;
        }
        self.pending.take().map(|(start, n)| self.range(m, start, n)).transpose()
    }

    /// The first range was frozen before any texture release. Compare the
    /// rest against the *current* bitmap, bounds, descriptors and index base.
    pub fn matches_prepared(mut self, m: &dyn GuestRead, first: IndexedRange,
        part: &SortedPart) -> Result<bool, Refusal> {
        let Some((expected, tail)) = part.indices.split_first() else { return Ok(false) };
        if self.strip != part.strip || !first.matches(part.index_buffer, expected) {
            return Ok(false);
        }
        for expected in tail {
            let Some(actual) = self.next(m)? else { return Ok(false) };
            if !actual.matches(part.index_buffer, expected) {
                return Ok(false);
            }
        }
        Ok(self.next(m)?.is_none())
    }
}

fn emissions(m: &dyn GuestRead, geometry: u32, part: u32, flags: u32, bitmap: u32,
    strip: bool) -> Result<Vec<Range<u32>>, Refusal> {
    let subparts = word(m, geometry, 8)?;
    if flags & USE_SUBPARTS == 0 || bitmap == 0 || subparts == 0 {
        let first = u32::from(half(m, part, 6)?);
        let count = u32::from(half(m, part, 8)?);
        return Ok(Vec::from([first..first + count]));
    }
    let first = half(m, part, 10)? as i16;
    let count = half(m, part, 12)? as i16;
    let mut spans: Vec<Range<u32>> = Vec::new();
    if count <= 0 {
        return Ok(spans);
    }
    let first = u32::try_from(first).map_err(|_| Refusal::Indices)?;
    let end = first + count as u32;
    if end > subparts {
        return Err(Refusal::Indices);
    }
    let entries = word(m, geometry, 12)?;
    if entries == 0 {
        return Err(Refusal::Indices);
    }
    let mut bits = 0;
    for i in first..end {
        if i == first || i % 32 == 0 {
            bits = word(m, bitmap, (i / 32) * 4)?;
        }
        if bits & (1 << (i % 32)) == 0 {
            continue;
        }
        let indices = word(m, entries, i * SUBPART_SIZE)?;
        let start = indices & 0xffff;
        let end = start + (indices >> 16);
        if !strip && let Some(previous) = spans.last_mut() && previous.end == start {
            previous.end = end;
        } else {
            spans.push(start..end);
        }
    }
    Ok(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;

    const ESP: u32 = 0x1000;
    const SLOT: u32 = 0x1800;
    const BSP: u32 = 0x2000;
    const CLUSTERS: u32 = 0x3000;
    const SECTION: u32 = CLUSTERS + 2 * 0xb0;
    const GEOMETRY: u32 = 0x4000;
    const PARTS: u32 = 0x5000;
    const PART: u32 = PARTS + PART_SIZE;
    const SUBPARTS: u32 = 0x6000;
    const BITMAP: u32 = 0x7000;
    const RECORD: u32 = RENDER_RECORDS + 3 * RENDER_RECORD_SIZE;

    fn scene() -> Mem {
        let mut m = Mem::default();
        for (a, v) in [
            (ESP + 4, 0x1234_0020), (ESP + 8, 17), (ESP + 12, 2), (ESP + 16, 3),
            (ESP + 20, 1), (ESP + 24, 0x8100_0301), (ESP + 28, SLOT),
            (SLOT + 4, 0x104), (SLOT + 8, 2), (SLOT + 16, 0xa000),
            (RECORD_COUNT, 4), (RECORD + 4, u32::MAX), (RECORD + 12, 2 << 9),
            (RECORD, USE_SUBPARTS | CAN_DRAW_STRIP), (RECORD + 0x14, BITMAP),
            (BSP + 0x9c, 3), (BSP + 0xa0, CLUSTERS), (SECTION + 0x50, GEOMETRY),
            (GEOMETRY, 2), (GEOMETRY + 4, PARTS), (GEOMETRY + 8, 36),
            (GEOMETRY + 12, SUBPARTS), (GEOMETRY + 0x20, 400), (GEOMETRY + 0x24, 0x9000),
            (PART, u32::from(LIST_PART) << 16), (PART + 4, 120 << 16),
            (PART + 8, 31 << 16 | 30), (PART + 12, 5),
            (BITMAP, 1 << 31), (BITMAP + 4, 0b1101),
        ] {
            m.w(a, v);
        }
        // Subpart 32 continues the previous run across a mask word.
        // Subpart 33 is invisible and has no record in the synthetic RAM.
        for (i, start) in [(31, 120), (32, 126), (34, 141), (35, 138)] {
            m.w(SUBPARTS + i * 8, 6 << 16 | start);
        }
        m
    }

    #[test]
    fn decodes_the_pass_and_coalesces_only_adjacent_visible_list_slices() {
        let item = SortedPart::read(&scene(), BSP, ESP, SLOT).unwrap();
        assert_eq!((item.owner, item.record, item.part), (Owner::Cluster(2), 3, 1));
        assert_eq!((item.section, item.geometry, item.part_addr), (SECTION, GEOMETRY, PART));
        assert_eq!((item.shader, item.program), (0x1234_0020, 0xa000));
        assert_eq!((item.lod, item.pass, item.implementation), (2, 1, 4));
        assert_eq!((item.layer, item.layer_argument, item.projected_size), (3, 17, 129));
        assert!(!item.strip);
        assert_eq!(item.indices, [120..132, 141..147, 138..144]);
    }

    #[test]
    fn a_strip_restarts_for_each_visible_slice_and_a_blank_mask_emits_nothing() {
        let mut m = scene();
        m.w(PART, 0);
        let item = SortedPart::read(&m, BSP, ESP, SLOT).unwrap();
        assert!(item.strip);
        assert_eq!(item.indices, [120..126, 126..132, 141..147, 138..144]);
        m.w(BITMAP, 0);
        m.w(BITMAP + 4, 0);
        assert!(SortedPart::read(&m, BSP, ESP, SLOT).unwrap().indices.is_empty());
        m.w(RECORD + 0x14, 0);
        assert_eq!(SortedPart::read(&m, BSP, ESP, SLOT).unwrap().indices, [120..150]);
        m.w(RECORD, USE_SUBPARTS);
        assert!(!SortedPart::read(&m, BSP, ESP, SLOT).unwrap().strip);
    }

    #[test]
    fn placements_use_their_definition_and_refuse_invalid_section_indices() {
        let mut m = scene();
        m.w(RECORD, INSTANCE_RECORD);
        m.w(RECORD + 12, 4 << 18);
        m.w(BSP + 0x140, 5);
        m.w(BSP + 0x144, 0xa000);
        m.w(0xa000 + 4 * 0x58 + 0x34, 2);
        m.w(BSP + 0x138, 3);
        m.w(BSP + 0x13c, 0xb000);
        m.w(0xb000 + 2 * 0xc8 + 0x50, GEOMETRY);
        let item = SortedPart::read(&m, BSP, ESP, SLOT).unwrap();
        assert_eq!((item.owner, item.section), (Owner::Instance(4), 0xb000 + 2 * 0xc8));
        m.w(GEOMETRY + 0x20, 149);
        assert_eq!(SortedPart::read(&m, BSP, ESP, SLOT), Err(Refusal::Indices));
        m.w(0xa000 + 4 * 0x58 + 0x34, 0xffff);
        assert_eq!(SortedPart::read(&m, BSP, ESP, SLOT), Err(Refusal::Section));
        assert_eq!(SortedPart::owner_at(&m, BSP, ESP), Some(Owner::Instance(4)));
        m.w(RECORD + 4, 17);
        assert_eq!(SortedPart::owner_at(&m, BSP, ESP), None);
        assert_eq!(SortedPart::read(&m, BSP, ESP, SLOT), Err(Refusal::Object));
    }
    #[test]
    fn follow_slots_read_their_own_handler_arguments_and_stop_at_the_boundary() {
        const LIST: u32 = 0xc000;
        const ITEMS: u32 = 0xd000;
        const CURSOR: u32 = ESP + 0x2c + 2 * ITEM_SIZE;
        let mut m = scene();
        for (address, value) in [
            (ESP + 0x28, 3), (ESP + 28, CURSOR), (CURRENT_LIST, LIST), (CURRENT_LAYER, 3),
            (LIST, 10), (LIST + 4, 9), (LIST + 0xc, 2), (LIST + 0x14, ITEMS),
        ] {
            m.w(address, value);
        }
        for (offset, item_index, shader, lod, pass, argument) in [
            (0, 7, 0x1234_0020, 2, 1, 0x8100_0301),
            (1, 3, 0xabcd_0080, 4, 3, 0x0700_0301),
            (2, 8, 0xef00_0013, 0, 0, 0x8000_0301),
        ] {
            let slot = CURSOR + ITEM_SIZE * offset;
            let program = 0xa000 + offset * 0x200;
            let item = ITEMS + ITEM_SIZE * item_index;
            m.w(slot, item_index);
            m.w(slot + 4, 2 + offset);
            m.w(slot + 8, 1);
            m.w(slot + 0x10, program);
            m.w(program + 0x100, 0xb000);
            m.w(item, GEOMETRY_BUILD);
            m.w(item + 4, GEOMETRY_DRAW);
            m.w(item + 8, shader);
            m.w(item + 12, argument);
            m.w(item + 20, lod << 8 | pass);
        }
        let run = SortedRun::read(&m, LIST, ESP, CURSOR).unwrap();
        assert_eq!((run.counter, run.remaining), (ESP + 0x28, 3));
        assert_eq!(run.next(&m, BSP, 0), SortedPart::read(&m, BSP, ESP, CURSOR));
        let second = run.next(&m, BSP, 1).unwrap();
        assert_eq!((second.shader, second.lod, second.pass), (0xabcd_0080, 4, 3));
        assert_eq!((second.projected_size, second.implementation, second.program), (7, 3, 0xa200));
        assert_eq!(second.sorted_slot, CURSOR + ITEM_SIZE);
        m.w(0xa200 + 0x100, u32::MAX);
        assert_eq!(run.next(&m, BSP, 1), Err(Refusal::OtherHandler));
        m.w(0xa200 + 0x100, 1);
        m.w(ITEMS + 3 * ITEM_SIZE + 4, 0xe000);
        assert_eq!(run.next(&m, BSP, 1), Err(Refusal::OtherHandler));
        assert_eq!(run.next(&m, BSP, 3), Err(Refusal::OtherHandler));
        m.w(ESP + 4, 0xabcd_0080);
        assert!(SortedRun::read(&m, LIST, ESP, CURSOR).is_err());
        m.w(ESP + 0x28, 2559);
        assert!(SortedRun::read(&m, LIST, ESP, CURSOR).is_err());
    }

    #[test]
    fn a_callback_can_change_the_seed_of_the_next_list_run_and_later_visibility() {
        let mut m = scene();
        let part = SortedPart::read(&m, BSP, ESP, SLOT).unwrap();
        let mut cursor = Emitter::start(&m, &part).unwrap();
        let first = cursor.next(&m).unwrap().unwrap();
        assert_eq!(first, IndexedRange { address: 0x9000 + 120 * 2, count: 12 });
        assert!(cursor.matches_prepared(&m, first, &part).unwrap());

        // The emitter saved descriptor 34's address to EDI before the
        // flush. It reads that descriptor's contents after both callbacks.
        m.w(SUBPARTS + 34 * 8, 4 << 16 | 270);
        m.w(GEOMETRY + 12, 0x8000);
        m.w(GEOMETRY + 0x24, 0xa000);
        m.w(BITMAP + 4, 0b0101); // hide subpart 35
        assert!(!cursor.matches_prepared(&m, first, &part).unwrap());
        assert_eq!(cursor.next(&m), Ok(Some(IndexedRange { address: 0xa000 + 270 * 2, count: 4 })));
        assert_eq!(cursor.next(&m), Ok(None));
    }

    #[test]
    fn strip_callback_changes_the_remaining_count_after_the_first_range_is_frozen() {
        let mut m = scene();
        let part = SortedPart::read(&m, BSP, ESP, SLOT).unwrap();
        m.w(PART, 0); // shader setup enabled strips since preparation
        let mut cursor = Emitter::start(&m, &part).unwrap();
        assert!(cursor.strip);
        let first = cursor.next(&m).unwrap().unwrap();
        m.w(GEOMETRY + 0x24, 0xb000);
        m.w(PART + 12, 2);
        assert_eq!(first, IndexedRange { address: 0x9000 + 120 * 2, count: 6 });
        assert!(!cursor.matches_prepared(&m, first, &part).unwrap());
        assert_eq!(cursor.next(&m), Ok(Some(IndexedRange { address: 0xb000 + 126 * 2, count: 6 })));
        assert_eq!(cursor.next(&m), Ok(None));
    }

}
