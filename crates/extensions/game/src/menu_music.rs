//! Retail menu music and attract timer state for `audio.loops`.

use guest_layout::points::{
    GLOBALS_TAG, SCENARIO, UI_ATTRACT_RESET, UI_ATTRACT_TIMEOUT, UI_CLOCK, UI_LAST_INPUT,
    UI_MUSIC_GATE, UI_MUSIC_STATE,
};
use guest_layout::tags::{self, GroupTag, entry_layout};
use guest_layout::{DatumHandle, GuestMemory, MemFault};
use serde::Serialize;

const WIGL_ATTRACT_MUSIC_LEAD: u32 = 0x1c0;

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct MenuMusic {
    pub ui_ms: u32,
    pub idle_ms: u32,
    pub attract_timeout_ms: u32,
    pub music_stop_at_idle_ms: Option<u32>,
    pub music_allowed: bool,
    pub music_active: bool,
    pub music_requested: bool,
    pub attract_reset_ui_ms: u32,
    pub movie_active: bool,
}

impl MenuMusic {
    pub fn read(m: &dyn GuestMemory) -> Result<Option<Self>, MemFault> {
        use bink_hle::addr::{ST_ACTIVE, ST_BINK};

        let Some(scenario) = SCENARIO.pointer(m)? else {
            return Ok(None);
        };
        // The same main-menu scenario test as the music request (0x235bfa).
        if m.u16(scenario.wrapping_add(0x10))? != 2 {
            return Ok(None);
        }
        let now = m.u32(UI_CLOCK.base)?;
        let input = m.u32(UI_LAST_INPUT.base)?;
        let reset = m.u32(UI_ATTRACT_RESET.base)?;
        let timeout = m.u32(UI_ATTRACT_TIMEOUT.base)?;
        let lead = wigl(m)
            .ok()
            .flatten()
            .and_then(|addr| m.u32(addr.wrapping_add(WIGL_ATTRACT_MUSIC_LEAD)).ok());
        Ok(Some(Self {
            ui_ms: now,
            idle_ms: now.wrapping_sub(input.max(reset)),
            attract_timeout_ms: timeout,
            music_stop_at_idle_ms: lead.map(|ms| timeout.wrapping_sub(ms)),
            music_allowed: m.u8(UI_MUSIC_GATE.base)? != 0,
            music_active: m.u32(UI_MUSIC_STATE.base)? == 1,
            music_requested: m.u32(UI_MUSIC_STATE.base + 4)? == 1,
            attract_reset_ui_ms: reset,
            movie_active: m.u32(ST_BINK)? != 0 && m.u8(ST_ACTIVE)? != 0,
        }))
    }

    pub fn changed_since(&self, previous: &Self) -> bool {
        self.music_allowed != previous.music_allowed
            || self.music_active != previous.music_active
            || self.music_requested != previous.music_requested
            || self.attract_reset_ui_ms != previous.attract_reset_ui_ms
            || self.movie_active != previous.movie_active
    }
}

/// Main-menu path through the interface reference block and the widget
/// globals to the shared UI tag (0x1482e8 and 0x148350).
fn wigl(m: &dyn GuestMemory) -> Result<Option<u32>, MemFault> {
    let Some(globals) = GLOBALS_TAG.pointer(m)? else {
        return Ok(None);
    };
    if m.u32(globals.wrapping_add(0x110))? == 0 {
        return Ok(None);
    }
    let interface = m.u32(globals.wrapping_add(0x114))?;
    if interface == 0 {
        return Ok(None);
    }
    let wgtz = tag_address(m, m.u32(interface.wrapping_add(0x84))?, *b"wgtz")?;
    match wgtz {
        Some(addr) => tag_address(m, m.u32(addr.wrapping_add(4))?, *b"wigl"),
        None => Ok(None),
    }
}

