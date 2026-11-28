//! Restore the legacy HUD message source behind Halo 2's text renderer.
//! Retail script evaluators maintain the selection, objective hold time,
//! flash state and visibility. See docs/halo1/text.md.

use std::cell::{Cell, RefCell};

use guest_layout::handle::DatumHandle;
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points::{
    HUD_TEXT_BOUNDS, HUD_TEXT_DRAW, HUD_TEXT_MEASURE, HUD_TEXT_SETUP, HUD_TEXT_SUBSTITUTE, SCENARIO,
};
use guest_layout::tags::{entry_layout, tag_entry_checked};
use hooks::{BuildError, Builder, Manifest};
use h1_contract::hud::{
    EVENT_LAYOUT_SIZE, EVENT_MARKER, GROUP, LEGACY_MARKER, LEGACY_SIZE, MARKER, MARKER_AT,
    SCENARIO_HMT, SIZE, glyph,
};

mod controls;
mod events;
mod flash;
mod prompts;
mod silhouettes;
mod timer;
mod tint;
mod weapon;

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
const FROM_MEASURE: u32 = 0x0024_d2d2;
const FROM_BOUNDS: u32 = 0x0024_d2eb;
const FROM_DRAW: u32 = 0x0024_d2f8;

// The inline-text branch copies into the draw's own stack buffer at
// 0x24d2b7. The caller there always invokes the substitution pass next.
const TEXT: u32 = 0x220;
const TEXT_ID: u32 = 0x440;
const TEXT_CAPACITY: usize = 2048;
const WORKSPACE_SIZE: usize = (TEXT_ID + 4 - TEXT) as usize;

fn list_row(mem: &dyn GuestMemory, tag: u32, list: u32, index: u16) -> Result<Option<u32>, MemFault> {
    if index >= mem.u16(tag + list + 2)? {
        return Ok(None);
    }
    let index = u32::from(mem.u16(tag + list)?) + u32::from(index);
    message_at(mem, tag, index)
}

fn message_at(mem: &dyn GuestMemory, tag: u32, index: u32) -> Result<Option<u32>, MemFault> {
    if index >= mem.u32(tag + 0x10)? {
        return Ok(None);
    }
    Ok(Some(mem.u32(tag + 0x14)? + 0x40 * index))
}

fn show_scenario_help(mem: &mut dyn GuestMemory, tag: u32, index: u16) -> Result<(), MemFault> {
    if !extended(mem, tag)? {
        return Ok(());
    }
    let Some(row) = list_row(mem, tag, h1_contract::hud::SCENARIO_HELP, index)? else {
        return Ok(());
    };
    let hud = mem.u32(HUD)?;
    let flags = mem.u32(HUD_FLAGS)?;
    if hud != 0 && flags != 0 {
        mem.set_u32(hud + HELP_MESSAGE, row)?;
        mem.set_u8(flags + HELP_SHOWN, 1)?;
    }
    Ok(())
}

fn extended(mem: &dyn GuestMemory, tag: u32) -> Result<bool, MemFault> {
    Ok(matches!(mem.u32(tag + MARKER_AT)?, MARKER | EVENT_MARKER))
}

struct Saved {
    address: u32,
    original: Vec<u8>,
}

thread_local! {
    // Both callbacks run on the same thread in one guest draw. No game
    // state survives the second callback.
    static SAVED: RefCell<Option<Saved>> = const { RefCell::new(None) };
    static DRAW_TEXT: Cell<u32> = const { Cell::new(0) };
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
    arguments: &[Option<&[u8]>],
    capacity: usize,
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
            1 if value >= 32 => {
                let Some(Some(arg)) = arguments.get(usize::from(value - 32)) else {
                    return Ok(None);
                };
                result.extend_from_slice(arg);
            }
            1 => result.extend_from_slice(&glyph(value).expect("controller glyph").to_le_bytes()),
            _ => return Ok(None),
        }
    }
    if result.len() >= 2 * capacity {
        result.truncate(2 * (capacity - 1));
        let last = u16::from_le_bytes(result[result.len() - 2..].try_into().unwrap());
        if (0xd800..=0xdbff).contains(&last) {
            result.truncate(result.len() - 2);
        }
    }
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
    let size = mem.u32(entry + entry_layout::SIZE)?;
    if mem.u32(entry + entry_layout::GROUP)? != GROUP.as_u32() || size < LEGACY_SIZE {
        return Ok(None);
    }
    let tag = mem.u32(entry + entry_layout::ADDRESS)?;
    let supported = match mem.u32(tag + MARKER_AT)? {
        MARKER => size >= SIZE,
        EVENT_MARKER => size >= EVENT_LAYOUT_SIZE,
        LEGACY_MARKER => true,
        _ => false,
    };
    Ok(supported.then_some(tag))
}

