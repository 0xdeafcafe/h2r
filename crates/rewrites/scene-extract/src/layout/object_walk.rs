//! Layer-3 object dispatches after the guest has selected render records
//! and sorted the layer. See docs/graphics/native-renderer.md.
//!
//! ## What a render record's flags make the geometry draw do
//!
//! The geometry draw `0x0004d0b0` (an item's `+4`) reads four bits of the
//! render record's `+0` in layer 3 (**Code**, docs/graphics/framescene.md,
//! slice 3). A part is in the scene with the bits the native pass draws
//! as [`PartFlags`]; a part whose flags' inputs do not read refuses, by
//! its own cause ([`Refusal`]).
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
//!   Without bit 9 it draws a list whatever the part's flag says: such a
//!   part is in the scene without the flag, and the native models draw a
//!   strip part's indices as a list for it
//!   (`native_render::object_program_gpu::LitPart::listed`).
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
//!   subparts, however many ([`Subparts`]); a negative first subpart or a
//!   mask that does not read refuses ([`Refusal::Subparts`]).
//! - **Bit 11, the auxiliary state toggle** ([`PartFlags::AUX_DEPTH`]).
//!   The prologue calls `0x00012d50` with BL 1 and the epilogue with BL
//!   0: it sets the device's program clip range and the view-projection
//!   rows `c[0..3]`, both scaled on ([`crate::layout::aux_depth`]). The
//!   part is in the scene and draws with its view's
//!   [`frame_scene::AuxDepth`]; when the toggle's inputs do not read it
//!   refuses ([`Refusal::AuxState`]).
//!
//! ## What else the draw takes from the record, the shader and the item
//!
//! - **The fade byte** (record `+0x1a`, [`PartFlags::FADE`]). Below `0xff`
//!   the prologue `0x0004d640` scales it by `0x0045dbb4` and, when that
//!   differs from the engine's cached fade `0x004b8494` by more than
//!   `0x0045dbdc`, sets the polygon stipple pattern `0x0001c290` picks
//!   for it (one of 17 at `0x00487288`) through `SetStipple`
//!   (`0x003f8cc0`) and caches the fade. The epilogue `0x0004d720` does
//!   the same for the engine's one (`0x0045dbc0`). The pattern masks the
//!   draw only while the device's stipple is on: the layer draws' target
//!   mode `0x00015680` turns it on (mode 1) for layers 1 and 3 outside
//!   view kinds 4 to 7 and off for layers 5, 8 and 20
//!   (`0x0002be17`..`0x0002bfef`). The part is in the scene with the flag;
//!   the host decides from the device.
//! - **The shader's flag bit 0** (`+0x16`, [`PartFlags::SHADER_FLAG`]).
//!   The draw then takes no lightmap in layer 3 (`0x0004d2ab`) and no PRT
//!   stream in layer 5 (`0x0004d237`); it asks for the stage bitmaps of
//!   the shader's templates (`0x0004d486`..`0x0004d55d`: `0x0012ccf0`,
//!   then `0x0012ce00` for one not resident), residency only; with its
//!   program's flag bit 20 (`0x0004d561`..`0x0004d572`) it calls
//!   `0x0003bea0`, which, the first time a frame (`0x00509430` clear,
//!   cleared at `0x0002c0f5`), clears the shader cache (`0x00016b10`,
//!   which first writes the cached program's restore states) and, when the
//!   active structure has a parameter block, copies and binds the
//!   structure's target (`0x000352e0`, `0x00014bc0`); and the prologue,
//!   when the active structure (`[0x004e0348]`) has a parameter block
//!   (`+0x214` above 0, the block at `+0x218`), runs `0x0003c650` (the
//!   bump matrices of stages 1 to 3) and `0x0003d000` (`c[21..25]` and the
//!   fog colour). The scene carries the case with no structure lightmap
//!   ([`frame_scene::LayerObjectRefusal::ShaderFlag`]); with a parameter
//!   block the part also has [`PartFlags::STRUCTURE_PARAMETERS`] and its
//!   view the upload ([`crate::layout::structure_parameters`]). Layer 3's
//!   walk reads the entry's program and refuses bit 20; a world layer's
//!   entry is built under the layer draw, so its part has
//!   [`PartFlags::TARGET_UNCOPIED`] while the frame's copy is not made,
//!   and the host, which knows the programs of the passes it draws,
//!   keeps the item whose program has the bit.
//! - **The item's argument** (`+0xc`: its high byte, the record, the
//!   part). The high byte is the item's projected size: the pass setup
//!   `0x0004d9c0` scales it by the part's `+0x2c`, `0x004670e4` and
//!   `0x0045dc10`, and the shader begin `0x00016b90` turns that into the
//!   two distance words at the shader context's `+0x28` and `+0x2c`
//!   (`0x00016c6e`..`0x00016cb0`). Only the texture-stage resolve reads
//!   them, for the level of detail it asks residency for
//!   (`0x0001b0a0`), so the
//!   byte changes no render state, constant or binding of the draw. The
//!   item's `+0x15` and `+0x14` bytes are the draw's third and fifth
//!   arguments (`0x0003bc7b`..`0x0003bc8d`), the shader level and the
//!   pass within the layer `0x00016b90` selects (`[template+0x5c]+4`
//!   indexed by the level, its layer's first pass plus the pass), so
//!   each item draws one pass at one level: the scene item carries them
//!   (`frame_scene::LayerItem::level`, `pass`). In a world layer the
//!   argument's low byte is the item's part (`0x0004d1d8` indexes the
//!   section's parts by it), so `extract::layer_item` takes the key's
//!   part from it; the layer-3 walk refuses a low byte other than its
//!   key's (`walk_part_argument`).

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
/// The active structure, its parameter block count (`> 0` makes the
/// prologue upload the block for a flagged shader), and the frame's
/// structure target copy flag `0x0003bea0` sets (a byte).
const ACTIVE_BSP: u32 = 0x004e_0348;
const BSP_PARAMETER_COUNT: u32 = 0x214;
pub(crate) const BSP_TARGET_READY: u32 = 0x0050_9430;
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
    /// Bit 11, and what the toggle around the draw reads does not read
    /// ([`crate::layout::aux_depth::read`]).
    AuxState,
    /// Bit 6 with a subpart mask and subparts, whose visible subparts
    /// cannot be read: a negative first one, or a mask that does not
    /// read.
    Subparts,
    /// A flagged shader (flag bit 0) on a record with a structure
    /// lightmap index, which the draw leaves unbound.
    ShaderFlag,
    /// The item's `+0x14` or `+0x15` is set, or its argument's low byte
    /// is not the key's part: another pass, level or part.
    Argument,
    /// The entry's program has flag bit 20: its draw calls `0x0003bea0`,
    /// which the native pass does not.
    Program,
    Other,
}

