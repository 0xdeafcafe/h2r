//! A light's item lists and the first-person table: `0x00040e90` in a
//! light's kind, with the record walk `0x00044ac0`
//! (docs/graphics/native-light-gather.md, "One light's passes" step 1 and
//! slice L4).
//!
//! [`plan`] reads and refuses before any write; the main view's kind
//! (`[vis+0x2a78]` 0) is not this body's. [`run`] then makes the guest's
//! steps in its order: both lists' headers zeroed, the attachment walk
//! `0x00041c80` as a guest call, and each of the light's records
//! (`[0x004c0b7c, 0x004c0b78)`):
//!
//! - a record with flag bit 10 is copied whole into the first-person
//!   slot of `[0x00509440] - 1` while that slot holds under 32
//!   ([`first_person`]); after the eighth light the count stops, so a
//!   later light's records join the eighth slot;
//! - a structure record whose parts the port takes appends each visible
//!   part's item to list 1 (`0x004c1a60`) for flag bit 1 and list 2
//!   (`0x004c1a78`) for flag bit 2, list 2 first when both
//!   ([`record_items`], `0x00041040` with the chooser `0x00040de0` and
//!   the append `0x00040e30`, `0x0003b9d0`);
//! - any other record (an object's, an override shader, a shader the
//!   port does not read) is made in place by the guest's per-part calls:
//!   `0x000449e0`, then per part `0x00045ce0`, `0x000460d0` and
//!   `0x00041040`.
//!
//! The record walk's calls are made below the frame the guest keeps there
//! ([`stack`]), so a callee is handed the scratch address the guest hands
//! it.

use std::ops::Range;

use super::frame::{FIRST_PERSON_COUNT, FIRST_PERSON_SLOT, FIRST_PERSON_TABLE};
use super::guest::{NONE, TAGS};
use super::{Refusal, Result, Writes};
use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use crate::light_constants::constant::{ONE, TWO};
use guest_layout::memory::{GuestMemory, MemFault};

/// The guest functions the body calls.
pub mod call {
    /// The attachment walk: EDI the visibility struct, EBX its kind.
    pub const ATTACHMENTS: u32 = 0x0004_1c80;
    /// A record's geometry: EAX the record, DL 0; stack (the placement
    /// bit), `ret 4`.
    pub const RECORD_GEOMETRY: u32 = 0x0004_49e0;
    /// A part's material into the scratch: ECX the record, EAX the
    /// scratch; stack (part, geometry kind), `ret 8`; AL visible.
    pub const PART_MATERIAL: u32 = 0x0004_5ce0;
    /// A part's subpart visibility: EAX the part, EBX the record's mask;
    /// stack (record), `ret 4`; AL visible.
    pub const PART_VISIBLE: u32 = 0x0004_60d0;
    /// A part's items: ECX the part, EDX the record; stack (kind, pixel
    /// size, &scratch, shader, any other selector, and-mask, or-mask),
    /// `ret 0x1c`.
    pub const PART_ITEMS: u32 = 0x0004_1040;
}

/// The guest's frame below the entry `ESP`, which the calls reserve.
pub mod stack {
    /// `0x00040e90` saves four registers before the attachment walk.
    pub const ATTACHMENTS: u32 = 0x10;
    /// The record walk's loop: the four saved, its three arguments and
    /// return address, its `0x40` bytes of locals and four saved more.
    pub const RECORDS: u32 = 0x70;
    /// The part scratch `0x00045ce0` fills and `0x00041040` reads, at the
    /// loop's `ESP + 0x24`.
    pub const SCRATCH: u32 = RECORDS - 0x24;
}

