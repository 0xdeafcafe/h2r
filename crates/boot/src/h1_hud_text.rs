//! Restore the legacy HUD message source behind Halo 2's text renderer.
//! Retail script evaluators maintain the selection, objective hold time,
//! flash state and visibility. See docs/halo1/text.md.

use std::cell::RefCell;

use engine::handle::DatumHandle;
use engine::memory::{GuestMemory, MemFault};
use engine::points::{HUD_TEXT_SETUP, HUD_TEXT_SUBSTITUTE, SCENARIO};
use engine::tags::{entry_layout, tag_entry_checked};
use feature::{BuildError, Builder, Manifest};
use h1text::hud::{GROUP, MARKER, MARKER_AT, SCENARIO_HMT, SIZE, glyph};

mod controls;
mod flash;
mod timer;

const HUD: u32 = 0x0050_23f4;
const HUD_FLAGS: u32 = 0x0051_0c98;
const HELP_SHOWN: u32 = 1;
const HELP_MESSAGE: u32 = 0x138c;
const OBJECTIVE_MESSAGE: u32 = 0x1390;
const OBJECTIVE_TIME: u32 = 0x1394;

// Locals of the draw at 0x24cf66, and its two native call return addresses.
const SCREEN_LOCAL: u32 = 0x40;
const HAS_TEXT_LOCAL: u32 = 0x0d;
const FROM_SETUP: u32 = 0x0024_d28c;
const FROM_SUBSTITUTE: u32 = 0x0024_d2c5;

// The inline-text branch copies into the draw's own stack buffer at
// 0x24d2b7. The caller there always invokes the substitution pass next.
const TEXT: u32 = 0x220;
const TEXT_ID: u32 = 0x440;
const TEXT_CAPACITY: usize = 256;
const WORKSPACE_SIZE: usize = (TEXT_ID + 4 - TEXT) as usize;

struct Saved {
    address: u32,
    original: Vec<u8>,
}

thread_local! {
    // Both callbacks run on the same thread in one guest draw. No game
    // state survives the second callback.
    static SAVED: RefCell<Option<Saved>> = const { RefCell::new(None) };
}

fn selected_message(mem: &dyn GuestMemory, hud: u32) -> Result<(u32, bool), MemFault> {
    let objective = mem.u32(hud + OBJECTIVE_MESSAGE)?;
    if objective != 0 && mem.u16(hud + OBJECTIVE_TIME)? != 0 {
        return Ok((objective, false));
    }
    let flags = mem.u32(HUD_FLAGS)?;
    if flags != 0 && mem.u8(flags + HELP_SHOWN)? != 0 {
        return Ok((mem.u32(hud + HELP_MESSAGE)?, true));
    }
    Ok((0, false))
}

fn message_text(
    mem: &dyn GuestMemory,
    tag: u32,
    message: u32,
) -> Result<Option<Vec<u8>>, MemFault> {
    let messages = mem.u32(tag + 0x14)?;
    let count = mem.u32(tag + 0x10)?;
    let Some(offset) = message.checked_sub(messages) else {
        return Ok(None);
    };
    if offset % 0x40 != 0 || offset / 0x40 >= count {
        return Ok(None);
    }

    let mut unit = u32::from(mem.u16(message + 0x20)?);
    let first = u32::from(mem.u16(message + 0x22)?);
    let end = first + u32::from(mem.u8(message + 0x24)?);
    if end > mem.u32(tag + 8)? {
        return Ok(None);
    }
    let text_size = mem.u32(tag)?;
    let text = mem.u32(tag + 4)?;
    let elements = mem.u32(tag + 0xc)?;
    let mut result = Vec::new();
    for index in first..end {
        let element = elements + 2 * index;
        let value = mem.u8(element + 1)?;
        match mem.u8(element)? {
            0 => {
                let len = u32::from(value);
                if len == 0 || 2 * (unit + len) > text_size {
                    return Ok(None);
                }
                let at = text + 2 * unit;
                result.extend_from_slice(&mem.bytes(at, 2 * (len as usize - 1))?);
                unit += len;
            }
            1 => {
                let Some(glyph) = glyph(value) else {
                    return Ok(None);
                };
                result.extend_from_slice(&glyph.to_le_bytes());
            }
            _ => return Ok(None),
        }
    }
    // The game's destination is 256 UTF-16 units including the terminator.
    result.truncate(2 * (TEXT_CAPACITY - 1));
    if result.is_empty() {
        return Ok(None);
    }
    result.extend_from_slice(&[0, 0]);
    Ok(Some(result))
}

fn mission_hud(mem: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    let scenario = mem.u32(SCENARIO.base)?;
    if scenario == 0 {
        return Ok(None);
    }
    let reference = scenario + SCENARIO_HMT;
    if mem.u32(reference)? != GROUP.as_u32() {
        return Ok(None);
    }
    let datum = DatumHandle(mem.u32(reference + 4)?);
    let Some(entry) = tag_entry_checked(mem, datum)? else {
        return Ok(None);
    };
    if mem.u32(entry + entry_layout::GROUP)? != GROUP.as_u32()
        || mem.u32(entry + entry_layout::SIZE)? < SIZE
    {
        return Ok(None);
    }
    let tag = mem.u32(entry + entry_layout::ADDRESS)?;
    Ok((mem.u32(tag + MARKER_AT)? == MARKER).then_some(tag))
}