/// Parts refused by each named cause in a walk (items, not objects).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Causes {
    pub aux_state: usize,
    pub subparts: usize,
    pub shader_flag: usize,
    pub argument: usize,
    pub program: usize,
    /// Every other refusal: the item's layout, a check against the
    /// guest's choice, or memory that does not read.
    pub layout: usize,
}

impl Causes {
    fn add(&mut self, why: Refusal) {
        match why {
            Refusal::AuxState => self.aux_state += 1,
            Refusal::Subparts => self.subparts += 1,
            Refusal::ShaderFlag => self.shader_flag += 1,
            Refusal::Argument => self.argument += 1,
            Refusal::Program => self.program += 1,
            Refusal::Other => self.layout += 1,
        }
    }
}

/// The [`PartFlags`] of a part whose record has `flags` at `+0`:
/// `triangle_list` the part's flag, `subparts` whether the record's
/// subpart mask and the section's subpart count are both nonzero. The
/// emitter draws a strip only with bit 9 on a part that is not a list.
pub fn record_flags(flags: u32, triangle_list: bool, subparts: bool) -> PartFlags {
    let mut out = PartFlags::NONE;
    if flags & RECORD_AUX_STATE != 0 {
        out |= PartFlags::AUX_DEPTH;
    }
    if flags & RECORD_SUBPARTS != 0 && subparts {
        out |= PartFlags::SUBPARTS;
    }
    if !triangle_list && flags & RECORD_STRIP != 0 {
        out |= PartFlags::STRIP;
    }
    if flags & RECORD_STENCIL_MARK != 0 {
        out |= PartFlags::STENCIL_MARK;
    }
    out
}

/// The subparts of a part (its first subpart `first` and count `count`,
/// the part's signed `+0xa` and `+0xc`) that a visible-subpart mask at
/// `mask` lets the emitter draw: bit `s & 31` of word `s >> 5` for the
/// section's subpart `s` (`0x0004db08`..`0x0004db17`). A count of zero or
/// less draws none.
pub fn visible_subparts(mem: &dyn GuestRead, mask: u32, first: i16, count: i16) -> Result<Subparts, Refusal> {
    if first < 0 {
        return Err(Refusal::Subparts);
    }
    let count = count.max(0) as u16;
    let mut visible = vec![0u64; usize::from(count).div_ceil(64)];
    let mut at = None;
    let mut w = 0;
    for n in 0..count {
        let s = first as u32 + u32::from(n);
        if at != Some(s >> 5) {
            w = word(mem, mask, 4 * (s >> 5)).ok_or(Refusal::Subparts)?;
            at = Some(s >> 5);
        }
        visible[usize::from(n / 64)] |= u64::from(w >> (s & 31) & 1) << (n % 64);
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

#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub fn rows(&self, mem: &dyn GuestRead) -> Option<Vec<Row>> {
        let rows = objects::record_rows(mem, self.key.record).ok()?;
        rows.iter().flatten().all(|v| v.is_finite()).then_some(rows)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    let (entries, causes) = entries_at(mem, list, base, count)?;
    Some(SortedObjects { base, current: current as usize, counter, entries, causes })
}

/// Where [`read_slots`] places the slots it is given: no guest memory.
pub const SLOTS_BASE: u32 = 0xf000_0000;

/// The entries of sort slots `slots` the Rust build made for `list`'s
/// layer 3 (`native_world::scene_build`, ordered by
/// `layer_sort::geometry`), as [`read`] reads the guest's at the handler:
/// each slot read where [`SLOTS_BASE`] places it, the rest from `mem`.
pub fn read_slots(mem: &dyn GuestRead, list: u32, slots: &[[u8; 24]]) -> Option<(Vec<Entry>, Causes)> {
    let bytes = slots.as_flattened();
    let over = Overlay { mem, base: SLOTS_BASE, bytes };
    entries_at(&over, list, SLOTS_BASE, u32::try_from(slots.len()).ok()?)
}

/// `mem` with `bytes` at `base`.
struct Overlay<'a> {
    mem: &'a dyn GuestRead,
    base: u32,
    bytes: &'a [u8],
}