/// The visibility struct the records were written for.
const CURRENT_VIS: u32 = 0x004c_0b80;
const VIS_KIND: u32 = 0x2a78;
pub(super) const COUNT: u32 = 0x004c_0b78;
pub(super) const MAIN_COUNT: u32 = 0x004c_0b7c;
pub(super) const RECORDS: u32 = 0x004b_a138;
pub(super) const RECORD: u32 = 0x20;
pub(super) const PIXEL_SIZES: u32 = 0x004c_0b84;
pub(super) const ATTACHMENT_BYTES: u32 = 0x004c_0ed6;
pub(super) const OVERRIDES: u32 = 0x004c_1228;
/// The scene lists: list `n` at `+0x18 n` (capacity, count, layers,
/// first, items at `+0x14`).
pub const LISTS: u32 = 0x004c_1a48;
pub(super) const LIST: u32 = 0x18;
const ACTIVE_BSP: u32 = 0x004e_0348;
const ACTIVE_BSP_INDEX: u32 = 0x0046_86c4;
const MATERIAL_VIS: u32 = 0x004e_d280;
const PART_PLANES: u32 = 0x0051_f400;
const PLANE_HASH: u32 = 0x0001_e8c0;
const PLANE_EQUAL: u32 = 0x0001_e8f0;
pub(super) const SCREEN_SCALE: u32 = 0x0046_70e4;
pub(super) const ALPHA_SCALE: u32 = 0x0045_dc10;
const DOUBLE_LOD: u32 = 0x004b_a019;
const LOD_SCALES: u32 = 0x0048_5b28;
/// The item's build and draw functions (`0x0004132b`).
const ITEM_BUILD: u32 = 0x0004_cbf0;
const ITEM_DRAW: u32 = 0x0004_d0b0;
const ITEM: u32 = 0x18;
pub(super) const PART: u32 = 0x48;
/// The shader groups whose definition is reached through a virtual call.
const VIRTUAL_GROUPS: [u32; 2] = [u32::from_be_bytes(*b"MTPR"), u32::from_be_bytes(*b"3trp")];
/// The record walk's part selector (`0x00040f09`).
const SELECTOR: u32 = 0x1f;
const SLOT_RECORDS: i16 = 32;

/// The record at index `i` (`movsx`: the plan keeps `i` under `0x8000`).
pub(super) fn record_at(i: u32) -> u32 {
    RECORDS.wrapping_add(i.wrapping_mul(RECORD))
}

/// What [`run`] starts from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub entry_esp: u32,
    /// EAX: the mode, which picks the record filter.
    pub mode: u32,
    pub vis: u32,
    /// `[vis+0x2a78]`, not 0.
    pub kind: u32,
    /// The flag bits a record must have one of (`0x00040e9d`).
    pub filter: u32,
}

/// Reads and refuses before any write. `Ok(None)`: the main view's kind,
/// which this body does not make.
pub fn plan(m: &dyn GuestMemory, entry_esp: u32, mode: u32) -> Result<Option<Plan>> {
    let vis = m.u32(CURRENT_VIS)?;
    let kind = m.u32(vis.wrapping_add(VIS_KIND))?;
    if kind == 0 {
        return Ok(None);
    }
    // The headers the body zeroes and appends to.
    m.bytes(LISTS + LIST, 2 * LIST as usize)?;
    m.u32(FIRST_PERSON_COUNT)?;
    let end = m.u32(COUNT)? & 0xffff;
    let start = m.u32(MAIN_COUNT)? & 0xffff;
    if end > 0x8000 || start > 0x8000 {
        return Err(Refusal::Unsupported);
    }
    // The frame the record walk's calls are made below.
    m.bytes(
        entry_esp.wrapping_sub(stack::RECORDS),
        stack::RECORDS as usize,
    )?;
    let filter = match mode {
        0 => u32::MAX,
        2 => 0x10,
        _ => 0x20,
    };
    Ok(Some(Plan {
        entry_esp,
        mode,
        vis,
        kind,
        filter,
    }))
}

/// One list's header after the walk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ItemRange {
    /// `+0xc`: zeroed.
    pub first: u32,
    /// `+4`.
    pub count: u32,
    /// `+8`: the layers of its items.
    pub layers: u32,
}

