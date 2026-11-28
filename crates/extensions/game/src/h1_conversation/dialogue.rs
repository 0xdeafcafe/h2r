//! Speech routes for rebuilt Halo 1 units. H2 0x114ec0 caches the chosen
//! dialogue datum in the per-unit block located by the word at +0x342.

use guest_layout::view::Tags;
use map::{H1_DIALOGUE_SLOT, H1_NATIVE_DIALOGUE_SLOT, H1_VOICE_VARIANT};

use super::{cast, leaders::guest_fault, voices, *};

const SPEECH_OFFSET: u32 = 0x342;
const UDLG: GroupTag = GroupTag::from_chars(*b"udlg");
const SND: GroupTag = GroupTag::from_chars(*b"snd!");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Line {
    pub(super) speaker: u32,
    pub(super) row: u32,
    pub(super) priority: u32,
}

pub(super) fn tag_address(m: &dyn GuestMemory, id: u32, group: GroupTag) -> Result<Option<u32>, MemFault> {
    let Some(tag) = Tags.entry(m, DatumHandle(id))? else {
        return Ok(None);
    };
    if tag.group(m)? != group {
        return Ok(None);
    }
    Ok(Some(tag.address(m)?).filter(|&address| address != 0))
}

pub(super) fn speech(m: &dyn GuestMemory, unit: u32) -> Result<Option<u32>, MemFault> {
    let Some(body) = object_address(m, unit)? else {
        return Ok(None);
    };
    let offset = read_i16(m, body + SPEECH_OFFSET)?;
    Ok((offset > 0).then(|| body + offset as u32))
}

pub fn active(m: &dyn GuestMemory, unit: u32) -> Result<u32, MemFault> {
    speech(m, unit)?.map_or(Ok(NONE), |address| m.u32(address))
}

pub fn source_sound(m: &dyn GuestMemory, unit: u32, slot: u16) -> Result<u32, MemFault> {
    Ok(source_row(m, unit, slot)?.map_or(NONE, |(_, sound)| sound))
}

pub(super) fn source_offset_row(
    m: &dyn GuestMemory,
    unit: u32,
    offset: usize,
) -> Result<Option<u32>, MemFault> {
    source_row(m, unit, (offset / 0x10 - 1) as u16).map(|row| row.map(|(index, _)| index))
}

pub(super) fn converted_bank(m: &dyn GuestMemory, sound_bank: u32) -> Result<bool, MemFault> {
    let Some(root) = tag_address(m, sound_bank, UDLG)? else {
        return Ok(false);
    };
    let count = m.u32(root + 0xc)?;
    if count == 0 {
        return Ok(false);
    }
    let rows = m.u32(root + 0x10)?;
    Ok(m.u32(rows + (count - 1) * 0x10)? & 0xffff_0000 == H1_DIALOGUE_SLOT)
}

pub(super) fn offset_at_row(
    m: &dyn GuestMemory,
    dialogue: u32,
    index: u16,
) -> Result<Option<usize>, MemFault> {
    let Some(root) = tag_address(m, dialogue, UDLG)? else {
        return Ok(None);
    };
    let index = u32::from(index);
    if index >= m.u32(root + 0xc)? {
        return Ok(None);
    }
    let rows = m.u32(root + 0x10)?;
    let marker = m.u32(rows + index * 0x10)?;
    Ok(matches!(
        marker & 0xffff_0000,
        H1_DIALOGUE_SLOT | H1_NATIVE_DIALOGUE_SLOT
    )
    .then_some((usize::from(marker as u16) + 1) * 0x10))
}

