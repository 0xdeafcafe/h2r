//! Menu compositor state at the same boundary as the inspected widgets.
//! The full-screen startup fade uses the unsigned UI clock difference
//! (0x147bd8). The modal blur (0x235128 -> 0x235756) uses the animation of
//! the incoming, active and outgoing screens on a populated lower layer.

use agent_proto::{At, Boundary, Phase};
use guest_layout::GuestMemory;
use serde_json::{Value, json};

use crate::list_class::{MAIN_MENU_KIND, UI_GAME_KIND};
use crate::ui_watch::{SHARED, channel};

const UI_NOW: u32 = guest_layout::points::UI_CLOCK.base;
const FADE_START: u32 = 0x0054_d5c0;
const SCENE_MODE: u32 = 0x0048_5870;
const SCENE_WEIGHT: u32 = 0x0048_5874;
const FADE_END_MS: u32 = 4000;
const GRACE_MS: u32 = 1000;
const HOLD_TICKS: u64 = 150;

#[derive(Default)]
pub(crate) struct History {
    first: Option<(u64, u32, u32, u64)>,
}

impl History {
    // Use game progress, so pausing for a debugger never establishes a hang.
    // Reset at a load, a changed clock or a fresh fade.
    fn held(&mut self, epoch: u64, sample: Option<(u32, u32)>, tick: u64) -> bool {
        let Some((start, elapsed)) = sample else {
            self.first = None;
            return false;
        };
        if let Some((e, s, ms, since)) = self.first
            && (e, s, ms) == (epoch, start, elapsed)
            && since <= tick
        {
            return tick - since >= HOLD_TICKS;
        }
        self.first = Some((epoch, start, elapsed, tick));
        false
    }

    pub(crate) fn read(
        &mut self,
        m: &dyn GuestMemory,
        at: &At,
        has_dialog: bool,
    ) -> (Value, Vec<Value>) {
        let now = m.u32(UI_NOW).ok();
        let start = m.u32(FADE_START).ok();
        let elapsed = now.zip(start).map(|(n, s)| n.wrapping_sub(s));
        let tick = at.boundary == Boundary::Tick && at.phase != Phase::Loading;
        let menu = tick && at.phase == Phase::Menu;
        let fade = start
            .zip(elapsed)
            .filter(|&(_, ms)| menu && ms < FADE_END_MS);
        let stuck = self.held(at.ticks.saturating_sub(at.map_ticks), fade, at.ticks);
        let mut issues = Vec::new();
        if stuck {
            issues.push(json!({"kind":"stuck_fade"}));
        }
        let mut layers = Vec::new();
        let mut complete = tick && now.is_some();
        if let Some(clock) = now.filter(|_| tick) {
            for slot in 0..=SHARED {
                for window in 0..=5 {
                    let Some(ch) = channel(window, slot) else {
                        continue;
                    };
                    // 0x234f8c composites all three, even after the outgoing
                    // screen has lost focus and disappeared from ui.screen.
                    for (offset, role) in [(8, "active"), (12, "pending"), (48, "outgoing")] {
                        match m.u32(ch + offset) {
                            Ok(0) => {}
                            Ok(screen) => {
                                let Some(layer) = layer(m, screen, window, slot, role, clock)
                                else {
                                    complete = false;
                                    continue;
                                };
                                if layer.overdue {
                                    let kind = if layer.backdrop == Some(true) {
                                        "stuck_blur"
                                    } else {
                                        "stuck_transition"
                                    };
                                    issues.push(json!({"kind":kind, "screen":layer.id,
                                        "window":window, "slot":slot, "role":role}));
                                }
                                complete &= layer.backdrop.is_some();
                                layers.push(json!({
                                    "window":window, "slot":slot, "role":role, "id":layer.id,
                                    "direction":layer.direction, "elapsed_ms":layer.elapsed,
                                    "duration_ms":layer.duration, "overdue":layer.overdue,
                                    "requests_backdrop":layer.backdrop
                                }));
                            }
                            Err(_) => complete = false,
                        }
                    }
                }
            }
        }
        (
            json!({
                "ui_clock_ms":now, "startup_fade_remaining_ms":elapsed.filter(|_| menu)
                    .map(|ms| FADE_END_MS.saturating_sub(ms)),
                "scene_fade_mode":m.u32(SCENE_MODE).ok(),
                "scene_fade_weight":m.f32(SCENE_WEIGHT).ok().filter(|v| v.is_finite()),
                "dialog_open":has_dialog, "layers":layers, "layers_complete":complete
            }),
            issues,
        )
    }
}

struct Layer {
    id: u32,
    backdrop: Option<bool>,
    direction: Option<&'static str>,
    elapsed: Option<u32>,
    duration: Option<u32>,
    overdue: bool,
}