/// The bit-10 copy of one record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FirstPerson {
    /// Copied into `slot` (`[0x00509440] - 1`) at `index`.
    Copied { slot: i32, index: i16 },
    /// The slot holds 32: the record is dropped.
    Full { slot: i32 },
}

/// How one record that passed the filters was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Made {
    /// Its items were appended in Rust.
    Rust,
    /// The guest's per-part calls made it.
    Guest(Refusal),
}

/// What one call did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LightItems {
    pub kind: u32,
    /// The light's records, `[0x004c0b7c, 0x004c0b78)`.
    pub records: Range<u32>,
    /// List 1 and list 2.
    pub receivers: ItemRange,
    pub casters: ItemRange,
    /// Per record that passed the filters, in order.
    pub made: Vec<(u32, Made)>,
    pub first_person: Vec<FirstPerson>,
}

/// One scene list's header, as the append `0x0003b9d0` reads it.
#[derive(Clone, Copy)]
pub(super) struct List {
    pub(super) header: u32,
    pub(super) capacity: i32,
    pub(super) count: i32,
    pub(super) layers: u32,
    pub(super) items: u32,
}

impl List {
    pub(super) fn read(m: &dyn GuestMemory, n: u32) -> Result<Self, MemFault> {
        let header = LISTS + LIST * n;
        Ok(Self {
            header,
            capacity: m.i32(header)?,
            count: m.i32(header + 4)?,
            layers: m.u32(header + 8)?,
            items: m.u32(header + 0x14)?,
        })
    }

    fn range(&self, m: &dyn GuestMemory) -> Result<ItemRange, MemFault> {
        Ok(ItemRange {
            first: m.u32(self.header + 0xc)?,
            count: self.count as u32,
            layers: self.layers,
        })
    }

    /// `0x0003b9d0` with the and-mask -1 and the or-mask 0 `0x00041040`
    /// passes: nothing when the list is full.
    pub(super) fn append(&mut self, out: &mut Writes, shader: u32, level: u32, layers: u32, argument: u32) {
        if self.count >= self.capacity {
            return;
        }
        let at = self
            .items
            .wrapping_add((self.count as u32).wrapping_mul(ITEM));
        self.count += 1;
        out.word(self.header + 4, self.count as u32);
        let mut item = Vec::with_capacity(22);
        for word in [ITEM_BUILD, ITEM_DRAW, shader, argument, layers] {
            item.extend_from_slice(&word.to_le_bytes());
        }
        item.extend_from_slice(&[0, level as u8]);
        out.array(at, item);
        self.layers |= layers;
        out.word(self.header + 8, self.layers);
    }
}

/// A shader's level-of-detail table, as `0x0001bf50` and `0x00041040`
/// read it.
pub(super) struct Lod {
    pub(super) classification: u32,
    pub(super) levels: u32,
    pub(super) count: i32,
}

impl Lod {
    pub(super) fn read(m: &dyn GuestMemory, shader: u32) -> Result<Self> {
        let index = shader & 0xffff;
        if index > 0x7fff {
            return Err(Refusal::Unsupported);
        }
        let table = m.u32(TAGS)?;
        let entry = table.wrapping_add(index * 16);
        if VIRTUAL_GROUPS.contains(&m.u32(entry)?) {
            return Err(Refusal::Unsupported);
        }
        let data = m.u32(entry.wrapping_add(8))?;
        let classification = u32::from(m.u16(data.wrapping_add(0x3c))?);
        let instance = m.u32(data.wrapping_add(0x24))?;
        let template = m.u32(instance)?;
        let template = m.u32(table.wrapping_add((template & 0xffff) * 16 + 8))?;
        let header = m.u32(template.wrapping_add(0x5c))?;
        let count = m.i32(header)?;
        if count <= 0 {
            // The level would be -1 and its row read before the table.
            return Err(Refusal::Unsupported);
        }
        Ok(Self {
            classification,
            levels: m.u32(header.wrapping_add(4))?,
            count,
        })
    }