fn tag_address(m: &dyn GuestMemory, raw: u32, group: [u8; 4]) -> Result<Option<u32>, MemFault> {
    let Some(tag) = DatumHandle::from_raw(raw) else {
        return Ok(None);
    };
    let Some(entry) = tags::tag_entry_checked(m, tag)? else {
        return Ok(None);
    };
    if m.u32(entry.wrapping_add(entry_layout::GROUP))? != GroupTag::from_chars(group).0 {
        return Ok(None);
    }
    let addr = m.u32(entry.wrapping_add(entry_layout::ADDRESS))?;
    Ok((addr != 0).then_some(addr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::points::{TAG_ENTRIES, TAG_INDEX};
    use guest_layout::tags::index_layout;
    use guest_layout::testing::FakeGuest;

    #[test]
    fn menu_timer_distinguishes_music_stop_from_movie_start_at_present_2300() {
        let mut g = FakeGuest::new();
        let scenario = 0x10000;
        let globals = 0x11000;
        let interface = 0x12000;
        let wgtz = 0x13000;
        let wigl = 0x14000;
        let index = 0x15000;
        let entries = 0x16000;
        let wgtz_id = DatumHandle::new(0x80, 0);
        let wigl_id = DatumHandle::new(0x81, 1);
        let input_ms = 200;
        let reset_ms = 400;
        let timeout_ms = 79_650;
        let lead_ms = 3_000;
        g.put_u32(SCENARIO.base, scenario)
            .put_u16(scenario + 0x10, 2)
            .put_u32(GLOBALS_TAG.base, globals)
            .put_u32(globals + 0x110, 1)
            .put_u32(globals + 0x114, interface)
            .put_u32(interface + 0x84, wgtz_id.raw())
            .put_u32(wgtz + 4, wigl_id.raw())
            .put_u32(wigl + WIGL_ATTRACT_MUSIC_LEAD, lead_ms)
            .put_u32(TAG_INDEX.base, index)
            .put_u32(index + index_layout::TAG_COUNT, 2)
            .put_u32(TAG_ENTRIES.base, entries)
            .put_u32(UI_LAST_INPUT.base, input_ms)
            .put_u32(UI_ATTRACT_RESET.base, reset_ms)
            .put_u32(UI_ATTRACT_TIMEOUT.base, timeout_ms)
            .put_u32(bink_hle::addr::ST_BINK, 0);
        for (id, group, addr) in [(wgtz_id, *b"wgtz", wgtz), (wigl_id, *b"wigl", wigl)] {
            let entry = entries + u32::from(id.index()) * entry_layout::STRIDE;
            g.put_u32(entry + entry_layout::DATUM, id.raw())
                .put_u32(entry + entry_layout::GROUP, GroupTag::from_chars(group).0)
                .put_u32(entry + entry_layout::ADDRESS, addr);
        }
        let ui_ms = |present: u32| reset_ms + present * 1000 / 30;
        g.put_u32(UI_CLOCK.base, ui_ms(2299))
            .put_u8(UI_MUSIC_GATE.base, 1)
            .put_u32(UI_MUSIC_STATE.base, 1)
            .put_u32(UI_MUSIC_STATE.base + 4, 1);
        let before = MenuMusic::read(&g).unwrap().unwrap();
        assert!(before.idle_ms < timeout_ms - lead_ms);
        assert!(before.music_allowed && before.music_requested && before.music_active);

        // Retail closes the gate before the movie deadline. The current
        // music mode can still be active while it processes the request.
        g.put_u32(UI_CLOCK.base, ui_ms(2300))
            .put_u8(UI_MUSIC_GATE.base, 0)
            .put_u32(UI_MUSIC_STATE.base + 4, 0);
        let at = MenuMusic::read(&g).unwrap().unwrap();
        assert_eq!(at.music_stop_at_idle_ms, Some(timeout_ms - lead_ms));
        assert!((timeout_ms - lead_ms..timeout_ms).contains(&at.idle_ms));
        assert!(at.changed_since(&before));
        assert!(at.music_active);
        assert!(!at.music_allowed && !at.music_requested && !at.movie_active);

        let skipped_movie_ms = reset_ms + timeout_ms;
        g.put_u32(UI_ATTRACT_RESET.base, skipped_movie_ms)
            .put_u32(UI_CLOCK.base, skipped_movie_ms + 33)
            .put_u8(UI_MUSIC_GATE.base, 1)
            .put_u32(UI_MUSIC_STATE.base + 4, 1);
        let after = MenuMusic::read(&g).unwrap().unwrap();
        assert_eq!(after.attract_reset_ui_ms, skipped_movie_ms);
        assert_eq!(after.idle_ms, 33);
        assert!(after.music_allowed && after.music_requested);
        assert!(!after.movie_active);
    }
}