// Follow the same local-user to player to input-settings path as 0x22df04.
// Move and look need the stick deflection icons, not the click icons.
fn stick_layout(mem: &dyn GuestMemory, user: u32) -> Result<Option<u16>, MemFault> {
    if user >= 4 {
        return Ok(None);
    }
    let local_users = mem.u32(0x004e_8c20)?;
    let player = mem.u32(local_users + 0xc + 4 * user)?;
    if player == u32::MAX {
        return Ok(None);
    }
    let players = mem.u32(0x004e_8c24)?;
    let player = mem.u32(players + 0x44)? + (player & 0xffff) * 0x21c;
    let port = mem.u32(player + 0x24)?;
    if port >= 4 {
        return Ok(None);
    }
    let layout = mem.u16(0x0051_ea18 + 0x1c * port + 0x18)?;
    Ok((layout < 4).then_some(layout))
}

fn substitute_sticks(text: &mut [u8], layout: u16) {
    for [low, high] in text.as_chunks_mut::<2>().0 {
        let action = u16::from_le_bytes([*low, *high]);
        if matches!(action, 0xe456 | 0xe457) {
            let right = (layout & 1) ^ (action - 0xe456);
            [*low, *high] = (0xe10e + right).to_le_bytes();
        }
    }
}

fn stage(
    mem: &mut dyn GuestMemory,
    frame: u32,
    primary: u32,
    shadow: u32,
    scratch: u32,
) -> Result<Option<Saved>, MemFault> {
    let Some(tag) = mission_hud(mem)? else {
        return Ok(None);
    };
    let hud = mem.u32(HUD)?;
    if hud == 0 {
        return Ok(None);
    }
    let (selected, help) = selected_message(mem, hud)?;
    let Some(mut text) = message_text(mem, tag, selected, &[], TEXT_CAPACITY)? else {
        return Ok(None);
    };
    let user = mem.u32(frame + 8)?;
    if user >= 4 {
        return Ok(None);
    }
    if let Ok(Some(layout)) = stick_layout(mem, user) {
        substitute_sticks(&mut text, layout);
    }

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
    workspace[..2].fill(0);
    workspace[WORKSPACE_SIZE - 4..].fill(0); // route the draw through inline text
    let scratch = scratch + user * (TEXT_CAPACITY as u32 * 2);
    mem.write(scratch, &text)?;
    mem.set_u8(frame - HAS_TEXT_LOCAL, 1)?;
    mem.write(address, &workspace)?;
    DRAW_TEXT.set(scratch);
    Ok(Some(Saved { address, original }))
}

/// The feature's id, for the features that order their hooks around it.
pub const FEATURE: &str = "h1-hud-text";

