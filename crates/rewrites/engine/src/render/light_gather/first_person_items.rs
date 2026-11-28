//! The first-person draw's items: what `0x0003fdf0` builds before its
//! layer draws, for its arms and weapon (docs/graphics/native-objects.md,
//! "First person").
//!
//! Each call of `0x0003fdf0` (argument 0 and argument 1) rebuilds them,
//! with the same steps:
//!
//! 1. The prologue (`0x0003fdf0`..`0x0003fe3d`): the sorted list's mark
//!    `[0x004b6284]` takes its count `[0x004b6280]`; scene list 0
//!    (`0x004c1a48`) saves its layers at `+0x10`, clears them, and starts
//!    its first item at its count.
//! 2. The first-person effects `0x00174220` (argument 1),
//!    [`super::first_person_effects`]: they can append sorted entries, so
//!    [`records`] reads the sorted list after them.
//! 3. The record loop (`0x0003fe60`..`0x0003ff93`): each record index in
//!    `0x004c8740` (`[0x0050943c]` of them) that passes the filters
//!    ([`super::items::selected`], with the mode's filter) has its
//!    parts made by `0x000449e0` (the record's geometry), `0x00045ce0`
//!    (the part's material and centre into a scratch), `0x000460d0` (the
//!    part's subparts against the record's mask) and `0x00041040` (the
//!    part's items).
//!
//! `0x00041040` in kind 0 appends one item per part to list 0 (the
//! chooser `0x00040de0` answers list 0 for kind 0; the second-list append
//! at `0x000412e9` is only for a light's kind), and a part whose layers
//! hold bit 15 also takes an entry of the sorted list (`0x0001e2d0`,
//! filled at `0x00041390`..`0x00041474`).
//!
//! [`make`] reads only and returns the guest's writes ([`Writes`]) in the
//! guest's order, with what it appended ([`Made`]). The first person's
//! records are objects' records (`+4` the render model): a structure
//! record, a shader reached through a virtual call, a level table with no
//! levels or a record with no node matrices is a [`Refusal`], and the
//! caller leaves the call to the guest.

use super::guest::{NONE, TAGS};
use super::items::{ALPHA_SCALE, List, Lod, PART, SCREEN_SCALE, part_visible, record_at, selected};
use super::{Refusal, Result, Writes};
use crate::light_constants::constant::ONE;
use guest_layout::memory::GuestMemory;

/// The record indices the first person draws (`u16`, `0xffff` none).
pub const RECORDS: u32 = 0x004c_8740;
/// How many of [`RECORDS`].
pub const RECORD_COUNT: u32 = 0x0050_943c;
/// Scene list 0's header, which the first person's items join.
pub const LIST: u32 = super::items::LISTS;
/// List 0's layers saved by the prologue, restored by the epilogue.
pub const SAVED_LAYERS: u32 = LIST + 0x10;
/// The sorted list: its count, the mark the first person takes, the
/// pointer to its entries, each [`SORTED_ENTRY`] bytes, at most
/// [`SORTED_CAPACITY`].
pub const SORTED_COUNT: u32 = 0x004b_6280;
pub const SORTED_MARK: u32 = 0x004b_6284;
pub const SORTED_ENTRIES: u32 = 0x004b_6288;
pub const SORTED_ENTRY: u32 = 0x70;
pub const SORTED_CAPACITY: u32 = 0x400;
/// A sorted geometry entry's draw (`0x00041020`).
pub const SORTED_DRAW: u32 = 0x0004_1020;
/// The view's position and forward vector the sorted depth is taken
/// along (`0x00041390`).
pub const VIEW_POSITION: u32 = 0x0048_5618;
pub const VIEW_FORWARD: u32 = 0x0048_5624;
/// The node matrices a record's `+8` offsets into (`0x0001d6b0`).
pub const NODE_MATRICES: u32 = 0x0048_7b1c;
/// The scale of a record's depth bias (`0x00045efc`).
pub const BIAS_SCALE: u32 = 0x0045_dc70;

const PIXEL_SIZES: u32 = 0x004c_0b84;
const ATTACHMENT_BYTES: u32 = 0x004c_0ed6;
const OVERRIDES: u32 = 0x004c_1228;

/// The record filter of mode `eax` (`0x0003fdfe`..`0x0003fe21`).
pub fn filter(mode: u32) -> u32 {
    match mode {
        0 => u32::MAX,
        2 => 0x10,
        _ => 0x20,
    }
}

