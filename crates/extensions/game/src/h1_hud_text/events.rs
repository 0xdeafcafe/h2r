//! Route the selected interaction and pickup fields through the Halo 1
//! event list before the native queue and formatter run.

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points::HUD_MESSAGE_LOOKUP;
use hooks::{Builder, Flow};
use h1_contract::hud::{CONTROL_PROMPTS, CUSTOM_DEVICE, EVENT_SIZE, EVENTS, SHARED_MESSAGES};

use super::{list_row, message_text, mission_hud};

const NATIVE_CAPACITY: usize = 256;
const INTERACTION_LOOKUP: u32 = 0x0024_ca05;
const EVENT_LOOKUPS: &[u32] = &[
    INTERACTION_LOOKUP,
    0x0024_cbdb, // pickup notification
    0x0019_1e95,
    0x0019_1ea6, // counted pickup, singular and plural
    0x0019_1f08,
    0x0019_1f19, // second counted pickup path
    0x0019_1f7e,
    0x0019_1f8f, // matching an earlier counted pickup
];

fn control_placement(mem: &dyn GuestMemory, frame: u32) -> Result<Option<u16>, MemFault> {
    // 0x191cf0 forms the interaction key from the device's object slot.
    let Some(slot) = mem.u32(frame + 0xc)?.checked_sub(0x400) else {
        return Ok(None);
    };
    let objects = mem.u32(0x004e_0300)?;
    if objects == 0 || slot >= mem.u32(objects + 0x38)? {
        return Ok(None);
    }
    let object = mem.u32(mem.u32(objects + 0x44)? + 12 * slot + 8)?;
    if object == 0 {
        return Ok(None);
    }
    // object_new at 0xb7c87 copies the scenario placement index here.
    Ok(Some(mem.u16(object + 0x1a)?))
}

fn control_argument(mem: &dyn GuestMemory, tag: u32, slot: u16) -> Result<Option<u16>, MemFault> {
    if u32::from(slot) >= mem.u32(tag + CONTROL_PROMPTS)? {
        return Ok(None);
    }
    let rows = mem.u32(tag + CONTROL_PROMPTS + 4)?;
    let row = mem.u16(rows + 2 * u32::from(slot))?;
    Ok((row != u16::MAX).then_some(row))
}

fn event_text(
    mem: &dyn GuestMemory,
    tag: u32,
    key: u32,
    interaction_frame: Option<u32>,
) -> Result<Option<Vec<u8>>, MemFault> {
    let count = mem.u32(tag + EVENTS)?;
    let routes = mem.u32(tag + EVENTS + 4)?;
    if routes == 0 || count > u32::from(u16::MAX) {
        return Ok(None);
    }
    for i in 0..count {
        let route = routes + i * EVENT_SIZE;
        if mem.u32(route)? != key {
            continue;
        }
        let mut template = mem.u16(route + 4)?;
        let mut indices = [mem.u16(route + 6)?, mem.u16(route + 8)?];
        if let Some(frame) = interaction_frame
            && mem.u16(route + 0xa)? != 0
            && let Some(placement) = control_placement(mem, frame)?
            && let Some(phrase) = control_argument(mem, tag, placement)?
        {
            template = mem.u16(tag + CUSTOM_DEVICE)?;
            indices[0] = phrase;
        }
        return render(mem, tag, template, indices);
    }
    Ok(None)
}

pub(super) fn render(
    mem: &dyn GuestMemory,
    tag: u32,
    template: u16,
    indices: [u16; 2],
) -> Result<Option<Vec<u8>>, MemFault> {
    let Some(message) = list_row(mem, tag, SHARED_MESSAGES, template)? else {
        return Ok(None);
    };
    let mut args: [Option<Vec<u8>>; 2] = [None, None];
    for (text, index) in args.iter_mut().zip(indices) {
        if index != u16::MAX {
            let Some(row) = list_row(mem, tag, SHARED_MESSAGES, index)? else {
                return Ok(None);
            };
            let Some(mut bytes) = message_text(mem, tag, row, &[], NATIVE_CAPACITY)? else {
                return Ok(None);
            };
            bytes.truncate(bytes.len() - 2);
            *text = Some(bytes);
        }
    }
    message_text(
        mem,
        tag,
        message,
        &[args[0].as_deref(), args[1].as_deref()],
        NATIVE_CAPACITY,
    )
}

