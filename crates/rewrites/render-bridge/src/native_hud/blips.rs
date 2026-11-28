//! The motion sensor's blips with no guest body (docs/graphics/native-hud-overlay.md,
//! "The blips"): a blip draw `0x00254490` whose state setup would change
//! nothing on the device is drawn by the host as the body's quad
//! (`engine::hud_sensor::blip_draw`) and returns without its body.
//!
//! Between two blip calls of one `HUD_SENSOR_DRAW` the guest makes no
//! D3D call (`0x001a2823`'s loop and `0x001a2197` are arithmetic, Code
//! `h2 disasm 0x001a2823 100`), so after a blip whose body set the state
//! the device holds what the next blip's setup sets, but for the stage
//! textures and the vertex program record, which [`engine::hud_sensor::blip_state`]
//! reads. The first blip of a call runs the body: it follows the sensor's
//! target setup `0x00254200`.
//!
//! A blip the guest draws is one `scene_kept_overlay_sensor_blip_<cause>`
//! ([`kept`]): `first`, `bitmap`, `unread`, `texture`, `shader`, `state`,
//! or the host's cause. A blip drawn here is `..._packet` (the device's
//! pushed draw, [`kept::carried`]) and one
//! `native_ui_overlay_hud_walk_sensor_blip_returned`. Placed unless
//! `impl.render.native-hud-blips=off`.

use std::cell::Cell;

use engine::hud_sensor::{self, BlipDraw, BlipRefusal};
use guest_layout::memory::GuestMemory;

use super::kept::{self, OverlayKind};
use super::{HudVertices, walk};

thread_local! {
    /// A blip of the open sensor call ran its body to its quad.
    static SET: Cell<bool> = const { Cell::new(false) };
}

/// `HUD_SENSOR_DRAW`'s entry or return: the next blip runs its body.
pub fn edge() {
    SET.with(|s| s.set(false));
}

/// What a blip call does.
#[derive(Debug, PartialEq, Eq)]
pub enum Turn {
    /// Run the guest body.
    Guest,
    /// The host drew the quad and the body's writes are made: return,
    /// MXCSR as the body leaves it.
    Return { mxcsr: u32 },
}

fn keep(cause: &'static str) -> Turn {
    kept::begin(OverlayKind::SensorBlip);
    kept::refuse(cause);
    kept::end(false);
    Turn::Guest
}