pub(super) fn source_row(
    m: &dyn GuestMemory,
    unit: u32,
    slot: u16,
) -> Result<Option<(u32, u32)>, MemFault> {
    let dialogue = active(m, unit)?;
    let Some(root) = tag_address(m, dialogue, UDLG)? else {
        return Ok(None);
    };
    let count = m.u32(root + 0xc)?;
    if count == 0 || count > 1024 {
        return Ok(None);
    }
    let rows = m.u32(root + 0x10)?;
    let wanted = H1_DIALOGUE_SLOT | u32::from(slot);
    let mut first = 0;
    let mut end = count;
    while first < end {
        let index = first + (end - first) / 2;
        let row = rows + index * 0x10;
        let marker = m.u32(row)?;
        // The native prefix precedes the sorted source slots.
        if marker & 0xffff_0000 != H1_DIALOGUE_SLOT || marker < wanted {
            first = index + 1;
        } else if marker > wanted {
            end = index;
        } else {
            let sound = m.u32(row + 0xc)?;
            return Ok(tag_address(m, sound, SND)?.map(|_| (index, sound)));
        }
    }
    Ok(None)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Variant {
    selector: i16,
    name: u32,
    dialogue: u32,
}

fn candidates(variants: &[Variant], requested: i16) -> Vec<Variant> {
    // H1 0x469ad tries a positive selector, then zero, then -1 (all).
    let selectors = [requested, 0, -1];
    for &selector in &selectors[usize::from(requested <= 0)..] {
        let matching: Vec<_> = variants
            .iter()
            .copied()
            .filter(|v| selector == -1 || v.selector == selector)
            .collect();
        if !matching.is_empty() {
            return matching;
        }
    }
    Vec::new()
}

fn choose(m: &mut dyn GuestMemory, unit: u32, requested: i16) -> Result<Option<Variant>, MemFault> {
    let Some(body) = object_address(m, unit)? else {
        return Ok(None);
    };
    let definition = DatumHandle(m.u32(body)?);
    let Some(tag) = Tags.entry(m, definition)? else {
        return Ok(None);
    };
    let definition = tag.address(m)?;
    if definition == 0 {
        return Ok(None);
    }
    let model = m.u32(definition + 0x38)?;
    let Some(root) = tag_address(m, model, GroupTag::from_chars(*b"hlmt"))? else {
        return Ok(None);
    };
    let count = m.u32(root + 0x50)?;
    let rows = m.u32(root + 0x54)?;
    let mut variants = Vec::new();
    for k in 0..count.min(256) {
        let row = rows + k * 0x38;
        let marker = m.u32(row + 0x24)?;
        if marker & 0xffff_0000 == H1_VOICE_VARIANT {
            variants.push(Variant {
                selector: marker as i16,
                name: m.u32(row)?,
                dialogue: m.u32(row + 0x34)?,
            });
        }
    }
    let options = candidates(&variants, requested);
    if options.is_empty() {
        return Ok(None);
    }
    Ok(Some(options[cast::draw_voice(m, options.len())?]))
}

pub(super) fn install(b: &mut Builder, pending: u32) {
    b.raw().before(
        &points::UNIT_DIALOGUE_SET_VARIANT,
        "select the converted unit's native voice variant",
        move |call| {
            let unit = call.cpu.eax;
            let stack = call.cpu.esp;
            let m: &mut dyn GuestMemory = &mut *call.ctx.runtime().memory();
            (|| {
                let voice = if let Some(registry) = voices::Registry::read(m)? {
                    voices::Table::current(m, registry, pending)?
                        .get(m, unit)?
                        .voice
                } else {
                    0
                };
                if let Some(variant) = choose(m, unit, voice)? {
                    m.set_u32(stack + 4, variant.name)?;
                }
                Ok(())
            })()
            .map_err(guest_fault)?;
            Ok(Flow::Continue)
        },
    );
}

/// A leader can be assigned to a unit that was already placed. Update the
/// same two cached fields the native variant path writes at 0x114ff3.
pub(super) fn rebind(m: &mut dyn GuestMemory, unit: u32, voice: i16) -> Result<(), MemFault> {
    if let Some(cache) = speech(m, unit)?
        && let Some(variant) = choose(m, unit, voice)?
        && let Some(root) = tag_address(m, variant.dialogue, UDLG)?
    {
        let designator = m.u32(root + 0x14)?;
        m.set_u32(cache, variant.dialogue)?;
        m.set_u32(cache + 4, designator)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use guest_layout::datum::layout;
    use guest_layout::tags::{TAG_ENTRIES_GLOBAL, TAG_INDEX_GLOBAL, entry_layout, index_layout};
    use guest_layout::testing::FakeGuest;

    use super::*;

    #[test]
    fn an_exact_source_slot_reads_the_speaking_units_cached_voice() {
        let mut m = FakeGuest::new();
        let objects = 0x10000;
        let object_rows = 0x11000;
        let index = 0x12000;
        let tags = 0x13000;
        let slot = 0x9e;
        m.put_u32(points::OBJECTS.base, objects)
            .put_u32(objects + layout::DATA, object_rows)
            .put_u32(objects + layout::HIGH_WATER, 2)
            .put_u32(objects + layout::ELEMENT_SIZE, 12)
            .put_u32(TAG_INDEX_GLOBAL, index)
            .put_u32(TAG_ENTRIES_GLOBAL, tags)
            .put_u32(index + index_layout::TAG_COUNT, 4);
        let unit = |i: u32| 0x8400_0000 | i;
        let voice = |i: u32| 0xe100_0000 | i;
        let sound = |i: u32| 0xe200_0002 | i;
        for i in 0..2 {
            let body = 0x20000 + i * 0x1000;
            let root = 0x30000 + i * 0x1000;
            let rows = root + 0x100;
            let voice_entry = tags + i * entry_layout::STRIDE;
            let sound_entry = tags + (2 + i) * entry_layout::STRIDE;
            m.put_u16(object_rows + 12 * i, 0x8400)
                .put_u32(object_rows + 12 * i + 8, body)
                .put_u16(body + SPEECH_OFFSET, 0x500)
                .put_u32(body + 0x500, voice(i))
                .put_u32(voice_entry + entry_layout::GROUP, UDLG.0)
                .put_u32(voice_entry + entry_layout::DATUM, voice(i))
                .put_u32(voice_entry + entry_layout::ADDRESS, root)
                .put_u32(root + 0xc, 1)
                .put_u32(root + 0x10, rows)
                .put_u32(rows, H1_DIALOGUE_SLOT | u32::from(slot))
                .put_u32(rows + 0xc, sound(i))
                .put_u32(sound_entry + entry_layout::GROUP, SND.0)
                .put_u32(sound_entry + entry_layout::DATUM, sound(i))
                .put_u32(sound_entry + entry_layout::ADDRESS, 0x40000 + i * 0x1000);
        }
        assert_eq!(source_sound(&m, unit(0), slot).unwrap(), sound(0));
        assert_eq!(source_sound(&m, unit(1), slot).unwrap(), sound(1));
        assert_eq!(source_sound(&m, unit(0), slot + 1).unwrap(), NONE);
        assert_eq!(source_sound(&m, 0x8500_0000, slot).unwrap(), NONE);
    }

    #[test]
    fn exact_leader_and_generic_alternatives_preserve_source_order() {
        let make = |selector, dialogue| Variant {
            selector,
            dialogue,
            name: dialogue + 10,
        };
        let authored = [make(0, 1), make(0, 2), make(100, 3), make(0, 4)];
        assert_eq!(candidates(&authored, 100), [authored[2]]);
        assert_eq!(
            candidates(&authored, 101),
            [authored[0], authored[1], authored[3]]
        );
        assert_eq!(
            candidates(&authored, 0),
            [authored[0], authored[1], authored[3]]
        );
        assert_eq!(candidates(&authored[2..3], 0), [authored[2]]);
        assert!(candidates(&[], 0).is_empty());
    }
}