fn stage(
    mem: &mut dyn GuestMemory,
    frame: u32,
    primary: u32,
    shadow: u32,
) -> Result<Option<Saved>, MemFault> {
    let Some(tag) = mission_hud(mem)? else {
        return Ok(None);
    };
    let hud = mem.u32(HUD)?;
    if hud == 0 {
        return Ok(None);
    }
    let (selected, help) = selected_message(mem, hud)?;
    let Some(text) = message_text(mem, tag, selected)? else {
        return Ok(None);
    };

    let screen = mem.u32(frame - SCREEN_LOCAL)?;
    if screen == 0 {
        return Ok(None);
    }
    let address = screen + TEXT;
    if help {
        let colour = flash::help_colour(mem, tag, hud)?;
        let bytes = colour.map(f32::to_le_bytes).concat();
        mem.write(primary, &bytes)?;
        mem.write(shadow, &bytes)?;
    }
    let original = mem.bytes(address, WORKSPACE_SIZE)?;
    let mut workspace = original.clone();
    workspace[..text.len()].copy_from_slice(&text);
    workspace[WORKSPACE_SIZE - 4..].fill(0); // route the draw through inline text
    mem.set_u8(frame - HAS_TEXT_LOCAL, 1)?;
    mem.write(address, &workspace)?;
    Ok(Some(Saved { address, original }))
}

pub(crate) fn install(registry: &mut feature::Registry) -> Result<(), BuildError> {
    let feature = feature::FnFeature {
        manifest: Manifest::new("h1-hud-text", "Halo 1 help and objectives"),
        build: |b: &mut Builder| {
            b.snapshot(feature::SnapshotPolicy::Stateless);
            b.raw()
                .before(&HUD_TEXT_SETUP, "supply the selected mission text", |c| {
                    let sp = c.cpu.esp;
                    let bp = c.cpu.ebp;
                    let primary = c.cpu.edi;
                    let mut mem = c.ctx.memory();
                    if mem.u32(sp).ok() == Some(FROM_SETUP) {
                        let shadow = mem.u32(sp + 16).unwrap_or(0);
                        SAVED.with(|saved| {
                            *saved.borrow_mut() =
                                stage(&mut *mem, bp, primary, shadow).ok().flatten();
                        });
                    }
                    Ok(feature::Flow::Continue)
                })
                .context_only();
            b.raw()
                .before(
                    &HUD_TEXT_SUBSTITUTE,
                    "return the native message workspace",
                    |c| {
                        let sp = c.cpu.esp;
                        let mut mem = c.ctx.memory();
                        if mem.u32(sp).ok() == Some(FROM_SUBSTITUTE) {
                            SAVED.with(|saved| {
                                if let Some(saved) = saved.borrow_mut().take() {
                                    let _ = mem.write(saved.address, &saved.original);
                                }
                            });
                        }
                        Ok(feature::Flow::Continue)
                    },
                )
                .context_only();
            controls::register(b);
            timer::register(b);
            Ok(())
        },
    };
    registry.add(&feature)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::memory::FlatMemory;

    #[test]
    fn legacy_message_copies_text_and_controller_action() {
        let mut mem = FlatMemory::new(0x1000, 0x1000);
        let tag = 0x1100;
        let text = 0x1200;
        let elements = 0x1300;
        let message = 0x1400;
        for (offset, value) in [
            (0, 10),
            (4, text),
            (8, 3),
            (0xc, elements),
            (0x10, 1),
            (0x14, message),
        ] {
            mem.set_u32(tag + offset, value).unwrap();
        }
        let runs = "a\0bc\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        mem.write(text, &runs).unwrap();
        mem.write(elements, &[0, 2, 1, 26, 0, 3]).unwrap();
        mem.set_u8(message + 0x24, 3).unwrap();
        let out = "a\u{e451}bc\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        assert_eq!(message_text(&mem, tag, message).unwrap(), Some(out));
        assert_eq!(message_text(&mem, tag, message + 1).unwrap(), None);

        mem.set_u16(message + 0x20, 4).unwrap();
        assert_eq!(message_text(&mem, tag, message).unwrap(), None);
    }

    #[test]
    fn objectives_take_priority_then_help_returns() {
        let mut mem = FlatMemory::new(0x0050_0000, 0x0002_0000);
        let hud = 0x0051_8000;
        let flags = 0x0051_a000;
        mem.set_u32(HUD_FLAGS, flags).unwrap();
        mem.set_u8(flags + HELP_SHOWN, 1).unwrap();
        mem.set_u32(hud + HELP_MESSAGE, 1).unwrap();
        mem.set_u32(hud + OBJECTIVE_MESSAGE, 2).unwrap();
        mem.set_u16(hud + OBJECTIVE_TIME, 8).unwrap();
        assert_eq!(selected_message(&mem, hud).unwrap(), (2, false));
        mem.set_u16(hud + OBJECTIVE_TIME, 0).unwrap();
        assert_eq!(selected_message(&mem, hud).unwrap(), (1, true));
        mem.set_u8(flags + HELP_SHOWN, 0).unwrap();
        assert_eq!(selected_message(&mem, hud).unwrap(), (0, false));
    }
}