/// What `0x00045ce0` leaves in the scratch for an object's part.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scratch {
    /// `+0`: the material's shader.
    pub shader: u32,
    /// `+4`: the part's centre (its node's translation).
    pub centre: [f32; 3],
    /// `+0x10`: the record's depth bias.
    pub bias: f32,
    /// `+0x28`: the part's size, or 1.
    pub size: f32,
}

/// One part's appends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Made {
    /// The record index and the part.
    pub record: u16,
    pub part: u32,
    /// The list-0 item's index, when appended (not when the list is full
    /// or the part has no layer).
    pub item: Option<u32>,
    /// The sorted entry's index, for a part with layer 15.
    pub sorted: Option<u32>,
    pub shader: u32,
    pub layers: u32,
}

/// What one call builds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Items {
    pub writes: Writes,
    pub made: Vec<Made>,
    /// List 0's layers after the loop: the layers the call can draw.
    pub layers: u32,
}

/// A tag's data, read as the guest reads it (`[TAGS] + 16 index + 8`).
fn tag_data(m: &dyn GuestMemory, handle: u32) -> Result<u32> {
    let table = m.u32(TAGS)?;
    Ok(m.u32(table.wrapping_add((handle & 0xffff) * 16 + 8))?)
}

/// `0x000449e0` (DL 0) for an object's record: its render model and the
/// geometry of the section the record names, `0` for none.
fn geometry(m: &dyn GuestMemory, record: u32) -> Result<(u32, u32)> {
    let model = m.u32(record + 4)?;
    if model == NONE {
        return Err(Refusal::Unsupported);
    }
    let definition = tag_data(m, model)?;
    let section = ((m.u32(record + 0xc)? >> 9) & 0x1ff) * 0x5c;
    let section = m.u32(definition + 0x28)?.wrapping_add(section);
    Ok((definition, m.u32(section.wrapping_add(0x34))?))
}

/// `0x00045ce0` for part `j` of an object's record: the scratch, or
/// `None` when the part is not drawn (section `0xff`).
fn material(m: &dyn GuestMemory, record: u32, definition: u32, part: u32, kind: u32) -> Result<Option<Scratch>> {
    let size = m.f32(part + 0x2c)?;
    // `comiss`, `jbe`: zero, negative or NaN take the image's 1.
    let size = if size > 0.0 { size } else { m.f32(ONE)? };
    let number = m.u16(part + 4)? as i16 as i32 as u32;
    let material = m.u32(definition + 0x64)?.wrapping_add(number.wrapping_mul(32));
    let shader = m.u32(material.wrapping_add(0xc))?;
    let section = (m.u32(record + 0xc)? >> 9) & 0x1ff;
    if section == 0xff {
        return Ok(None);
    }
    let section = m.u32(definition + 0x28)?.wrapping_add(section * 0x5c);
    // `0x0001d6b0`: none for -1 or a negative offset, and the guest then
    // reads near zero.
    let offset = m.u32(record + 8)?;
    if offset as i32 <= -1 {
        return Err(Refusal::Unsupported);
    }
    let nodes = NODE_MATRICES.wrapping_add(offset & 0x0fff_ffff);
    let node = m.u16(section.wrapping_add(0x2c))?;
    let matrix = if node == 0xffff {
        nodes.wrapping_add(0x44)
    } else {
        let node = if m.u8(definition + 4)? & 4 != 0 {
            let remap = nodes.wrapping_add(4).wrapping_add((kind as u16 as i16 as i32 as u32).wrapping_mul(4));
            m.u16(remap)? as i16 as i32 as u32
        } else {
            node as i16 as i32 as u32
        };
        nodes.wrapping_add(node.wrapping_mul(0x30)).wrapping_add(0x44)
    };
    let centre = [
        m.f32(matrix.wrapping_add(0xc))?,
        m.f32(matrix.wrapping_add(0x1c))?,
        m.f32(matrix.wrapping_add(0x2c))?,
    ];
    let packed = m.u8(record + 0x1b)? & 0xf;
    let bias = if packed != 0 {
        (i32::from(packed) - 5) as f32 * m.f32(BIAS_SCALE)?
    } else {
        0.0
    };
    Ok(Some(Scratch { shader, centre, bias, size }))
}