fn replace(
    mem: &mut dyn GuestMemory,
    sp: u32,
    frame: u32,
    key: u32,
    output: u32,
) -> Result<bool, MemFault> {
    let caller = mem.u32(sp)?;
    if output == 0 || !EVENT_LOOKUPS.contains(&caller) {
        return Ok(false);
    }
    let Some(tag) = mission_hud(mem)? else {
        return Ok(false);
    };
    if !super::extended(mem, tag)? {
        return Ok(false);
    }
    let interaction_frame = (caller == INTERACTION_LOOKUP).then_some(frame);
    let Some(text) = event_text(mem, tag, key, interaction_frame)? else {
        return Ok(false);
    };
    mem.write(output, &text)?;
    Ok(true)
}

pub(super) fn register(b: &mut Builder) {
    b.raw()
        .before(&HUD_MESSAGE_LOOKUP, "resolve converted event text", |c| {
            let (sp, frame, key, output) = (c.cpu.esp, c.cpu.ebp, c.cpu.edi, c.cpu.ecx);
            let replaced = replace(&mut *c.ctx.memory(), sp, frame, key, output).unwrap_or(false);
            if replaced {
                return c.ret(0).map(Flow::Return);
            }
            Ok(Flow::Continue)
        })
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn native_event_key_picks_two_arguments_and_a_button_action() {
        let mut mem = FlatMemory::new(0x1000, 0x1000);
        let tag = 0x1100;
        let strings = 0x1200;
        let elements = 0x1300;
        let messages = 0x1400;
        let routes = 0x1600;
        let key = 0x0e00_1234;
        let utf16 = |s: &str| {
            s.encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        };
        let data = utf16(" in \0driver\0truck\0");
        for (at, value) in [
            (tag, data.len() as u32),
            (tag + 4, strings),
            (tag + 8, 6),
            (tag + 0xc, elements),
            (tag + 0x10, 3),
            (tag + 0x14, messages),
            (tag + SHARED_MESSAGES, 3 << 16),
            (tag + EVENTS, 1),
            (tag + EVENTS + 4, routes),
            (routes, key),
        ] {
            mem.set_u32(at, value).unwrap();
        }
        mem.write(strings, &data).unwrap();
        mem.write(elements, &[1, 18, 1, 32, 0, 5, 1, 33, 0, 7, 0, 6])
            .unwrap();
        for (i, start, first, count) in [(0, 0, 0, 4), (1, 5, 4, 1), (2, 12, 5, 1)] {
            let at = messages + i * 0x40;
            mem.set_u16(at + 0x20, start).unwrap();
            mem.set_u16(at + 0x22, first).unwrap();
            mem.set_u8(at + 0x24, count).unwrap();
        }
        mem.set_u16(routes + 6, 1).unwrap();
        mem.set_u16(routes + 8, 2).unwrap();
        assert_eq!(
            event_text(&mem, tag, key, None).unwrap(),
            Some(utf16("\u{e448}driver in truck\0"))
        );
        assert_eq!(event_text(&mem, tag, key + 1, None).unwrap(), None);

        mem.set_u16(strings + 10, h1_contract::hud::SILHOUETTE_BASE).unwrap();
        mem.set_u16(strings + 12, 0).unwrap();
        mem.set_u8(elements + 9, 2).unwrap();
        assert_eq!(
            event_text(&mem, tag, key, None).unwrap(),
            Some(utf16("\u{e448}\u{e500} in truck\0"))
        );

        mem.set_u16(routes + 8, u16::MAX).unwrap();
        assert_eq!(event_text(&mem, tag, key, None).unwrap(), None);
    }

    #[test]
    fn a_control_uses_the_phrase_of_its_live_placement() {
        let mut mem = FlatMemory::new(0x004e_0000, 0x2000);
        let frame = 0x004e_0100;
        let objects = 0x004e_1000;
        let headers = 0x004e_1100;
        let object = 0x004e_1200;
        let tag = 0x004e_1300;
        let phrases = 0x004e_1400;
        for (at, value) in [
            (frame + 0xc, 0x402),
            (0x004e_0300, objects),
            (objects + 0x38, 3),
            (objects + 0x44, headers),
            (headers + 12 * 2 + 8, object),
            (tag + CONTROL_PROMPTS, 2),
            (tag + CONTROL_PROMPTS + 4, phrases),
        ] {
            mem.set_u32(at, value).unwrap();
        }
        mem.set_u16(object + 0x1a, 1).unwrap();
        mem.set_u16(phrases, u16::MAX).unwrap();
        mem.set_u16(phrases + 2, 12).unwrap();
        assert_eq!(control_placement(&mem, frame).unwrap(), Some(1));
        assert_eq!(control_argument(&mem, tag, 1).unwrap(), Some(12));
        assert_eq!(control_argument(&mem, tag, 0).unwrap(), None);
        assert_eq!(control_argument(&mem, tag, u16::MAX).unwrap(), None);
    }
}