pub fn install(registry: &mut hooks::Registry) -> Result<(), BuildError> {
    let feature = hooks::FnFeature {
        manifest: Manifest::new(FEATURE, "Halo 1 help and objectives"),
        build: |b: &mut Builder| {
            b.snapshot(hooks::SnapshotPolicy::Stateless);
            let scratch = b.guest_alloc(
                4 * TEXT_CAPACITY as u32 * 2,
                runtime::guest::Permissions::RW,
            )?;
            b.raw()
                .before(&HUD_TEXT_SETUP, "supply the selected mission text", move |c| {
                    let sp = c.cpu.esp;
                    let bp = c.cpu.ebp;
                    let primary = c.cpu.edi;
                    let mut mem = c.ctx.memory();
                    if mem.u32(sp).ok() == Some(FROM_SETUP) {
                        DRAW_TEXT.set(0);
                        let shadow = mem.u32(sp + 16).unwrap_or(0);
                        SAVED.with(|saved| {
                            *saved.borrow_mut() =
                                stage(&mut *mem, bp, primary, shadow, scratch).ok().flatten();
                        });
                    }
                    Ok(hooks::Flow::Continue)
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
                                    let _ = mem.write_bytes(saved.address, &saved.original, 1);
                                }
                            });
                            let text = DRAW_TEXT.get();
                            if text != 0 {
                                let _ = mem.write_bytes(
                                    sp + 4,
                                    &[text, TEXT_CAPACITY as u32].map(u32::to_le_bytes).concat(),
                                    1,
                                );
                            }
                        }
                        Ok(hooks::Flow::Continue)
                    },
                )
                .context_only();
            b.raw()
                .before(&HUD_TEXT_MEASURE, "measure the selected text", |c| {
                    if c.ctx.memory().u32(c.cpu.esp).ok() == Some(FROM_MEASURE)
                        && DRAW_TEXT.get() != 0
                    {
                        c.cpu.ecx = DRAW_TEXT.get();
                    }
                    Ok(hooks::Flow::Continue)
                })
                .context_only();
            b.raw()
                .before(&HUD_TEXT_BOUNDS, "lay out the selected text", |c| {
                    let sp = c.cpu.esp;
                    let mut mem = c.ctx.memory();
                    if mem.u32(sp).ok() == Some(FROM_BOUNDS) && DRAW_TEXT.get() != 0 {
                        let _ = mem.set_u32(sp + 4, DRAW_TEXT.get());
                    }
                    Ok(hooks::Flow::Continue)
                })
                .context_only();
            b.raw()
                .before(&HUD_TEXT_DRAW, "draw the selected text", |c| {
                    if c.ctx.memory().u32(c.cpu.esp).ok() == Some(FROM_DRAW)
                        && DRAW_TEXT.get() != 0
                    {
                        c.cpu.ecx = DRAW_TEXT.replace(0);
                    }
                    Ok(hooks::Flow::Continue)
                })
                .context_only();
            controls::register(b);
            events::register(b);
            prompts::register(b);
            silhouettes::register(b);
            timer::register(b);
            let rows = weapon::register(b)?;
            tint::register(b, rows);
            Ok(())
        },
    };
    registry.add(&feature)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

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
        assert_eq!(message_text(&mem, tag, message, &[], TEXT_CAPACITY).unwrap(), Some(out));
        assert_eq!(message_text(&mem, tag, message + 1, &[], TEXT_CAPACITY).unwrap(), None);

        mem.set_u16(message + 0x20, 4).unwrap();
        assert_eq!(message_text(&mem, tag, message, &[], TEXT_CAPACITY).unwrap(), None);
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

    /// `display_scenario_help` (slot 903) shows the scenario's converted
    /// help row in the native help panel, as Halo 1's 0xf2f30 does with
    /// its help list. Rows past the list, or a tag without the conversion
    /// marker, change nothing.
    #[test]
    fn scenario_help_shows_the_converted_row_in_the_help_panel() {
        let mut mem = FlatMemory::new(0x0050_0000, 0x0002_0000);
        let hud = 0x0051_8000;
        let flags = 0x0051_a000;
        let tag = 0x0051_c000;
        let messages = 0x0051_d000;
        mem.set_u32(HUD, hud).unwrap();
        mem.set_u32(HUD_FLAGS, flags).unwrap();
        mem.set_u32(tag + MARKER_AT, MARKER).unwrap();
        mem.set_u32(tag + 0x10, 12).unwrap();
        mem.set_u32(tag + 0x14, messages).unwrap();
        // Five help rows from message 7, after the mission's own messages.
        mem.set_u16(tag + h1_contract::hud::SCENARIO_HELP, 7).unwrap();
        mem.set_u16(tag + h1_contract::hud::SCENARIO_HELP + 2, 5).unwrap();

        show_scenario_help(&mut mem, tag, 5).unwrap();
        assert_eq!(mem.u8(flags + HELP_SHOWN).unwrap(), 0);
        assert_eq!(mem.u32(hud + HELP_MESSAGE).unwrap(), 0);

        show_scenario_help(&mut mem, tag, 3).unwrap();
        assert_eq!(mem.u8(flags + HELP_SHOWN).unwrap(), 1);
        assert_eq!(mem.u32(hud + HELP_MESSAGE).unwrap(), messages + 0x40 * 10);
        assert_eq!(selected_message(&mem, hud).unwrap(), (messages + 0x40 * 10, true));

        mem.set_u32(tag + MARKER_AT, 0).unwrap();
        show_scenario_help(&mut mem, tag, 4).unwrap();
        assert_eq!(mem.u32(hud + HELP_MESSAGE).unwrap(), messages + 0x40 * 10);
    }

    #[test]
    fn cave_help_uses_the_mapped_stick_and_preserves_the_literal_stick() {
        for (layout, move_glyph, look_glyph) in [
            (0, 0xe10e, 0xe10f),
            (1, 0xe10f, 0xe10e),
            (2, 0xe10e, 0xe10f),
            (3, 0xe10f, 0xe10e),
        ] {
            let mut text = [0xe456u16, 0xe457, 0xe10e, 0]
                .map(u16::to_le_bytes)
                .concat();
            substitute_sticks(&mut text, layout);
            assert_eq!(text, [move_glyph, look_glyph, 0xe10e, 0].map(u16::to_le_bytes).concat());
        }
    }
}