/// The sorted entry's class, `0x00040e10` on the shader's flags `+0x16`
/// (DL bit 0, CL bit 1, `0x000413cd`..`0x000413d8`).
fn sorted_class(flags: u8) -> u32 {
    match (flags & 1 != 0, flags & 2 != 0) {
        (false, false) => 3,
        (false, true) => 2,
        (true, false) => 1,
        (true, true) => 0,
    }
}

/// The prologue's writes (`0x0003fdf0`..`0x0003fe3d`): the sorted list's
/// mark takes its count, list 0 saves its layers, clears them and starts
/// its first item at its count.
pub fn prologue(m: &dyn GuestMemory) -> Result<Writes> {
    let mut out = Writes::new();
    out.word(SORTED_MARK, m.u32(SORTED_COUNT)?);
    let list = List::read(m, 0)?;
    out.word(SAVED_LAYERS, list.layers);
    out.word(LIST + 8, 0);
    out.word(LIST + 0xc, list.count as u32);
    Ok(out)
}

/// The writes and appends of one call of `0x0003fdf0` with EAX `mode`,
/// up to its layer draws, where the first-person effects `0x00174220`
/// change nothing: the prologue ([`prologue`]) and the record loop.
pub fn make(m: &dyn GuestMemory, mode: u32) -> Result<Items> {
    let writes = prologue(m)?;
    let mut list = List::read(m, 0)?;
    list.layers = 0;
    record_loop(m, mode, writes, list, m.u32(SORTED_COUNT)?)
}

/// The record loop's writes and appends (`0x0003fe48`..`0x0003ff93`),
/// after the prologue's writes and the first-person effects
/// (`super::first_person_effects`) are in memory: list 0 and the sorted
/// list as they left them.
pub fn records(m: &dyn GuestMemory, mode: u32) -> Result<Items> {
    record_loop(m, mode, Writes::new(), List::read(m, 0)?, m.u32(SORTED_COUNT)?)
}

