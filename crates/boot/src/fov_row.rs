//! The "Field of View" row on our Video screen (`crate::video_screen`, opened
//! from Halo 2's Settings, Controls screen), and the camera hook that
//! applies it. See
//! docs/settings-menu.md "Field of view" for the addresses, the evidence
//! and why this stays out of the simulation.
//!
//! **The row** is one of the Video screen's rows, drawn and pressed by the
//! `ui` model from the `graphics.fov` setting (`crate::setting_defs::FOV`).
//!
//! **The camera.** `0x187130` is the game's per-local-player field of view
//! (eax = the player-control index, result in xmm0, plain `ret`): the
//! observer default when the player has no unit, else the unit tag's camera
//! field of view divided by the zoom magnification of the unit's current
//! zoom level. Its one caller is the first-person camera update `0x23c110`,
//! which clamps the result to `[0x55e484, 0x55e488]` (1 and 110 degrees)
//! and stores it as the director camera's horizontal field of view. The
//! observer turns that into the vertical field of view and the scale the
//! renderer, its culling and the particle code read (`0x171d90`,
//! `0x2f800`). We run the original, then replace the unzoomed value with
//! the setting and keep the zoom's ratio: `result * target / unzoomed`. The
//! upper clamp is raised to the setting when the setting is above it, and
//! put back otherwise.
//!
//! The camera hook is registered by `layout_glue::install`; the logic is
//! here, over [`Gm`].

use crate::layout_glue::Gm;

/// `player_control_get_field_of_view`: our camera hook.
pub const PLAYER_FOV: u32 = 0x0018_7130;
/// The first-person camera's upper clamp on that value (a dynamically
/// initialised global; its only reader is `0x23c225`).
pub const FOV_CLAMP_MAX: u32 = 0x0055_e488;
/// The read-only constant that clamp is initialised from (`0x3782c0`).
pub const FOV_CLAMP_MAX_RETAIL: u32 = 0x0045_e098;

/// The player-control array pointer, its stride and the unit index in it.
const PLAYER_CONTROL: u32 = 0x004e_d284;
const PLAYER_CONTROL_STRIDE: u32 = 0x94;
const PLAYER_CONTROL_UNIT: u32 = 0x14;
/// The object header table pointer, its data at `+0x44`, 12-byte headers
/// with the object at `+8`; the object's tag index at `+0`.
const OBJECT_HEADERS: u32 = 0x004e_0300;
/// The tag instance table pointer: 16-byte entries, definition at `+8`.
const TAG_INSTANCES: u32 = 0x004e_3b44;
/// The unit definition's camera field of view (radians).
const UNIT_CAMERA_FOV: u32 = 0xcc;

/// The camera's unzoomed field of view in radians for `degrees` (the
/// `graphics.fov` setting), or `None` for retail (70): the hook then
/// changes nothing.
pub fn target(degrees: u32) -> Option<f32> {
    (degrees != crate::setting_defs::FOV_RETAIL as u32).then(|| (degrees as f32).to_radians())
}

fn f32_at<M: Gm>(m: &M, a: u32) -> Option<f32> {
    m.u32(a).map(f32::from_bits)
}

/// The unzoomed field of view `0x187130` starts from for player-control
/// index `user`: the unit tag's camera field of view, read the way the
/// function reads it. `None` when there is no unit or anything is
/// unreadable or implausible (the hook then treats the result as unzoomed).
pub fn unzoomed_fov<M: Gm>(m: &M, user: u32) -> Option<f32> {
    if user >= 4 {
        return None;
    }
    let pc = m
        .u32(PLAYER_CONTROL)?
        .wrapping_add(user * PLAYER_CONTROL_STRIDE);
    let unit = m.u32(pc.wrapping_add(PLAYER_CONTROL_UNIT))?;
    if unit == u32::MAX {
        return None;
    }
    let headers = m.u32(m.u32(OBJECT_HEADERS)?.wrapping_add(0x44))?;
    let object = m.u32(headers.wrapping_add((unit & 0xffff) * 12 + 8))?;
    let tag = m.u32(object)? & 0xffff;
    let def = m.u32(m.u32(TAG_INSTANCES)?.wrapping_add(tag * 16 + 8))?;
    let fov = f32_at(m, def.wrapping_add(UNIT_CAMERA_FOV))?;
    (fov.is_finite() && fov > 0.0 && fov < std::f32::consts::PI).then_some(fov)
}

/// The field of view to hand back for the game's `result`: unchanged with
/// no target (retail, bit for bit); else the target times the zoom ratio
/// `result / unzoomed` (`unzoomed` defaults to `result`, so an unzoomed or
/// unit-less camera gets the target itself).
pub fn camera_fov(result: f32, unzoomed: Option<f32>, target: Option<f32>) -> f32 {
    let Some(target) = target else { return result };
    let base = unzoomed.unwrap_or(result);
    if !(base.is_finite() && base > 0.0 && result.is_finite()) {
        return result;
    }
    result * (target / base)
}

/// The first-person camera's upper clamp for `target`: retail's, or the
/// target when it is wider.
pub fn clamp_max(retail: f32, target: Option<f32>) -> f32 {
    target.map_or(retail, |t| t.max(retail))
}