impl GuestRead for Overlay<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        match addr.checked_sub(self.base) {
            Some(at) if (at as usize) < self.bytes.len() => {
                let at = at as usize;
                let word = self.bytes.get(at..at + 4)?;
                Some(u32::from_le_bytes(word.try_into().ok()?))
            }
            _ => self.mem.read_u32(addr),
        }
    }
}

/// The `count` entries of 0x18 bytes at `base` of a layer-3 walk over
/// `list`.
fn entries_at(mem: &dyn GuestRead, list: u32, base: u32, count: u32) -> Option<(Vec<Entry>, Causes)> {
    if count > MAX_LAYER_ENTRIES {
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
    // Whether the toggle's inputs read, asked at the first part with bit 11.
    let mut aux_reads = None;
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
            read_part(mem, slot, item, render, program, packed, &mut aux_reads).inspect_err(|&why| causes.add(why)).ok()
        } else {
            None
        };
        entries.push(Entry::Object { handle, part });
    }
    Some((entries, causes))
}

fn read_part(
    mem: &dyn GuestRead,
    slot: u32,
    item: u32,
    record: u32,
    program: u32,
    packed: u32,
    aux_reads: &mut Option<bool>,
) -> Result<Part, Refusal> {
    let mut why = Refusal::Other;
    let laid = read_part_layout(mem, slot, item, record, program, packed, &mut why);
    let (part, flags, conditional, extra) = laid.ok_or(why)?;
    let flags = record_flags(flags, part.triangle_list, conditional.is_some()) | extra;
    if flags.contains(PartFlags::AUX_DEPTH)
        && !*aux_reads.get_or_insert_with(|| crate::layout::aux_depth::read(mem).is_some())
    {
        return Err(Refusal::AuxState);
    }
    let subparts = match conditional {
        Some(c) if flags.contains(PartFlags::SUBPARTS) => Some(visible_subparts(mem, c.mask, c.first, c.count)?),
        _ => None,
    };
    Ok(Part { flags, subparts, ..part })
}