    /// `0x0001bf50`: the first level whose threshold the scaled size
    /// reaches (`comiss`, `jae`: a NaN reaches none), at most the last.
    pub(super) fn level(&self, m: &dyn GuestMemory, pixels: f32) -> Result<u32, MemFault> {
        let mut level = 0u32;
        if self.classification != 5 {
            let mut p = pixels;
            if m.u8(DOUBLE_LOD)? != 0 {
                p *= m.f32(TWO)?;
            }
            let p = m.f32(LOD_SCALES.wrapping_add(self.classification * 4))? * p;
            while (level as i32) < self.count {
                if p >= m.f32(self.levels.wrapping_add(level * 10 + 6))? {
                    break;
                }
                level += 1;
            }
        }
        Ok(level.min(self.count as u32 - 1))
    }

    pub(super) fn layers(&self, m: &dyn GuestMemory, level: u32) -> Result<u32, MemFault> {
        m.u32(self.levels.wrapping_add(level * 10 + 2))
    }
}

/// `0x00184000`: a material's visibility bit in the active BSP's masks.
fn material_visible(m: &dyn GuestMemory, bit: u8, placement: u32) -> Result<bool, MemFault> {
    if bit == 0xff {
        return Ok(true);
    }
    let bsp = m.u16(ACTIVE_BSP_INDEX)? as i16 as i32 as u32;
    let masks = m.u32(MATERIAL_VIS)?;
    let base = if placement == NONE {
        masks.wrapping_add(bsp.wrapping_mul(32)).wrapping_add(1)
    } else {
        masks
            .wrapping_add(bsp.wrapping_mul(0x400))
            .wrapping_add(placement)
            .wrapping_add(0x201)
    };
    let bit = u32::from(bit);
    Ok(m.u32(base.wrapping_add((bit >> 5) * 4))? & (1 << (bit & 31)) != 0)
}