/// After the original `0x187130` returned `result` for player `user`: the
/// value to return, with the clamp brought in line. Retail with the clamp
/// already retail changes nothing.
pub fn apply<M: Gm>(m: &mut M, user: u32, result: f32, target: Option<f32>) -> f32 {
    if let Some(retail) = f32_at(m, FOV_CLAMP_MAX_RETAIL) {
        let want = clamp_max(retail, target);
        if f32_at(m, FOV_CLAMP_MAX) != Some(want) {
            m.set_u32(FOV_CLAMP_MAX, want.to_bits());
        }
    }
    if target.is_none() {
        return result;
    }
    camera_fov(result, unzoomed_fov(m, user), target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake(BTreeMap<u32, u8>);
    impl Gm for Fake {
        fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
            for (i, b) in out.iter_mut().enumerate() {
                match self.0.get(&(addr + i as u32)) {
                    Some(v) => *b = *v,
                    None => return false,
                }
            }
            true
        }
        fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
            for (i, b) in data.iter().enumerate() {
                self.0.insert(addr + i as u32, *b);
            }
            true
        }
    }

    fn deg(d: f32) -> f32 {
        d.to_radians()
    }

    /// Guest memory with player `user`'s unit whose tag says `fov`, and the
    /// retail clamp.
    fn world(user: u32, fov: f32) -> Fake {
        let mut f = Fake::default();
        f.set_u32(PLAYER_CONTROL, 0x10_0000);
        f.set_u32(
            0x10_0000 + user * PLAYER_CONTROL_STRIDE + PLAYER_CONTROL_UNIT,
            0xe123_0005,
        );
        f.set_u32(OBJECT_HEADERS, 0x20_0000);
        f.set_u32(0x20_0044, 0x30_0000);
        f.set_u32(0x30_0000 + 5 * 12 + 8, 0x40_0000);
        f.set_u32(0x40_0000, 0xf00d_0009);
        f.set_u32(TAG_INSTANCES, 0x50_0000);
        f.set_u32(0x50_0000 + 9 * 16 + 8, 0x60_0000);
        f.set_u32(0x60_0000 + UNIT_CAMERA_FOV, fov.to_bits());
        f.set_u32(FOV_CLAMP_MAX_RETAIL, deg(110.0).to_bits());
        f.set_u32(FOV_CLAMP_MAX, deg(110.0).to_bits());
        f
    }

    #[test]
    fn retail_returns_the_games_value_bit_for_bit() {
        for v in [deg(70.0), deg(35.0), deg(8.75), 0.0, f32::NAN, 1.0e-30] {
            assert_eq!(camera_fov(v, Some(deg(70.0)), None).to_bits(), v.to_bits());
            let mut f = world(0, deg(70.0));
            let before = f.0.clone();
            assert_eq!(apply(&mut f, 0, v, None).to_bits(), v.to_bits());
            assert_eq!(f.0, before, "retail writes nothing");
        }
        // 70 from the setting (0 in the file or H2_FOV reads as 70) is retail.
        assert_eq!(target(70), None);
        assert_eq!(target(75), Some(deg(75.0)));
    }

    #[test]
    fn the_unzoomed_camera_gets_the_target() {
        let mut f = world(1, deg(70.0));
        let out = apply(&mut f, 1, deg(70.0), Some(deg(90.0)));
        assert!((out - deg(90.0)).abs() < 1e-6, "{}", out.to_degrees());
        // A unit whose tag says 78: unzoomed is still the target.
        let mut f = world(0, deg(78.0));
        let out = apply(&mut f, 0, deg(78.0), Some(deg(100.0)));
        assert!((out - deg(100.0)).abs() < 1e-6);
        // No unit (or unreadable): the result is the unzoomed value.
        let mut f = Fake::default();
        let out = apply(&mut f, 0, deg(70.0), Some(deg(110.0)));
        assert!((out - deg(110.0)).abs() < 1e-6);
    }

    #[test]
    fn zoom_keeps_its_magnification() {
        // Retail: 70 degrees, 2x and 8x scopes give 35 and 8.75.
        for (zoom, retail) in [(2.0f32, 35.0f32), (8.0, 8.75)] {
            let mut f = world(0, deg(70.0));
            let out = apply(&mut f, 0, deg(retail), Some(deg(100.0)));
            assert!(
                (out - deg(100.0 / zoom)).abs() < 1e-6,
                "{zoom}x: {}",
                out.to_degrees()
            );
            // The ratio unzoomed / zoomed is the magnification, as in retail.
            assert!((deg(100.0) / out - zoom).abs() < 1e-4);
        }
    }

    #[test]
    fn the_clamp_opens_for_wide_values_and_closes_again() {
        let mut f = world(0, deg(70.0));
        let clamp = |f: &Fake| f32::from_bits(f.u32(FOV_CLAMP_MAX).unwrap());
        apply(&mut f, 0, deg(70.0), Some(deg(100.0)));
        assert_eq!(clamp(&f), deg(110.0), "100 fits under retail's 110");
        apply(&mut f, 0, deg(70.0), Some(deg(120.0)));
        assert_eq!(clamp(&f), deg(120.0));
        apply(&mut f, 0, deg(70.0), None);
        assert_eq!(clamp(&f), deg(110.0), "back to retail");
        assert_eq!(clamp_max(deg(110.0), Some(deg(150.0))), deg(150.0));
        assert_eq!(clamp_max(deg(110.0), None), deg(110.0));
    }

    #[test]
    fn the_unit_chain_is_read_like_the_game_reads_it() {
        let f = world(2, deg(78.0));
        assert_eq!(unzoomed_fov(&f, 2), Some(deg(78.0)));
        assert_eq!(unzoomed_fov(&f, 1), None, "player 1 has no data");
        assert_eq!(unzoomed_fov(&f, 4), None);
        let mut f = world(0, deg(78.0));
        f.set_u32(0x10_0000 + PLAYER_CONTROL_UNIT, u32::MAX);
        assert_eq!(unzoomed_fov(&f, 0), None, "no unit");
        let f = world(0, 4.0);
        assert_eq!(unzoomed_fov(&f, 0), None, "implausible tag value");
    }
}
