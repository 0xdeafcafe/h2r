//! Draw the restored scenario timer through the legacy HUD renderer's
//! final, independent text pass. The native evaluators still own its
//! duration, pause, warning threshold and corner.

use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points::{HUD_TEXT_DRAW, HUD_TEXT_SETUP, HUD_TEXT_TAIL};
use hooks::{Builder, Flow};

use super::{HUD, flash, mission_hud};

const STARTED: u32 = 0x1398;
const DURATION: u32 = 0x139c;
const WARNING: u32 = 0x139e;
const X: u32 = 0x13a0;
const Y: u32 = 0x13a2;
const CORNER: u32 = 0x13a4;
const PAUSED: u32 = 0x13a6;
const SHOWN: u32 = 0x13a7;

const FROM_TAIL: u32 = 0x0024_d684;
const FROM_SETUP: u32 = 0x0024_d69d;
const FROM_DRAW: u32 = 0x0024_d6c0;
const VIEWPORT: u32 = 0x004b_9dd0;
const LINE_HEIGHT: i16 = 20;

#[derive(Clone, Copy)]
struct Timer {
    seconds: u32,
    warning: bool,
    corner: u16,
    x: i16,
    y: i16,
}

fn remaining(duration: u16, start: u32, now: u32, paused: bool) -> Option<i16> {
    if duration == u16::MAX {
        return None;
    }
    let ticks = if paused {
        duration as i16
    } else {
        duration.wrapping_sub(now.wrapping_sub(start) as u16) as i16
    };
    (ticks >= 0).then_some(ticks)
}

fn timer(mem: &dyn GuestMemory) -> Result<Option<Timer>, MemFault> {
    if mission_hud(mem)?.is_none() {
        return Ok(None);
    }
    let hud = mem.u32(HUD)?;
    if hud == 0 || mem.u8(hud + SHOWN)? == 0 {
        return Ok(None);
    }
    let game_time = mem.u32(flash::GAME_TIME)?;
    let Some(ticks) = remaining(
        mem.u16(hud + DURATION)?,
        mem.u32(hud + STARTED)?,
        mem.u32(game_time + 8)?,
        mem.u8(hud + PAUSED)? != 0,
    ) else {
        return Ok(None);
    };
    let seconds = (f32::from(ticks) * mem.f32(game_time + 4)?).ceil() as u32;
    Ok(Some(Timer {
        seconds,
        warning: ticks <= mem.u16(hud + WARNING)? as i16,
        corner: mem.u16(hud + CORNER)?.min(4),
        x: mem.u16(hud + X)? as i16,
        y: mem.u16(hud + Y)? as i16,
    }))
}

impl Timer {
    fn text(self) -> Vec<u8> {
        let alignment = match self.corner {
            0 | 3 => 0xe407,
            4 => 0xe406,
            _ => 0xe405,
        };
        let time = format!("{:02}:{:02}", self.seconds / 60, self.seconds % 60);
        std::iter::once(alignment)
            .chain(time.encode_utf16())
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    fn bounds(self, width: i16, height: i16) -> [i16; 4] {
        let right = matches!(self.corner, 0 | 3);
        let x = if right {
            self.x.saturating_neg()
        } else {
            self.x
        };
        let y = match self.corner {
            2 | 3 => height.saturating_sub(LINE_HEIGHT).saturating_sub(self.y),
            4 => (height / 2)
                .saturating_sub(LINE_HEIGHT / 2)
                .saturating_add(self.y),
            _ => self.y,
        };
        [y, x, y.saturating_add(LINE_HEIGHT), width.saturating_add(x)]
    }
}

fn place(mem: &mut dyn GuestMemory, rect: u32, timer: Timer) -> Result<(), MemFault> {
    let at = |offset: u32| mem.u16(VIEWPORT + offset).map(|v| v as i16);
    let height = at(0xc)?.saturating_sub(at(0)?);
    let width = at(0xe)?.saturating_sub(at(2)?);
    let bounds = timer.bounds(width, height).map(i16::to_le_bytes).concat();
    mem.write(rect, &bounds)
}

pub(super) fn register(b: &mut Builder) {
    b.raw()
        .before(&HUD_TEXT_TAIL, "supply the script countdown", |c| {
            let sp = c.cpu.esp;
            let out = c.cpu.esi;
            let supplied = {
                let mut mem = c.ctx.memory();
                mem.u32(sp).ok() == Some(FROM_TAIL)
                    && timer(&*mem)
                        .ok()
                        .flatten()
                        .is_some_and(|timer| mem.write_bytes(out, &timer.text(), 1).is_ok())
            };
            if supplied {
                c.cpu.eax = 1;
                return c.ret(8).map(Flow::Return);
            }
            Ok(Flow::Continue)
        })
        .context_only();
    b.raw()
        .before(&HUD_TEXT_SETUP, "apply the countdown warning colour", |c| {
            let sp = c.cpu.esp;
            let primary = c.cpu.edi;
            let mut mem = c.ctx.memory();
            if mem.u32(sp).ok() == Some(FROM_SETUP)
                && timer(&*mem)
                    .ok()
                    .flatten()
                    .is_some_and(|timer| timer.warning)
            {
                let rgb = [1.0f32, 0.25, 0.25].map(f32::to_le_bytes).concat();
                let _ = mem.write_bytes(primary + 4, &rgb, 1);
            }
            Ok(Flow::Continue)
        })
        .context_only();
    b.raw()
        .before(
            &HUD_TEXT_DRAW,
            "place the countdown at its script anchor",
            |c| {
                let sp = c.cpu.esp;
                let rect = c.cpu.eax;
                let mut mem = c.ctx.memory();
                if mem.u32(sp).ok() == Some(FROM_DRAW)
                    && let Some(timer) = timer(&*mem).ok().flatten()
                {
                    let _ = place(&mut *mem, rect, timer);
                }
                Ok(Flow::Continue)
            },
        )
        .context_only();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_handles_pause_and_tick_wrap() {
        assert_eq!(remaining(120, u32::MAX - 19, 10, false), Some(90));
        assert_eq!(remaining(120, 10, 100, true), Some(120));
        assert_eq!(remaining(120, 10, 131, false), None);
        assert_eq!(remaining(u16::MAX, 10, 10, true), None);
    }

    #[test]
    fn countdown_uses_its_corner_and_keeps_seconds_padded() {
        let timer = Timer {
            seconds: 61,
            warning: false,
            corner: 3,
            x: 8,
            y: 12,
        };
        assert_eq!(timer.bounds(640, 480), [448, -8, 468, 632]);
        let mut text = vec![0xe407];
        text.extend("01:01\0".encode_utf16());
        assert_eq!(
            timer.text(),
            text.into_iter()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>()
        );
    }
}
