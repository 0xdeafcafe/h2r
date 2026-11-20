//! `frame-scene.sink` (docs/graphics/framescene.md, slice 6): the frame
//! step declares the slot of the frame its guest work is in, and the
//! device's `d3d::SceneSink` files each op it sees under it.
//!
//! The slot moves at the guest's own entries (research points, **Code**,
//! docs/graphics/render-loop.md): `render_frame_and_present` `0x00223240`
//! begins and ends a frame outside every game view; `render_game_view`
//! `0x0002b790` opens a game view at `Opaque` with the view index its
//! record holds at `+4`; the light pass `0x00031c80` is `AfterOpaque` and
//! its exit `AfterLights`; the sorted transparents `0x0001e370` begin
//! `Transparents`; `render_screen_effects` `0x00013870`'s exit is
//! `AfterPost`; a game view's exit, the HUD's draw `0x001917eb` and the end
//! of `render_views` `0x0002b5d0` are the `Overlay` of a frame that has had
//! a game view ([`frame_scene::SlotTracker`]).
//!
//! Game code writes its draws into the push buffer, which the device
//! decodes at its next entry, so before the slot moves the device runs
//! what was written under the old one ([`Drain`], registered by the
//! host).
//!
//! An option, off by default: without it [`slot`] answers `None` and the
//! sink records nothing.

use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use engine::points;
use frame_scene::{Slot, SlotAt, SlotEvent, SlotTracker};
use hooks::Builder;

pub const ID: &str = "frame-scene.sink";

/// The view index in a view record (`render_game_view` stores it at
/// `0x004ba050`).
const VIEW_RECORD_INDEX: u32 = 4;

// Frame-end and overlay hooks may run before Swap. Keep a game-view entry
// until the original present, including views without any carried packets.
static GAME_VIEW: AtomicBool = AtomicBool::new(false);

/// Consume the game-view entry observed since the last original Swap.
pub fn take_present_game_view() -> bool {
    GAME_VIEW.swap(false, Ordering::Relaxed)
}

/// [`OFF`], or the current slot as `slot << 8 | view`.
static AT: AtomicU16 = AtomicU16::new(OFF);
const OFF: u16 = u16::MAX;
static TRACKER: Mutex<SlotTracker> = Mutex::new(SlotTracker::new());
static DRAIN: Mutex<Option<Arc<dyn Drain>>> = Mutex::new(None);

/// The device, run up to here: what game code wrote into the push buffer
/// is decoded and handed to the backend under the slot it was written in.
pub trait Drain: Send + Sync {
    fn drain(&self, mem: &mut dyn GuestMemory);
}

/// Registers the device's [`Drain`] (the host's D3D install).
pub fn set_drain(drain: Arc<dyn Drain>) {
    *DRAIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(drain);
}

fn pack(at: SlotAt) -> u16 {
    ((at.slot.index() as u16) << 8) | u16::from(at.view)
}

fn unpack(v: u16) -> Option<SlotAt> {
    if v == OFF {
        return None;
    }
    let slot = *Slot::ALL.get(usize::from(v >> 8))?;
    Some(SlotAt { slot, view: v as u8 })
}

/// Where the frame step is, or `None` without the option: the sink's
/// `d3d::scene_sink::SlotSource`.
pub fn slot() -> Option<SlotAt> {
    unpack(AT.load(Ordering::Relaxed))
}

/// The device drained, then the slot moved, at a guest entry.
fn at_entry(mem: &mut dyn GuestMemory, event: SlotEvent) {
    if AT.load(Ordering::Relaxed) == OFF {
        return;
    }
    let drain = DRAIN.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(d) = drain {
        d.drain(mem);
    }
    on(event);
}

/// Moves the slot (no-op without the option).
pub fn on(event: SlotEvent) {
    if AT.load(Ordering::Relaxed) == OFF {
        return;
    }
    let mut t = TRACKER.lock().unwrap_or_else(|e| e.into_inner());
    if matches!(event, SlotEvent::GameView(_)) { GAME_VIEW.store(true, Ordering::Relaxed); }
    t.on(event);
    AT.store(pack(t.at()), Ordering::Relaxed);
}

/// A snapshot load: the frame starts again outside every view.
pub fn forget() {
    on(SlotEvent::Frame);
}

fn arm() {
    GAME_VIEW.store(false, Ordering::Relaxed);
    *TRACKER.lock().unwrap_or_else(|e| e.into_inner()) = SlotTracker::new();
    AT.store(pack(SlotAt::default()), Ordering::Relaxed);
}

/// The game view `render_game_view` is entered with: its record's index.
fn view_index(mem: &dyn GuestMemory, esp: u32) -> u8 {
    esp.checked_add(4)
        .and_then(|a| mem.u32(a).ok())
        .and_then(|record| mem.u32(record.checked_add(VIEW_RECORD_INDEX)?).ok())
        .unwrap_or(0) as u8
}

/// Places the slot's observers and turns [`slot`] on (the frame step's
/// build, with [`ID`] set).
pub fn install(b: &mut Builder) {
    arm();
    b.raw()
        .observe_before(&points::RENDER_GAME_VIEW, "carried_slot_game_view", |o| {
            let mut g = o.guest();
            let view = view_index(&*g, o.cpu.esp);
            at_entry(&mut *g, SlotEvent::GameView(view));
        })
        .optional();
    for (point, event, label, after) in [
        (&points::RENDER_FRAME, SlotEvent::Frame, "carried_slot_frame", false),
        (&points::RENDER_FRAME, SlotEvent::Frame, "carried_slot_frame_end", true),
        (&points::RENDER_GAME_VIEW, SlotEvent::Overlay, "carried_slot_game_view_end", true),
        (&points::RENDER_DYNAMIC_LIGHTS, SlotEvent::LightsBegin, "carried_slot_lights", false),
        (&points::RENDER_DYNAMIC_LIGHTS, SlotEvent::LightsEnd, "carried_slot_lights_end", true),
        (&points::SCENE_TRANSPARENTS, SlotEvent::Transparents, "carried_slot_transparents", false),
        (&points::RENDER_SCREEN_EFFECTS, SlotEvent::PostEnd, "carried_slot_post_end", true),
        (&points::UI_HUD_DRAW, SlotEvent::Overlay, "carried_slot_hud", false),
        (&points::RENDER_VIEWS, SlotEvent::Overlay, "carried_slot_views_end", true),
    ] {
        if after {
            b.raw().observe_after(point, label, move |o| at_entry(&mut *o.guest(), event)).optional();
        } else {
            b.raw().observe_before(point, label, move |o| at_entry(&mut *o.guest(), event)).optional();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slot_packs_and_unpacks_and_off_is_none() {
        for slot in Slot::ALL {
            for view in [0, 1, 3, 255] {
                let at = SlotAt { slot, view };
                assert_eq!(unpack(pack(at)), Some(at));
            }
        }
        assert_eq!(unpack(OFF), None);
    }
}
