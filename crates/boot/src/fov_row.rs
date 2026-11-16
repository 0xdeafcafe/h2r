//! The "Field of View" row on our Video screen (`crate::video_screen`, opened
//! from Halo 2's Settings, Controls screen), and the camera hook that
//! applies it. See
//! docs/settings-menu.md "Field of view" for the addresses, the evidence
//! and why this stays out of the simulation.
//!
//! **The row** is one of the Video screen's rows: the Controls row text
//! function, on the Video screen's list, fills our label and the value (the
//! degrees, as our own strings), and the row-chosen handler steps the value
//! and returns without running the original.
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
//! The hooks are registered by `layout_glue::install` (the row shares the
//! list constructor, the row text function, the row-chosen handler and the
//! string lookup with the other rows); the logic is here, over [`Gm`].

use crate::fov_setting::{self, FovSetting};
use crate::layout_glue::{Gm, ID_BASE};
use crate::speaker_row;

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

/// Our label and value strings, in `layout_glue::is_ours`'s range.
pub const LABEL_ID: u32 = ID_BASE + 0x21;
pub const LABEL_TEXT: &str = "Field of View";
const VALUE_ID_BASE: u32 = ID_BASE + 0x40;

/// The string id that shows `degrees`.
pub fn value_id(degrees: u32) -> u32 {
    let d = degrees.clamp(fov_setting::ANY_MIN, fov_setting::ANY_MAX);
    VALUE_ID_BASE + (d - fov_setting::ANY_MIN)
}

/// Every string this row needs: the label and each value it can show.
pub fn strings() -> Vec<(u32, String)> {
    let mut v = vec![(LABEL_ID, LABEL_TEXT.to_string())];
    v.extend((fov_setting::ANY_MIN..=fov_setting::ANY_MAX).map(|d| (value_id(d), d.to_string())));
    v
}

/// The texts to set for the Controls row text call with the stack at `esp`:
/// `(text widget index, string id)` for the label and the value, or `None`
/// for any row but ours.
pub fn row_texts<M: Gm>(m: &M, esp: u32, row: i16, shown: u32) -> Option<[(u32, u32); 2]> {
    let item = m.u32(esp.wrapping_add(4))?;
    (speaker_row::item_row(m, item)? == row).then_some([
        (speaker_row::LABEL_TEXT_INDEX, LABEL_ID),
        (speaker_row::VALUE_TEXT_INDEX, value_id(shown)),
    ])
}

/// A row-chosen call for `chosen` (read with `layout_glue::chosen_row`):
/// for our row, step the value and return true (the original must not
/// run); false otherwise.
pub fn chosen(chosen: Option<i16>, row: i16, setting: &FovSetting) -> bool {
    if chosen != Some(row) {
        return false;
    }
    setting.step();
    true
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
    use crate::layout_glue::{chosen_row, is_ours};
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

    const ESP: u32 = 0x1000;
    const ITEM: u32 = 0x2000;
    const ROW_PTR: u32 = 0x3000;
    const ROW: i16 = 7;

    fn text_call(row: i16) -> Fake {
        let mut f = Fake::default();
        f.set_u32(ESP + 4, ITEM);
        f.set_u16(ITEM + 0x70, row as u16);
        f
    }

    fn chosen_call(row: i16) -> Fake {
        let mut f = Fake::default();
        f.set_u32(ESP + 4, 0xdead_0000);
        f.set_u32(ESP + 8, ROW_PTR);
        f.set_u16(ROW_PTR, row as u16);
        f
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
    fn the_row_shows_our_label_and_the_degrees() {
        let f = text_call(ROW);
        assert_eq!(
            row_texts(&f, ESP, ROW, 70),
            Some([(0, LABEL_ID), (1, value_id(70))])
        );
        assert_eq!(
            row_texts(&f, ESP, ROW, 95),
            Some([(0, LABEL_ID), (1, value_id(95))])
        );
        let all = strings();
        let text = |id| all.iter().find(|(i, _)| *i == id).map(|(_, t)| t.as_str());
        assert_eq!(text(LABEL_ID), Some("Field of View"));
        assert_eq!(text(value_id(70)), Some("70"));
        assert_eq!(text(value_id(120)), Some("120"));
        assert_eq!(text(value_id(93)), Some("93"));
        assert_eq!(text(value_id(150)), Some("150"));
        // Every id is ours, distinct, and clear of the other rows' ids.
        let mut ids: Vec<u32> = all.iter().map(|(i, _)| *i).collect();
        assert!(ids.iter().all(|i| is_ours(*i)));
        ids.extend(
            crate::layout_glue::served_strings(true, true)
                .iter()
                .map(|(i, _)| *i),
        );
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }

    #[test]
    fn vanilla_rows_and_the_speaker_row_go_elsewhere() {
        for r in [0i16, 1, 2, 3, 4, 5, 6, 8, -1] {
            assert_eq!(row_texts(&text_call(r), ESP, ROW, 90), None, "{r}");
            let s = FovSetting::new(None, None);
            assert!(!chosen(chosen_row(&chosen_call(r), ESP), ROW, &s), "{r}");
            assert_eq!(s.shown(), 70, "row {r} must not step");
        }
        assert_eq!(row_texts(&Fake::default(), ESP, ROW, 90), None);
        assert!(!chosen(
            chosen_row(&Fake::default(), ESP),
            ROW,
            &FovSetting::new(None, None)
        ));
    }

    #[test]
    fn choosing_the_row_steps_and_the_text_follows() {
        let s = FovSetting::new(None, None);
        let press = chosen_call(ROW);
        let text = text_call(ROW);
        let value = |s: &FovSetting| row_texts(&text, ESP, ROW, s.shown()).unwrap()[1].1;
        assert_eq!(value(&s), value_id(70));
        assert!(chosen(chosen_row(&press, ESP), ROW, &s));
        assert_eq!(value(&s), value_id(75));
        for _ in 0..9 {
            chosen(chosen_row(&press, ESP), ROW, &s);
        }
        assert_eq!(value(&s), value_id(120));
        assert!(chosen(chosen_row(&press, ESP), ROW, &s));
        assert_eq!(value(&s), value_id(70));
    }

    #[test]
    fn a_locked_row_is_handled_but_changes_nothing() {
        let s = FovSetting::new(Some(100), None);
        assert!(chosen(chosen_row(&chosen_call(ROW), ESP), ROW, &s));
        assert_eq!(s.shown(), 100);
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
        // 70 (or 0) from the setting is retail.
        assert_eq!(FovSetting::new(Some(70), None).target(), None);
        assert_eq!(FovSetting::new(Some(0), None).target(), None);
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