/// What an object item of a layer draw (`layer`: one of the world layers
/// 1, 5, 7, 8 and 20, or 3 for the first person's items, which the sorted
/// walk does not read; `item` its list entry, `record` its render record,
/// `key` its key) makes the geometry draw `0x0004d0b0` do beyond its
/// material, or why the scene cannot reproduce it (**Code**; the module
/// notes):
///
/// - Outside layer 3 the draw takes no lightmap (`0x0004d28b`: the page
///   and its coordinate stream only for layer 3), and in layer 5 it adds
///   the model's PRT stream for an object's record (`0x0004d220`,
///   `0x00024040` and `0x00024490`), which [`crate::layout::prt`] reads,
///   unless the shader has flag bit 0.
/// - The prologue `0x0004d640` sets the polygon stipple from the record's
///   fade byte (`+0x1a`) unless it is `0xff` ([`PartFlags::FADE`]),
///   toggles the auxiliary state with bit 11, and marks the stencil with
///   bit 14 only in layers 1 and 3 (`0x0004d6b5`): elsewhere the bit is
///   no flag.
/// - With the shader's flag bit 0 ([`PartFlags::SHADER_FLAG`]) the draw
///   asks for its bitmaps' residency, and the prologue uploads the active
///   structure's parameter block when it has one
///   ([`PartFlags::STRUCTURE_PARAMETERS`]); the entry's program, which
///   with its flag bit 20 makes the draw call `0x0003bea0` first, is
///   built under the layer draw, so the part has
///   [`PartFlags::TARGET_UNCOPIED`] while the frame's copy is not made,
///   for the host to keep an item whose program has the bit. In layer 3 a
///   flagged record with a structure lightmap index refuses
///   ([`LayerObjectRefusal::ShaderFlag`]).
/// - The argument's high byte reaches only the residency the stage
///   resolve asks for. Its low byte is the part, `key`'s
///   (`extract::layer_item` puts it there), and the item's `+0x14` and
///   `+0x15` (the pass and level its draw sets up) are the scene item's
///   `pass` and `level`.
/// - Bits 9 and 6 as in layer 3 ([`record_flags`], [`visible_subparts`]).
///
/// [`LayerObjectRefusal::ShaderFlag`]: frame_scene::LayerObjectRefusal::ShaderFlag
pub fn layer_part(
    mem: &dyn GuestRead, layer: u8, _item: u32, record: u32, key: ItemKey,
) -> Result<(PartFlags, Option<Subparts>), frame_scene::LayerObjectRefusal> {
    use frame_scene::LayerObjectRefusal as R;
    let fade = (half(mem, record, RECORD_DRAW_FLAGS).ok_or(R::Unread)? as u8) != u8::MAX;
    let tag_table = word(mem, TAG_INSTANCES, 0).ok_or(R::Unread)?;
    if word(mem, tag_table, 16 * (key.shader & 0xffff)) != Some(u32::from_be_bytes(*b"shad")) {
        return Err(R::Unread);
    }
    let shader = word(mem, tag_table, 16 * (key.shader & 0xffff) + 8).ok_or(R::Unread)?;
    let flagged = half(mem, shader, SHADER_FLAGS).ok_or(R::Unread)? & 1 != 0;
    let (mut uploads, mut copied) = (false, true);
    if flagged {
        uploads = flagged_shader(mem)?;
        // Whether `0x0003bea0` returns at once for a program with flag bit
        // 20: the frame's structure target copy is made (a byte).
        copied = word(mem, BSP_TARGET_READY, 0).ok_or(R::Unread)? & 0xff != 0;
        let record_key = word(mem, record, 0xc).ok_or(R::Unread)?;
        if layer == LIGHTMAP_LAYER as u8 && ((record_key >> 18) & NO_STRUCTURE_LIGHTMAP) != NO_STRUCTURE_LIGHTMAP {
            return Err(R::ShaderFlag);
        }
    }
    let flags = word(mem, record, 0).ok_or(R::Unread)?;
    let mode = word(mem, record, objects::RECORD_MODEL).ok_or(R::Unread)?;
    let model = word(mem, tag_table, 16 * (mode & 0xffff) + 8).ok_or(R::Unread)?;
    if u32::from(key.section) >= word(mem, model, SECTION_COUNT).ok_or(R::Unread)? {
        return Err(R::Unread);
    }
    let sections = word(mem, model, MODEL_SECTIONS).ok_or(R::Unread)?;
    let section = sections.checked_add(SECTION_STRIDE * u32::from(key.section)).ok_or(R::Unread)?;
    let geometry = word(mem, section, SECTION_GEOMETRY).ok_or(R::Unread)?;
    if u32::from(key.part) >= word(mem, geometry, SECTION_PART_COUNT).ok_or(R::Unread)? {
        return Err(R::Unread);
    }
    let part = word(mem, geometry, SECTION_PARTS)
        .and_then(|p| p.checked_add(PART_SIZE * u32::from(key.part)))
        .ok_or(R::Unread)?;
    let triangle_list = half(mem, part, PART_FLAGS).ok_or(R::Unread)? & PART_TRIANGLE_LIST != 0;
    let mut conditional = None;
    if flags & RECORD_SUBPARTS != 0 {
        let mask = word(mem, record, RECORD_SUBPART_MASK).ok_or(R::Unread)?;
        if mask != 0 && word(mem, geometry, GEOMETRY_SUBPART_COUNT).ok_or(R::Unread)? != 0 {
            conditional = Some(Conditional {
                mask,
                first: half(mem, part, PART_FIRST_SUBPART).ok_or(R::Unread)? as i16,
                count: half(mem, part, PART_SUBPART_COUNT).ok_or(R::Unread)? as i16,
            });
        }
    }
    let named = |why: Refusal| match why {
        Refusal::AuxState => R::AuxState,
        Refusal::Subparts => R::Subparts,
        Refusal::ShaderFlag => R::ShaderFlag,
        Refusal::Argument | Refusal::Program | Refusal::Other => R::Unread,
    };
    let mut out = record_flags(flags, triangle_list, conditional.is_some());
    if fade {
        out |= PartFlags::FADE;
    }
    if flagged {
        out |= PartFlags::SHADER_FLAG;
    }
    if uploads {
        out |= PartFlags::STRUCTURE_PARAMETERS;
    }
    if !copied {
        out |= PartFlags::TARGET_UNCOPIED;
    }
    if !matches!(layer, 1 | 3) {
        out = PartFlags(out.0 & !PartFlags::STENCIL_MARK.0);
    }
    if out.contains(PartFlags::AUX_DEPTH) && crate::layout::aux_depth::read(mem).is_none() {
        return Err(R::AuxState);
    }
    let subparts = match conditional {
        Some(c) if out.contains(PartFlags::SUBPARTS) => Some(visible_subparts(mem, c.mask, c.first, c.count).map_err(named)?),
        _ => None,
    };
    Ok((out, subparts))
}