/// `0x000460d0` for a record with flag bit 6: any of the part's subparts
/// set in the record's mask.
pub(super) fn part_visible(m: &dyn GuestMemory, part: u32, mask: u32) -> Result<bool, MemFault> {
    let first = i32::from(m.u16(part.wrapping_add(0xa))? as i16);
    let count = i32::from(m.u16(part.wrapping_add(0xc))? as i16);
    for d in first..first + count {
        let word = m.u32(mask.wrapping_add(((d >> 5) as u32).wrapping_mul(4)))?;
        if word & (1 << (d & 31)) != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The plane lookup `0x0001e280` that `0x00045ce0` makes for a visible
/// structure part writes only the scratch when its table's callbacks are
/// the two pure ones.
fn planes(m: &dyn GuestMemory) -> Result<()> {
    let table = m.u32(PART_PLANES)?;
    if table == 0
        || m.u32(table.wrapping_add(0x20))? == 0
        || m.u32(table.wrapping_add(0x2c))? != PLANE_HASH
        || m.u32(table.wrapping_add(0x30))? != PLANE_EQUAL
    {
        return Err(Refusal::Unsupported);
    }
    Ok(())
}

/// The bit-10 copy of the record at `record` into the slot of
/// `[0x00509440] - 1` (`0x00044b3a`..`0x00044b66`), with the guest's
/// arithmetic: no check of the slot number.
pub fn first_person(m: &dyn GuestMemory, record: u32) -> Result<(Writes, FirstPerson), MemFault> {
    let n = m.i32(FIRST_PERSON_COUNT)?;
    let slot = FIRST_PERSON_TABLE
        .wrapping_sub(FIRST_PERSON_SLOT)
        .wrapping_add((n as u32).wrapping_mul(FIRST_PERSON_SLOT));
    let count_at = slot.wrapping_add(0x404);
    let index = m.u16(count_at)? as i16;
    let mut out = Writes::new();
    if index >= SLOT_RECORDS {
        return Ok((
            out,
            FirstPerson::Full {
                slot: n.wrapping_sub(1),
            },
        ));
    }
    let at = slot.wrapping_add((i32::from(index) as u32).wrapping_mul(RECORD));
    out.array(at, m.bytes(record, RECORD as usize)?);
    out.half(count_at, (index as u16).wrapping_add(1));
    Ok((
        out,
        FirstPerson::Copied {
            slot: n.wrapping_sub(1),
            index,
        },
    ))
}

/// The record filters of `0x00044b72`..`0x00044baa`: one of the filter's
/// bits, the selector in `+0xc`, flag bit 0 and a non-zero alpha.
/// The first-person lights' record loop (`0x00040380`..`0x000403bb`)
/// makes the same tests on a slot's record.
pub(super) fn selected(m: &dyn GuestMemory, record: u32, filter: u32) -> Result<bool, MemFault> {
    let flags = m.u32(record)?;
    Ok(flags & filter & 0x1f_ffff != 0
        && (m.u32(record + 0xc)? >> 4) & 0x1f == SELECTOR
        && flags & 1 != 0
        && m.u8(record + 0x1a)? != 0)
}

/// The items of the structure record `i` in a light's `kind`
/// (`0x00044bb0`..`0x00044c6a` and `0x00041040`'s light arm), as writes,
/// or why the guest's per-part calls make it. Reads only: the record's
/// writes are published by the caller before the next record is planned.
pub fn record_items(m: &dyn GuestMemory, i: u32, kind: u32) -> Result<Writes> {
    record_items_with(m, i, kind, false)
}

/// [`record_items`] with the fifth argument `0x00041040` is handed: set
/// (the first-person lights' record loop passes 1, `0x00040464`), a part
/// whose level has no layer but bit 15 is still appended
/// (`0x000412ad`).
pub fn record_items_with(
    m: &dyn GuestMemory,
    i: u32,
    kind: u32,
    keep_empty: bool,
) -> Result<Writes> {
    let record = record_at(i);
    let flags = m.u32(record)?;
    if m.u32(record + 4)? != NONE {
        // An object's record: its parts' appends stay the guest's.
        return Err(Refusal::Unsupported);
    }
    if m.u8(ATTACHMENT_BYTES.wrapping_add(i))? != 0xff {
        return Err(Refusal::Unsupported);
    }
    let packed = m.u32(record + 0xc)?;
    let mask = m.u32(record + 0x14)?;
    let pixel = m.u8(PIXEL_SIZES.wrapping_add(i))?;
    let bsp = m.u32(ACTIVE_BSP)?;
    // 0x000449e0, DL 0: no resource touch.
    let section = if (flags >> 12) & 1 == 0 {
        m.u32(bsp.wrapping_add(0xa0))?
            .wrapping_add(((packed >> 9) & 0x1ff) * 0xb0)
    } else {
        let at = m
            .u32(bsp.wrapping_add(0x144))?
            .wrapping_add(((packed >> 18) & 0x7ff) * 0x58);
        let definition = m.u16(at.wrapping_add(0x34))? as i16 as i32 as u32;
        m.u32(bsp.wrapping_add(0x13c))?
            .wrapping_add(definition.wrapping_mul(0xc8))
    };
    let mut out = Writes::new();
    let geometry = m.u32(section.wrapping_add(0x50))?;
    if geometry == 0 {
        return Ok(out);
    }
    let count = m.i32(geometry)?;
    if count > 0x1_0000 {
        return Err(Refusal::Unsupported);
    }
    let parts = m.u32(geometry.wrapping_add(4))?;
    let materials = m.u32(bsp.wrapping_add(0xa8))?;
    let placement = match (packed >> 18) & 0x7ff {
        0x7ff => NONE,
        p => p,
    };
    let (receiver, caster) = (flags & 2 != 0, flags & 4 != 0 && kind != 3);
    let mut lists = [List::read(m, 1)?, List::read(m, 2)?];
    let (one, screen, alpha) = (m.f32(ONE)?, m.f32(SCREEN_SCALE)?, m.f32(ALPHA_SCALE)?);
    for j in 0..count.max(0) as u32 {
        let part = parts.wrapping_add(j * PART);
        // 0x00045ce0: the part's material and its visibility bit.
        let number = m.u16(part.wrapping_add(4))? as i16 as i32 as u32;
        let material = materials.wrapping_add(number.wrapping_mul(32));
        if !material_visible(m, m.u8(material.wrapping_add(0x1c))?, placement)? {
            continue;
        }
        planes(m)?;
        // 0x000460d0.
        if flags & 0x40 != 0 && !part_visible(m, part, mask)? {
            continue;
        }
        let shader = m.u32(material.wrapping_add(0xc))?;
        if shader == NONE {
            continue;
        }
        // 0x00041040.
        if matches!(m.u16(part)?, 0 | 5) {
            continue;
        }
        let size = m.f32(part.wrapping_add(0x2c))?;
        let size = if size > 0.0 { size } else { one };
        let pixels = ((f32::from(pixel) * size) * screen) * alpha;
        let lod = Lod::read(m, shader)?;
        let level = lod.level(m, pixels)?;
        let layers = lod.layers(m, level)?;
        if layers & 0x8080 != 0 && layers & 0xa == 0 && lod.layers(m, 0)? & (1 << 21) != 0 {
            continue;
        }
        if layers & 0xffff_7fff == 0 && !keep_empty {
            continue;
        }
        let argument = ((u32::from(pixel) << 16 | i) << 8) | j;
        // 0x000412e9: list 2 first for both bits; then the chooser's list.
        if receiver && caster {
            lists[1].append(&mut out, shader, level, layers, argument);
        }
        if receiver {
            lists[0].append(&mut out, shader, level, layers, argument);
        } else if caster {
            lists[1].append(&mut out, shader, level, layers, argument);
        }
    }
    Ok(out)
}

async fn guest(cx: &Cx, call: GuestCall, reserve: u32) -> Result<u32, EntryError> {
    Ok(cx.call(call.reserve(reserve)).await?.eax)
}

/// Record `i` by the guest's per-part calls, as the record walk makes it.
async fn guest_record(cx: &Cx, plan: &Plan, i: u32) -> Result<(), EntryError> {
    let record = record_at(i);
    let scratch = plan.entry_esp.wrapping_sub(stack::SCRATCH);
    let (flags, kind_of_geometry, pixel, shader_override) =
        cx.read(|m| -> Result<_, MemFault> {
            let slot = m.u8(ATTACHMENT_BYTES.wrapping_add(i))?;
            let shader_override = match slot {
                0xff => NONE,
                s => m.u32(OVERRIDES + 4 * u32::from(s))?,
            };
            Ok((
                m.u32(record)?,
                m.u32(record + 0xc)? & 0xf,
                u32::from(m.u8(PIXEL_SIZES.wrapping_add(i))?),
                shader_override,
            ))
        })?;
    let request = GuestCall::new(call::RECORD_GEOMETRY)
        .eax(i)
        .edx(0)
        .args([(flags >> 12) & 0xffff_ff01]);
    let geometry = guest(cx, request, stack::RECORDS).await?;
    if geometry == 0 {
        return Ok(());
    }
    let mut j = 0u32;
    while (j as i32) < cx.read(|m| m.i32(geometry))? {
        let request = GuestCall::new(call::PART_MATERIAL)
            .ecx(i)
            .eax(scratch)
            .args([j, kind_of_geometry]);
        if guest(cx, request, stack::RECORDS).await? as u8 != 0 {
            let mask = cx.read(|m| m.u32(record + 0x14))?;
            let request = GuestCall::new(call::PART_VISIBLE)
                .eax(j)
                .ebx(mask)
                .args([i]);
            if guest(cx, request, stack::RECORDS).await? as u8 != 0 {
                let shader = match shader_override {
                    NONE => cx.read(|m| m.u32(scratch))?,
                    s => s,
                };
                if shader != NONE {
                    // 0x00044c42: SETNE of the selector against 0x1f, which
                    // it is, over the override in ECX.
                    let any = shader_override & !0xff;
                    let request = GuestCall::new(call::PART_ITEMS).ecx(j).edx(i).args([
                        plan.kind,
                        pixel,
                        scratch,
                        shader,
                        any,
                        u32::MAX,
                        0,
                    ]);
                    guest(cx, request, stack::RECORDS).await?;
                }
            }
        }
        j += 1;
    }
    Ok(())
}

/// Makes the call. Returns as `0x00040e90` does in a light's kind
/// (`ret`, EAX the record walk's end) with what it did.
pub async fn run(cx: &Cx, plan: Plan) -> Result<(Ret, LightItems), EntryError> {
    // 0x00040ee0: both lists' count, first and layers.
    cx.write(|m| -> Result<(), MemFault> {
        for header in [LISTS + LIST, LISTS + 2 * LIST] {
            for offset in [4, 0xc, 8] {
                m.set_u32(header + offset, 0)?;
            }
        }
        Ok(())
    })?;
    let request = GuestCall::new(call::ATTACHMENTS)
        .eax(plan.mode)
        .ebx(plan.kind)
        .esi(plan.filter)
        .edi(plan.vis);
    guest(cx, request, stack::ATTACHMENTS).await?;

    // 0x00044ac0 (kind, filter, 0x1f).
    let (start, end) = cx.read(|m| -> Result<_, MemFault> {
        Ok((m.u32(MAIN_COUNT)? & 0xffff, m.u32(COUNT)? & 0xffff))
    })?;
    if end > 0x8000 {
        return Err(EntryError::IsolatedCall(format!(
            "light records end at {end:#x}"
        )));
    }
    let mut items = LightItems {
        kind: plan.kind,
        records: start..end.max(start),
        receivers: ItemRange::default(),
        casters: ItemRange::default(),
        made: Vec::new(),
        first_person: Vec::new(),
    };
    for i in start..end {
        let record = record_at(i);
        if cx.read(|m| m.u32(record))? & 0x400 != 0 {
            let (writes, copied) = cx.read(|m| first_person(m, record))?;
            cx.write(|m| writes.apply(m))?;
            items.first_person.push(copied);
            continue;
        }
        if !cx.read(|m| selected(m, record, plan.filter))? {
            continue;
        }
        match cx.read(|m| record_items(m, i, plan.kind)) {
            Ok(writes) => {
                cx.write(|m| writes.apply(m))?;
                items.made.push((i, Made::Rust));
            }
            Err(why) => {
                guest_record(cx, &plan, i).await?;
                items.made.push((i, Made::Guest(why)));
            }
        }
    }
    let (receivers, casters) = cx.read(|m| -> Result<_, MemFault> {
        Ok((List::read(m, 1)?.range(m)?, List::read(m, 2)?.range(m)?))
    })?;
    items.receivers = receivers;
    items.casters = casters;
    Ok((Ret::new(end, 0), items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scratch_is_inside_the_record_walks_frame() {
        assert_eq!(stack::SCRATCH, 0x4c);
        // Its 0x2c bytes end below the walk's return address and arguments.
        assert!(stack::SCRATCH - 0x2c >= 0x20 && stack::SCRATCH < stack::RECORDS);
        assert_eq!(FIRST_PERSON_TABLE - FIRST_PERSON_SLOT, 0x004c_62f8);
    }
}