/// The blip call whose stack is at `esp`; `draw` is the host's
/// ([`super::Host::sensor_blip`]).
pub fn turn(
    mem: &mut dyn GuestMemory,
    esp: u32,
    mxcsr: u32,
    draw: impl FnOnce(&mut dyn GuestMemory, &HudVertices) -> bool,
) -> Turn {
    if mem.u32(esp).ok() != Some(super::sensor::BLIP_RETURN) {
        edge();
        return Turn::Guest;
    }
    let d: BlipDraw = match hud_sensor::blip_draw(&*mem, esp, mxcsr) {
        Ok(d) => d,
        Err(why) => {
            // The body may or may not reach the device's state.
            edge();
            return keep(match why {
                BlipRefusal::Bitmap => "bitmap",
                BlipRefusal::Unread => "unread",
            });
        }
    };
    if !d.draws {
        // The body stops before its state and its quad.
        return Turn::Guest;
    }
    if !SET.with(|s| s.replace(true)) {
        return keep("first");
    }
    match hud_sensor::blip_state(&*mem, &d) {
        Ok(None) => {}
        Ok(Some(cause)) => return keep(cause),
        Err(_) => return keep("unread"),
    }
    kept::begin(OverlayKind::SensorBlip);
    let drawn = draw(mem, &d.vertices);
    kept::end(drawn);
    if !drawn {
        return Turn::Guest;
    }
    if hud_sensor::blip_writes(mem, &d).is_err() {
        // Nothing the host drew can be taken back: count it, as the
        // body's writes are what the next blip compares against.
        walk::count("sensor_blip_writes");
        edge();
    }
    walk::count("sensor_blip_returned");
    Turn::Return { mxcsr: d.mxcsr }
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::hud_sensor::*;
    use guest_layout::testing::FakeGuest;

    const ESP: u32 = 0x0070_0000;

    /// The kept counters are shared: serialised with `kept`'s tests.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        kept::tests::SERIAL.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// One resident blip bitmap pair, a call's stack at `ESP` from
    /// `0x001a2197`, stage 0 holding the first bitmap's texture.
    fn image() -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_u32(HUD_GLOBALS, 0x0073_0000).put_u32(0x0073_0110, 1).put_u32(0x0073_0114, 0x0071_0000);
        m.put_u32(0x0071_0064, 2).put_u32(0x0071_006c, 3);
        m.put_u32(TAG_INSTANCES, 0x0074_0000).put_u32(CACHE_STAMP, 100);
        for (index, definition, bitmap, texture) in [(2u32, 0x0075_0000u32, 0x0072_0000u32, 0x7100u32), (3, 0x0075_1000, 0x0072_1000, 0x7200)] {
            m.put_u32(0x0074_0000 + 16 * index + 8, definition);
            m.put_u32(definition + 0x44, 1).put_u32(definition + 0x48, bitmap);
            m.put_u32(bitmap + 0x70, 101).put_u32(bitmap + 0x50, texture).put_u32(bitmap + 4, 0x0008_0010);
        }
        m.put_u8(BLIPS_DRAW, 1).put_f32(0x0045_dbc0, 1.0).put_f32(0x0045_dc0c, 0.5).put_f32(0x0045_dd40, 1.0);
        m.put_u32(ESP, super::super::sensor::BLIP_RETURN).put_u32(ESP + 4, 0x0076_0000);
        m.put_f32(ESP + 8, 1.0).put_f32(ESP + 12, 2.0).put_u32(ESP + 16, 0x0077_0000).put_u32(ESP + 20, 0);
        m.put_f32(0x0076_0000, 4.0).put_f32(0x0076_0004, 6.0);
        m.put_f32(0x0077_0000, 1.0).put_f32(0x0077_0004, 1.0).put_f32(0x0077_0008, 1.0);
        m.put_u32(TEXTURES, 0x7100).put_u32(TEXTURES_BOUND, 0x7100);
        m.put_u32(CULL_MODE, 0).put_u32(Z_BIAS, 0).put_u32(BITMAP_WIDTH, 0).put_u32(BITMAP_HEIGHT, 0);
        m
    }

    fn kept_count(cause: &str) -> u64 {
        let name = format!("scene_kept_overlay_sensor_blip_{cause}");
        kept::counters().into_iter().find(|(k, _)| *k == name).map_or(0, |(_, n)| n)
    }

    #[test]
    fn after_the_first_blip_of_a_call_each_blip_is_the_hosts_quad_and_returns() {
        let _serial = serial();
        let mut m = image();
        edge();
        let first = kept_count("first");
        let mut drawn = Vec::new();
        assert_eq!(turn(&mut m, ESP, 0x1f80, |_, _| unreachable!()), Turn::Guest);
        assert_eq!(kept_count("first"), first + 1);
        for _ in 0..3 {
            let turned = turn(&mut m, ESP, 0x1f80, |_, v| {
                drawn.push(*v);
                true
            });
            assert_eq!(turned, Turn::Return { mxcsr: 0x1f80 });
        }
        assert_eq!(drawn.len(), 3);
        assert_eq!(drawn[0][0][2], (0, [3.0, 7.0, 1.0, 1.0]));
        assert_eq!(m.u32(BITMAP_WIDTH).unwrap(), 16);
        // The call's return: the next call's first blip is the guest's.
        edge();
        assert_eq!(turn(&mut m, ESP, 0x1f80, |_, _| unreachable!()), Turn::Guest);
    }

    #[test]
    fn a_blip_whose_setup_would_change_the_device_runs_its_body() {
        let _serial = serial();
        let mut m = image();
        edge();
        turn(&mut m, ESP, 0x1f80, |_, _| unreachable!());
        // A pulsing blip binds the second bitmap.
        m.put_u32(ESP + 20, 1);
        let before = kept_count("texture");
        assert_eq!(turn(&mut m, ESP, 0x1f80, |_, _| unreachable!()), Turn::Guest);
        assert_eq!(kept_count("texture"), before + 1);
        // Its body bound it: the next pulsing blip is the host's.
        m.put_u32(TEXTURES_BOUND, 0x7200).put_u32(TEXTURES, 0x7200);
        assert!(matches!(turn(&mut m, ESP, 0x1f80, |_, _| true), Turn::Return { .. }));
        // A host that refuses leaves it to the body.
        assert_eq!(turn(&mut m, ESP, 0x1f80, |_, _| false), Turn::Guest);
    }

    #[test]
    fn a_blip_rust_cannot_read_or_another_caller_runs_its_body_and_the_next_is_first() {
        let _serial = serial();
        let mut m = image();
        edge();
        turn(&mut m, ESP, 0x1f80, |_, _| unreachable!());
        m.put_u32(0x0072_0000 + 0x70, 100);
        let before = kept_count("bitmap");
        assert_eq!(turn(&mut m, ESP, 0x1f80, |_, _| unreachable!()), Turn::Guest);
        assert_eq!(kept_count("bitmap"), before + 1);
        m.put_u32(0x0072_0000 + 0x70, 101);
        let first = kept_count("first");
        assert_eq!(turn(&mut m, ESP, 0x1f80, |_, _| unreachable!()), Turn::Guest);
        assert_eq!(kept_count("first"), first + 1);
        // Not from `0x001a2197`.
        m.put_u32(ESP, 0x1234);
        assert_eq!(turn(&mut m, ESP, 0x1f80, |_, _| unreachable!()), Turn::Guest);
        m.put_u32(ESP, super::super::sensor::BLIP_RETURN);
        assert_eq!(turn(&mut m, ESP, 0x1f80, |_, _| unreachable!()), Turn::Guest);
    }

    #[test]
    fn a_held_blip_flag_draws_nothing_and_keeps_nothing() {
        let _serial = serial();
        let mut m = image();
        edge();
        m.put_u8(BLIPS_DRAW, 0);
        let first = kept_count("first");
        assert_eq!(turn(&mut m, ESP, 0x1f80, |_, _| unreachable!()), Turn::Guest);
        assert_eq!(kept_count("first"), first);
    }
}