/// Whether the prologue of a shader with flag bit 0 (the module notes)
/// uploads the active structure's parameter block (`+0x214` above 0,
/// [`PartFlags::STRUCTURE_PARAMETERS`]).
fn flagged_shader(mem: &dyn GuestRead) -> Result<bool, frame_scene::LayerObjectRefusal> {
    use frame_scene::LayerObjectRefusal as R;
    let bsp = word(mem, ACTIVE_BSP, 0).ok_or(R::Unread)?;
    Ok((word(mem, bsp, BSP_PARAMETER_COUNT).ok_or(R::Unread)? as i32) > 0)
}

/// A part whose record draws only its visible subparts: the mask's
/// address and the part's first subpart and count.
#[derive(Clone, Copy)]
struct Conditional {
    mask: u32,
    first: i16,
    count: i16,
}

/// The part an item draws (without its flags), its record's `+0` flags,
/// when the record's bit 6, subpart mask and its section's subpart count
/// are all set, where its visible subparts are read from, and the flags
/// its record's fade byte and its shader's flag bit 0 add
/// ([`PartFlags::FADE`], [`PartFlags::SHADER_FLAG`],
/// [`PartFlags::STRUCTURE_PARAMETERS`]); `None` when the item's layout or
/// a check against the guest's choice fails, or the flagged shader's inputs
/// do not read ([`flagged_shader`]; the walk refuses program flag
/// bit 20 itself, and a flagged record with a structure lightmap index).
fn read_part_layout(
    mem: &dyn GuestRead,
    slot: u32,
    item: u32,
    record: u32,
    program: u32,
    packed: u32,
    why: &mut Refusal,
) -> Option<(Part, u32, Option<Conditional>, PartFlags)> {
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
    let lightmapped = ((record_key >> 18) & NO_STRUCTURE_LIGHTMAP) != NO_STRUCTURE_LIGHTMAP;
    if packed as u8 != key.part || half(mem, item, 0x14)? != 0 {
        *why = Refusal::Argument;
        return None;
    }
    if word(mem, program, PROGRAM_FLAGS)? & (1 << 20) != 0 {
        *why = Refusal::Program;
        return None;
    }
    if record_key & 0xe000_0000 != MODEL_RECORD
        || (lightmapped && !matches!(key.implementation, 2 | 3))
        || word(mem, tag_table, 16 * (key.shader & 0xffff))? != u32::from_be_bytes(*b"shad")
        || usize::from(half(mem, slot, ENTRY_IMPLEMENTATION)? as u8) != key.implementation
        || index_count == 0
    {
        return None;
    }
    // The argument's high byte (the projected size) changes nothing the
    // draw sets (the module notes). A flagged shader draws as the scene
    // draws it with no lightmap to leave unbound, and with the structure's
    // parameter block when its prologue uploads one.
    let mut extra = PartFlags::NONE;
    if (half(mem, record, RECORD_DRAW_FLAGS)? as u8) != u8::MAX {
        extra |= PartFlags::FADE;
    }
    if half(mem, shader, SHADER_FLAGS)? & 1 != 0 {
        if lightmapped {
            *why = Refusal::ShaderFlag;
            return None;
        }
        extra |= PartFlags::SHADER_FLAG;
        if flagged_shader(mem).ok()? {
            extra |= PartFlags::STRUCTURE_PARAMETERS;
        }
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
    Some((read, flags, conditional, extra))
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

    /// The layer-3 walk takes a part with a fade byte, a projected size
    /// or a flagged shader with the flags they add (the structure's
    /// parameter block included), and refuses a flagged shader whose draw
    /// leaves a structure lightmap unbound.
    #[test]
    fn the_walk_takes_a_fade_a_projected_size_and_a_flagged_shader_it_can_draw() {
        let (mut mem, _) = sorted_part();
        let at = 0x1000 + HANDLER_COUNTER + 4;
        let (item, record, shader) = (0x3000, RENDER_RECORDS + RENDER_RECORD_SIZE, 0xa000);
        let flags_of = |mem: &Mem| match read(mem, 0x2000, 0x1000, at).unwrap().entries.as_slice() {
            [Entry::Object { part, .. }] => part.as_ref().map(|p| p.flags),
            other => panic!("{other:?}"),
        };
        mem.w(record + 0x18, 0x40 << 16);
        assert_eq!(flags_of(&mem), Some(PartFlags::FADE));
        mem.w(record + 0x18, 0xff << 16);
        mem.w(item + 0xc, 0x20 << 24 | 1 << 8 | 2);
        assert_eq!(flags_of(&mem), Some(PartFlags::NONE), "the projected size");
        mem.w(item + 0xc, 1 << 8 | 2);
        let bsp = 0xd000;
        mem.w(0x004e_0348, bsp);
        mem.w(bsp + 0x214, 0);
        mem.w(shader + 0x14, 1 << 16);
        // The fixture's record names structure lightmap 0, which the
        // flagged draw leaves unbound.
        assert_eq!(flags_of(&mem), None, "a lightmap the flagged draw leaves unbound");
        mem.w(record + 0xc, MODEL_RECORD | NO_STRUCTURE_LIGHTMAP << 18 | 9 << 9 | 2);
        // The walk refuses program bit 20 itself, so the target copy is
        // not asked for.
        assert_eq!(flags_of(&mem), Some(PartFlags::SHADER_FLAG));
        mem.w(bsp + 0x214, 1);
        assert_eq!(
            flags_of(&mem),
            Some(PartFlags::SHADER_FLAG | PartFlags::STRUCTURE_PARAMETERS),
            "the prologue uploads the structure's parameter block"
        );
    }

    /// An object item of a world layer other than 3 carries its record's
    /// flags (the stencil mark only in layers 1 and 3), or names what the
    /// draw does for it that the scene does not carry.
    #[test]
    fn a_world_layers_object_item_carries_its_records_flags_or_names_why_not() {
        use frame_scene::LayerObjectRefusal as R;
        let (mut mem, _) = sorted_part();
        let (item, record, parts, shader) = (0x3000, RENDER_RECORDS + RENDER_RECORD_SIZE, 0x9000, 0xa000);
        let key = part().key;
        let at = |mem: &Mem, layer| layer_part(mem, layer, item, record, key);
        for layer in [1, 5, 7, 8, 20] {
            assert_eq!(at(&mem, layer), Ok((PartFlags::NONE, None)), "layer {layer}");
        }
        mem.w(record, 3 << 29 | RECORD_STENCIL_MARK);
        assert_eq!(at(&mem, 1), Ok((PartFlags::STENCIL_MARK, None)));
        assert_eq!(at(&mem, 8), Ok((PartFlags::NONE, None)), "no mark outside layers 1 and 3");
        mem.w(record, 3 << 29);
        // A fade byte below 0xff is the part's flag: the host decides
        // from the device's stipple.
        mem.w(record + 0x18, 0x80 << 16);
        for layer in [1, 3, 5, 7, 8, 20] {
            assert_eq!(at(&mem, layer), Ok((PartFlags::FADE, None)), "layer {layer}");
        }
        mem.w(record + 0x18, 0xff << 16);
        // The shader's flag bit 0: drawn with the structure's parameter
        // block where the prologue uploads it.
        let (bsp, target_ready) = (0xd000, 0x0050_9430);
        mem.w(0x004e_0348, bsp);
        mem.w(bsp + 0x214, 0);
        mem.w(target_ready, 1);
        mem.w(shader + 0x14, 1 << 16);
        assert_eq!(at(&mem, 7), Ok((PartFlags::SHADER_FLAG, None)));
        mem.w(bsp + 0x214, 2);
        assert_eq!(
            at(&mem, 7),
            Ok((PartFlags::SHADER_FLAG | PartFlags::STRUCTURE_PARAMETERS, None)),
            "the prologue uploads the parameter block"
        );
        // Before the frame's target copy the part says so: the host keeps
        // an item whose program would make the copy.
        mem.w(target_ready, 0);
        assert_eq!(
            at(&mem, 7),
            Ok((PartFlags::SHADER_FLAG | PartFlags::STRUCTURE_PARAMETERS | PartFlags::TARGET_UNCOPIED, None)),
            "the draw may copy the structure's target"
        );
        mem.w(bsp + 0x214, 0);
        assert_eq!(at(&mem, 7), Ok((PartFlags::SHADER_FLAG | PartFlags::TARGET_UNCOPIED, None)), "without a block too");
        mem.w(target_ready, 0x100);
        assert_eq!(at(&mem, 7), Ok((PartFlags::SHADER_FLAG | PartFlags::TARGET_UNCOPIED, None)), "the flag is a byte");
        mem.w(target_ready, 1);
        mem.w(record + 0xc, MODEL_RECORD | 5 << 18 | 9 << 9 | 2);
        assert_eq!(at(&mem, 3), Err(R::ShaderFlag), "layer 3 leaves the lightmap unbound");
        assert_eq!(at(&mem, 7), Ok((PartFlags::SHADER_FLAG, None)), "no lightmap outside layer 3");
        mem.w(record + 0xc, MODEL_RECORD | 9 << 9 | 2);
        mem.w(shader + 0x14, 0);
        mem.w(target_ready, 0);
        // The argument's high byte (the projected size) changes nothing
        // the draw sets. The part, pass and level the item names are the
        // scene item's to draw, not a refusal.
        mem.w(item + 0xc, 0x80 << 24 | 1 << 8 | 2);
        assert_eq!(at(&mem, 7), Ok((PartFlags::NONE, None)));
        mem.w(item + 0xc, 1 << 8 | 3);
        assert_eq!(at(&mem, 7), Ok((PartFlags::NONE, None)), "the key's part, which extraction takes from the low byte");
        mem.w(item + 0xc, 1 << 8 | 2);
        mem.w(item + 0x14, 1);
        assert_eq!(at(&mem, 7), Ok((PartFlags::NONE, None)), "another pass");
        mem.w(item + 0x14, 1 << 8);
        assert_eq!(at(&mem, 7), Ok((PartFlags::NONE, None)), "another level");
        mem.w(item + 0x14, 0);
        // A part that is not a triangle list draws as a strip only with
        // bit 9.
        mem.w(parts + 2 * PART_SIZE, 0);
        assert_eq!(at(&mem, 7), Ok((PartFlags::NONE, None)), "its indices as a list");
        mem.w(record, 3 << 29 | RECORD_STRIP);
        assert_eq!(at(&mem, 7), Ok((PartFlags::STRIP, None)));
        mem.w(record + objects::RECORD_MODEL, 0);
        assert_eq!(at(&mem, 7), Err(R::Unread), "a model that does not read");
    }

    /// Each refusal of a walked part has its own count, so what keeps the
    /// guest's layer-3 draws is read from `walk_part_*` and not guessed.
    #[test]
    fn every_refused_part_is_counted_by_its_cause() {
        let (mut mem, handle) = sorted_part();
        let at = 0x1000 + HANDLER_COUNTER + 4;
        let (item, shader, program) = (0x3000, 0xa000, 0x4000);
        let refused = |mem: &Mem| {
            let sorted = read(mem, 0x2000, 0x1000, at).unwrap();
            assert_eq!(sorted.entries, [Entry::Object { handle, part: None }]);
            sorted.causes
        };
        mem.w(item + 0xc, 1 << 8 | 3);
        assert_eq!(refused(&mem), Causes { argument: 1, ..Causes::default() }, "another part");
        mem.w(item + 0xc, 1 << 8 | 2);
        mem.w(item + 0x14, 1 << 8);
        assert_eq!(refused(&mem), Causes { argument: 1, ..Causes::default() }, "another level");
        mem.w(item + 0x14, 0);
        mem.w(program + PROGRAM_FLAGS, 1 << 20);
        assert_eq!(refused(&mem), Causes { program: 1, ..Causes::default() }, "flag bit 20");
        mem.w(program + PROGRAM_FLAGS, 0);
        mem.w(0x004e_0348, 0xd000);
        mem.w(0xd000 + 0x214, 0);
        mem.w(shader + 0x14, 1 << 16);
        assert_eq!(
            refused(&mem),
            Causes { shader_flag: 1, ..Causes::default() },
            "a lightmap the flagged draw leaves unbound"
        );
        mem.w(shader + 0x14, 0);
        mem.w(at + ENTRY_CLASSIFICATION, 0);
        assert_eq!(refused(&mem), Causes { layout: 1, ..Causes::default() }, "a class the guest did not pick");
        mem.w(at + ENTRY_CLASSIFICATION, 4);
        assert_eq!(read(&mem, 0x2000, 0x1000, at).unwrap().causes, Causes::default());
    }

    #[test]
    fn a_records_flags_become_part_flags() {
        // Bit 14 marks; bit 9 makes a non-list part a strip.
        assert_eq!(record_flags(0, true, false), PartFlags::NONE);
        assert_eq!(record_flags(RECORD_STENCIL_MARK, true, false), PartFlags::STENCIL_MARK);
        assert_eq!(record_flags(RECORD_STRIP, false, false), PartFlags::STRIP);
        assert_eq!(record_flags(RECORD_STRIP, true, false), PartFlags::NONE, "a list part ignores bit 9");
        assert_eq!(
            record_flags(RECORD_STRIP | RECORD_STENCIL_MARK, false, false),
            PartFlags::STRIP | PartFlags::STENCIL_MARK
        );
        // Without bit 9 the guest draws a strip part's indices as a list.
        assert_eq!(record_flags(0, false, false), PartFlags::NONE);
        // Bit 6 chooses subparts only when there is a mask and subparts to
        // choose.
        assert_eq!(record_flags(RECORD_SUBPARTS, true, false), PartFlags::NONE);
        assert_eq!(record_flags(RECORD_SUBPARTS, true, true), PartFlags::SUBPARTS);
        assert_eq!(record_flags(RECORD_SUBPARTS | RECORD_STRIP, false, true), PartFlags::SUBPARTS | PartFlags::STRIP);
        assert_eq!(record_flags(RECORD_SUBPARTS, false, true), PartFlags::SUBPARTS);
        // Bit 11 draws with the view's auxiliary depth.
        assert_eq!(record_flags(RECORD_AUX_STATE, true, false), PartFlags::AUX_DEPTH);
        assert_eq!(record_flags(RECORD_AUX_STATE | RECORD_SUBPARTS, true, true), PartFlags::AUX_DEPTH | PartFlags::SUBPARTS);
        assert_eq!(record_flags(RECORD_AUX_STATE, false, false), PartFlags::AUX_DEPTH);
    }

    #[test]
    fn a_mask_is_read_over_the_parts_own_subparts() {
        let mut mem = Mem::default();
        mem.w(0x100, 0b1010_0000);
        mem.w(0x104, 1);
        let got = visible_subparts(&mem, 0x100, 5, 4).unwrap();
        assert_eq!(got, Subparts { first: 5, count: 4, visible: vec![0b0101] });
        // Subpart 32 is bit 0 of the second word.
        assert_eq!(visible_subparts(&mem, 0x100, 31, 2).unwrap().visible, [0b10]);
        assert_eq!(visible_subparts(&mem, 0x100, 0, -1).unwrap().count, 0, "a count below one draws none");
        assert_eq!(visible_subparts(&mem, 0x100, -1, 2), Err(Refusal::Subparts));
        assert_eq!(visible_subparts(&mem, 0x100, 60, 8), Err(Refusal::Subparts), "the third word does not read");
    }

    /// A part of 100 subparts, past the 64 one word of the scene's mask
    /// holds: every one the mask chooses, read over four of its words.
    #[test]
    fn a_mask_over_a_hundred_subparts_is_read_whole() {
        let mut mem = Mem::default();
        // Subparts 10..110 of the section; the mask's words 0 to 3.
        mem.w(0x100, 1 << 10 | 1 << 31);
        mem.w(0x104, 1);
        mem.w(0x108, 0);
        mem.w(0x10c, 1 << 13 | 1 << 2);
        let got = visible_subparts(&mem, 0x100, 10, 100).unwrap();
        assert_eq!((got.first, got.count, got.visible.len()), (10, 100, 2));
        // Section subparts 10, 31, 32, 98 and 109: the part's 0, 21, 22, 88, 99.
        assert_eq!(got.drawn().collect::<Vec<_>>(), [0, 21, 22, 88, 99]);
        assert_eq!(got, Subparts::of(10, 100, [0, 21, 22, 88, 99]));
        mem.0.remove(&0x10c);
        assert_eq!(visible_subparts(&mem, 0x100, 10, 100), Err(Refusal::Subparts), "the fourth word does not read");
    }

    /// Every word the auxiliary state toggle reads, made up.
    fn aux_globals(mem: &mut Mem) {
        use crate::layout::aux_depth::{AUX_SCALE, CLIP_FAR, CLIP_NEAR, ONE, SCALE_DENOMINATOR, SCALE_NUMERATOR, VIEW};
        for at in [AUX_SCALE, ONE, CLIP_NEAR, CLIP_FAR, SCALE_NUMERATOR, SCALE_DENOMINATOR] {
            mem.f(at, 1.0);
        }
        for k in 0..12 {
            mem.f(VIEW + 4 + 4 * k, 0.0);
        }
        for k in 0..16 {
            mem.f(VIEW + 0x78 + 4 * k, 0.0);
        }
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
        // The same part without bit 9: in the scene unflagged, as the guest
        // draws its indices as a list.
        with_record_bits(&mut mem, 0);
        let sorted = walk(&mem);
        let listed = Part { triangle_list: false, ..part() };
        assert_eq!(sorted.entries, [Entry::Object { handle, part: Some(listed) }]);
        assert_eq!(sorted.causes, Causes::default());
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
            subparts: Some(Subparts { first: 30, count: 5, visible: vec![0b0_1001] }),
            ..part()
        };
        assert_eq!(sorted.entries, [Entry::Object { handle, part: Some(chosen.clone()) }]);
        assert_eq!(sorted.causes, Causes::default());
        assert_eq!(chosen.subparts.as_ref().unwrap().drawn().collect::<Vec<_>>(), [0, 3]);
        // A mask word that does not read: refused by name.
        mem.0.remove(&0xd004);
        assert_eq!(walk(&mem).causes, Causes { subparts: 1, ..Causes::default() });
        // More subparts than one word of the scene's mask holds: in the
        // scene, every one chosen (30, 33 and the section's 96).
        mem.w(0xd004, 1 << 1);
        mem.w(0xd008, 0);
        mem.w(0xd00c, 1);
        mem.w(0xd010, 0);
        mem.w(at_part + PART_SUBPART_COUNT, 100);
        let many = Part { subparts: Some(Subparts::of(30, 100, [0, 3, 66])), ..chosen.clone() };
        let sorted = walk(&mem);
        assert_eq!(sorted.entries, [Entry::Object { handle, part: Some(many) }]);
        assert_eq!(sorted.causes, Causes::default());
        // No subparts: drawn, as the guest draws it, with none.
        mem.w(at_part + PART_SUBPART_COUNT, 0);
        let none = Part { subparts: Some(Subparts { first: 30, count: 0, visible: Vec::new() }), ..chosen };
        assert_eq!(walk(&mem).entries, [Entry::Object { handle, part: Some(none) }]);
        mem.w(0x8000 + GEOMETRY_SUBPART_COUNT, 0);
        assert_eq!(walk(&mem).entries, [Entry::Object { handle, part: Some(part()) }], "no subparts to choose");
        mem.w(at_part + 8, 18);

        // The auxiliary state toggle: in the scene when what it reads
        // reads, refused by name when not.
        with_record_bits(&mut mem, RECORD_AUX_STATE);
        let sorted = walk(&mem);
        assert_eq!(sorted.entries, [Entry::Object { handle, part: None }]);
        assert_eq!(sorted.causes, Causes { aux_state: 1, ..Causes::default() });
        aux_globals(&mut mem);
        let aux = Part { flags: PartFlags::AUX_DEPTH, ..part() };
        let sorted = walk(&mem);
        assert_eq!(sorted.entries, [Entry::Object { handle, part: Some(aux) }]);
        assert_eq!(sorted.causes, Causes::default());
    }
}