fn layer(
    m: &dyn GuestMemory,
    screen: u32,
    window: u32,
    slot: u32,
    role: &str,
    now: u32,
) -> Option<Layer> {
    // Reading the fields below requires a complete screen widget.
    screen.checked_add(0x5f4)?;
    if m.u32(screen + 4).ok()? != 0 {
        return None;
    }
    let id = m.u32(screen + 0x70).ok()?;
    let flags = m.u8(screen + 0x42).ok()?;
    let direction = match flags & 3 {
        1 => Some("in"),
        2 => Some("out"),
        _ => None,
    };
    let mut elapsed = None;
    let mut duration = None;
    let mut overdue = false;
    if direction.is_some() {
        let age = now.wrapping_sub(m.u32(screen + 0x4c).ok()?);
        let delay = m.u32(screen + 0x44).ok()?;
        let length = m.u32(screen + 0x54).ok()?;
        elapsed = Some(age);
        duration = Some(delay.saturating_add(length));
        // Pending objects can still be waiting for their initialization.
        overdue = role != "pending" && past_deadline(age, delay, length);
    }
    Some(Layer {
        id,
        direction,
        elapsed,
        duration,
        overdue,
        backdrop: backdrop(m, screen, window, slot),
    })
}

fn past_deadline(age: u32, delay: u32, length: u32) -> bool {
    age < i32::MAX as u32 && age > delay.saturating_add(length).saturating_add(GRACE_MS)
}

// 0x23029a consults three virtual methods. Decode the known retail accessors
// only. An unfamiliar class gets null rather than a guessed blur result.
fn backdrop(m: &dyn GuestMemory, screen: u32, window: u32, slot: u32) -> Option<bool> {
    // 0x235055 suppresses the convolution in campaign and multiplayer.
    if window == 5 || m.u8(UI_GAME_KIND).ok()? != MAIN_MENU_KIND {
        return Some(false);
    }
    let vt = m.u32(screen).ok()?;
    vt.checked_add(108)?;
    if m.u32(vt + 80).ok()? != 0x0023_012c
        || m.u32(vt + 84).ok()? != 0x0023_0130
        || m.u32(vt + 108).ok()? != 0x0014_73c9
        || m.u32(screen + 0x74).ok()? != window
        || m.u32(screen + 0x78).ok()? != slot
    {
        return None;
    }
    if m.u8(screen + 0x5f4).ok()? != 0 {
        return Some(false);
    }
    for lower in window + 1..=5 {
        if let Some(ch) = channel(lower, slot)
            && (m.u32(ch + 8).ok()? != 0 || m.u32(ch + 12).ok()? != 0)
        {
            return Some(true);
        }
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_ticks_advancing_fades_and_new_maps_break_a_stall() {
        let mut history = History::default();
        let sample = Some((80, 120));
        assert!(!history.held(42, sample, 200));
        assert!(!history.held(42, sample, 200));
        assert!(history.held(42, sample, 350));
        assert!(!history.held(42, Some((80, 121)), 351));
        assert!(!history.held(52, Some((80, 121)), 501));
        assert!(!history.held(52, None, 551));
        assert!(!history.held(52, sample, 750));
    }

    #[test]
    fn a_modal_can_hold_its_blur_until_the_user_closes_it() {
        use guest_layout::memory::FlatMemory;

        let mut m = FlatMemory::new(0x0054_0000, 0x10000);
        let screen = 0x0054_1000;
        let vt = 0x0054_2000;
        m.set_u8(UI_GAME_KIND, MAIN_MENU_KIND).unwrap();
        m.set_u32(screen, vt).unwrap();
        for (offset, function) in [(80, 0x0023_012c), (84, 0x0023_0130), (108, 0x0014_73c9)] {
            m.set_u32(vt + offset, function).unwrap();
        }
        m.set_u32(screen + 0x74, 1).unwrap();
        m.set_u32(screen + 0x78, SHARED).unwrap();
        m.set_u32(channel(5, SHARED).unwrap() + 8, 0x0054_3000)
            .unwrap();
        let idle = layer(&m, screen, 1, SHARED, "active", 8000).unwrap();
        assert_eq!(idle.backdrop, Some(true));
        assert!(!idle.overdue);

        m.set_u8(screen + 0x42, 2).unwrap();
        m.set_u32(screen + 0x4c, 7500).unwrap();
        m.set_u32(screen + 0x54, 500).unwrap();
        assert!(
            !layer(&m, screen, 1, SHARED, "outgoing", 8000)
                .unwrap()
                .overdue
        );
        assert!(
            layer(&m, screen, 1, SHARED, "outgoing", 9500)
                .unwrap()
                .overdue
        );
    }

    #[test]
    fn an_animation_gets_its_authored_delay_duration_and_grace() {
        assert!(!past_deadline(1400, 100, 300));
        assert!(past_deadline(1401, 100, 300));
        assert!(!past_deadline(u32::MAX - 10, 100, 300));
    }
}