fn record_loop(m: &dyn GuestMemory, mode: u32, writes: Writes, mut list: List, sorted_count: u32) -> Result<Items> {
    let mut out = Items { writes, ..Items::default() };
    let filter = filter(mode);
    let mut sorted = sorted_count;
    let sorted_at = m.u32(SORTED_ENTRIES)?;
    let (screen, alpha) = (m.f32(SCREEN_SCALE)?, m.f32(ALPHA_SCALE)?);
    let view = [
        m.f32(VIEW_POSITION)?,
        m.f32(VIEW_POSITION + 4)?,
        m.f32(VIEW_POSITION + 8)?,
    ];
    let forward = [
        m.f32(VIEW_FORWARD)?,
        m.f32(VIEW_FORWARD + 4)?,
        m.f32(VIEW_FORWARD + 8)?,
    ];

    let count = m.i32(RECORD_COUNT)?;
    for n in 0..count.max(0) as u32 {
        let index = m.u16(RECORDS.wrapping_add(2 * n))?;
        if index == 0xffff {
            continue;
        }
        if index >= 0x8000 {
            return Err(Refusal::Unsupported);
        }
        let i = u32::from(index);
        let record = record_at(i);
        if !selected(m, record, filter)? {
            continue;
        }
        let kind = m.u32(record + 0xc)? & 0xf;
        let pixel = u32::from(m.u8(PIXEL_SIZES.wrapping_add(i))?);
        let shader_override = match m.u8(ATTACHMENT_BYTES.wrapping_add(i))? {
            0xff => NONE,
            s => m.u32(OVERRIDES + 4 * u32::from(s))?,
        };
        let (definition, geometry) = geometry(m, record)?;
        if geometry == 0 {
            continue;
        }
        let parts = m.i32(geometry)?;
        if parts > 0x1_0000 {
            return Err(Refusal::Unsupported);
        }
        let base = m.u32(geometry.wrapping_add(4))?;
        for j in 0..parts.max(0) as u32 {
            let part = base.wrapping_add(j * PART);
            let Some(scratch) = material(m, record, definition, part, kind)? else {
                continue;
            };
            if m.u32(record)? & 0x40 != 0 && !part_visible(m, part, m.u32(record + 0x14)?)? {
                continue;
            }
            let shader = match shader_override {
                NONE => scratch.shader,
                s => s,
            };
            if shader == NONE {
                continue;
            }
            // 0x00041040, kind 0, the fifth argument 0, masks -1 and 0.
            if matches!(m.u16(part)?, 0 | 5) || scratch.shader == NONE {
                continue;
            }
            let pixels = ((pixel as f32 * scratch.size) * screen) * alpha;
            let lod = Lod::read(m, shader)?;
            let level = lod.level(m, pixels)?;
            let layers = lod.layers(m, level)?;
            // The material's own shader at that level, and the shader's
            // first level (`0x000411c6`..`0x0004129d`).
            let own = Lod::read(m, scratch.shader)?;
            let own_layers = m.u32(own.levels.wrapping_add(level * 10 + 2))?;
            if own_layers & 0x8080 != 0 && own_layers & 0xa == 0 && lod.layers(m, 0)? & (1 << 21) != 0 {
                continue;
            }
            let argument = ((pixel << 16 | i) << 8) | j;
            let mut made = Made {
                record: index,
                part: j,
                item: None,
                sorted: None,
                shader,
                layers,
            };
            if layers & 0xffff_7fff != 0 && list.count < list.capacity {
                made.item = Some(list.count as u32);
                // 0x0003b9d0 with the and-mask -1 and the or-mask 0.
                list.append(&mut out.writes, shader, level, layers, argument);
            }
            if layers & 0x8000 != 0 && sorted < SORTED_CAPACITY {
                // 0x0001e2d0, then the fill.
                let entry = sorted_at.wrapping_add(sorted * SORTED_ENTRY);
                made.sorted = Some(sorted);
                sorted += 1;
                out.writes.word(SORTED_COUNT, sorted);
                let flags = m.u8(tag_data(m, shader)?.wrapping_add(0x16))?;
                let d = [
                    scratch.centre[0] - view[0],
                    scratch.centre[1] - view[1],
                    scratch.centre[2] - view[2],
                ];
                let along = d[2] * forward[2] + d[1] * forward[1] + d[0] * forward[0];
                let depth = (0.0 - along) - scratch.bias;
                out.writes.word(entry + 0x24, shader);
                out.writes.word(entry + 0x28, argument);
                out.writes.word(entry, sorted_class(flags));
                out.writes.word(entry + 8, 0);
                out.writes.word(entry + 0x20, SORTED_DRAW);
                out.writes.word(entry + 4, depth.to_bits());
                for (k, c) in scratch.centre.iter().enumerate() {
                    out.writes.word(entry + 0x64 + 4 * k as u32, c.to_bits());
                }
                out.writes.byte(entry + 0xc, 0);
            }
            out.made.push(made);
        }
    }
    out.layers = list.layers;
    Ok(out)
}

/// The epilogue (`0x0004028e`..`0x0004029c`): list 0's layers back, its
/// first item 0.
pub fn restore(m: &dyn GuestMemory) -> Result<Writes> {
    let mut out = Writes::new();
    out.word(LIST + 8, m.u32(SAVED_LAYERS)?);
    out.word(LIST + 0xc, 0);
    Ok(out)
}

/// The layers call `call` (the stack argument) draws, in its order, and
/// the gate byte each needs set (`0x0003ffa0`..`0x00040284`; layer 2 by
/// `0x00044370` with EDI 2). Each draws only when list 0's layers hold it.
pub const fn layers(call: u32) -> &'static [(u8, Option<u32>)] {
    if call & 0xff != 0 {
        &[(16, None)]
    } else {
        &[
            (1, None),
            (3, None),
            (8, Some(0x004b_a022)),
            (20, None),
            (7, None),
            (2, Some(0x004b_a023)),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mode_filters_are_the_guests() {
        assert_eq!(filter(0), u32::MAX);
        assert_eq!(filter(2), 0x10);
        assert_eq!(filter(1), 0x20);
        assert_eq!(filter(3), 0x20);
    }

    #[test]
    fn the_sorted_class_is_0x00040e10s() {
        assert_eq!(sorted_class(0), 3);
        assert_eq!(sorted_class(2), 2);
        assert_eq!(sorted_class(1), 1);
        assert_eq!(sorted_class(3), 0);
    }

    #[test]
    fn the_second_call_draws_layer_16_only() {
        assert_eq!(layers(1), &[(16, None)]);
        let first: Vec<u8> = layers(0).iter().map(|l| l.0).collect();
        assert_eq!(first, [1, 3, 8, 20, 7, 2]);
    }
}
