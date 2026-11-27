//! The weather plates of the open view (`0x00037a60`, called per block
//! element by the weather pass `0x00036b50` at `0x00036eca`), as
//! [`frame_scene::WeatherPlate`] effect items.
//!
//! The pass takes the view's visible clusters (the list at `0x00547f94`),
//! each cluster's weather palette index (`+0x76` of its `0xb0`-byte record
//! in the scenario table at `[0x004e0348] + 0xa0`), and for each palette
//! entry in use (`[0x004e0348] + 0x88`, `0x88` bytes each, tag `+0x24`) the
//! `weat` tag's block of `0x3a8`-byte elements (`+8` count, `+0xc` array).
//! Each element is one plate; its scroll and tint are the player state at
//! `element + 0xb8 * player` (`player` is `[0x00485600]`).
//!
//! What is not here: whether the guest draws a plate at all depends on its
//! placement test `0x0003a5c0`, and a plate samples the view's colour
//! target. Both are native-render's: a plate it cannot place or sample is
//! `scene_dropped_weather_plate` (`native_render::weather_scene`).

use frame_scene::effect::{EffectItem, WeatherPlate};
use frame_scene::{BuildError, SceneBuilder};

use crate::GuestRead;

/// The scenario's weather tables: palette count `+0x84`, palette `+0x88`,
/// cluster records `+0xa0`.
pub const SCENARIO: u32 = 0x004e_0348;
/// The tag instance table: 16-byte entries, the tag's data at `+8`.
pub const TAGS: u32 = 0x004e_3b44;
/// The visible-cluster list (`+4` count, `+0xc` values).
pub const CLUSTERS: u32 = 0x0054_7f94;
/// The view's player and window shorts.
pub const PLAYER: u32 = 0x0048_5600;
pub const WINDOW_TOP_LEFT: u32 = 0x0048_5648;
pub const WINDOW_BOTTOM_RIGHT: u32 = 0x0048_564c;

const PALETTE_STRIDE: u32 = 0x88;
const CLUSTER_STRIDE: u32 = 0xb0;
const ELEMENT_STRIDE: u32 = 0x3a8;
const PLAYER_STRIDE: u32 = 0xb8;
/// The guest's cap on weather systems in use.
const MAX_SYSTEMS: u32 = 8;
const MAX_ELEMENTS: u32 = 64;

fn clamp01(x: f32) -> f32 {
    if x < 0.0 { 0.0 } else if x > 1.0 { 1.0 } else { x }
}

fn plate(mem: &dyn GuestRead, at: u32, index: u16, player: u8, window: [i16; 4]) -> Option<WeatherPlate> {
    let f = |off: u32| mem.read_f32(at.checked_add(off)?);
    let state = at.checked_add(PLAYER_STRIDE * u32::from(player))?;
    let s = |off: u32| mem.read_f32(state.checked_add(off)?);
    let layer_scale = [f(0x48)?, f(0x4c)?, f(0x50)?];
    let mut scroll = [[0.0; 2]; 3];
    for (i, row) in scroll.iter_mut().enumerate() {
        let i = i as u32;
        *row = [s(0x11c + 8 * i)? * layer_scale[i as usize], s(0x120 + 8 * i)? * layer_scale[i as usize]];
    }
    let gain = s(0x148)? * s(0x14c)?;
    Some(WeatherPlate {
        element: index,
        player,
        window,
        bitmaps: [mem.read_u32(at + 4)?, mem.read_u32(at + 0xc)?, mem.read_u32(at + 0x14)?],
        layer_scale,
        scroll,
        extent: [f(0x80)?, f(0x84)?],
        size: [s(0x134)?, s(0x138)?, s(0x13c)?],
        tint: [clamp01(s(0xec)? * gain), clamp01(s(0xf0)? * gain), clamp01(s(0xf4)? * gain)],
        fill: [clamp01(f(0x98)?), clamp01(f(0x9c)?), clamp01(f(0xa0)?)],
    })
}

/// The plates of the open view, in the guest's order: palette entries in
/// use ascending, each tag's elements in order. Returns how many were put in
/// the view, and how many elements did not read. `None` when the view's
/// tables do not read at all.
pub fn extract(mem: &dyn GuestRead, builder: &mut SceneBuilder) -> Result<Option<(u32, u32)>, BuildError> {
    let (Some(scenario), Some(list)) = (mem.read_u32(SCENARIO).filter(|&s| s != 0), mem.read_u32(CLUSTERS).filter(|&l| l != 0)) else {
        return Ok(None);
    };
    let (Some(palette_count), Some(palette), Some(clusters), Some(count), Some(values)) = (
        mem.read_u32(scenario + 0x84),
        mem.read_u32(scenario + 0x88),
        mem.read_u32(scenario + 0xa0),
        mem.read_u32(list + 4).map(|c| c & 0xffff),
        mem.read_u32(list + 0xc),
    ) else {
        return Ok(None);
    };
    let (Some(player), Some(top_left), Some(bottom_right)) = (
        mem.read_u32(PLAYER).map(|p| p as u8),
        mem.read_u32(WINDOW_TOP_LEFT),
        mem.read_u32(WINDOW_BOTTOM_RIGHT),
    ) else {
        return Ok(None);
    };
    // Left and right are the high halves.
    let window = [(top_left >> 16) as i16, top_left as i16, (bottom_right >> 16) as i16, bottom_right as i16];
    let mut used = [false; MAX_SYSTEMS as usize];
    for n in 0..count {
        let Some(cluster) = mem.read_u32(values + 4 * n) else { continue };
        let Some(w) = mem.read_u32(clusters + cluster * CLUSTER_STRIDE + 0x74).map(|w| (w >> 16) as u16) else { continue };
        let tag = mem.read_u32(palette + u32::from(w) * PALETTE_STRIDE + 0x24);
        if w != 0xffff && u32::from(w) < palette_count.min(MAX_SYSTEMS) && tag.is_some_and(|t| t != u32::MAX) {
            used[usize::from(w)] = true;
        }
    }
    let (mut put, mut unread) = (0, 0);
    for (system, _) in used.iter().enumerate().filter(|(_, u)| **u) {
        let id = mem.read_u32(palette + system as u32 * PALETTE_STRIDE + 0x24).unwrap_or(u32::MAX);
        let data = mem
            .read_u32(TAGS)
            .and_then(|t| mem.read_u32(t + (id & 0xffff) * 16 + 8));
        let (Some(data), true) = (data, id != u32::MAX) else {
            unread += 1;
            continue;
        };
        let (Some(n), Some(elements)) = (mem.read_u32(data + 8), mem.read_u32(data + 0xc)) else {
            unread += 1;
            continue;
        };
        for e in 0..n.min(MAX_ELEMENTS) {
            match plate(mem, elements + e * ELEMENT_STRIDE, e as u16, player, window) {
                Some(p) => {
                    builder.push_effect(EffectItem::WeatherPlate(p))?;
                    put += 1;
                }
                None => unread += 1,
            }
        }
    }
    Ok(Some((put, unread)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Mem;
    use frame_scene::{SceneId, Stamp, ViewKey, ViewKind};

    const SC: u32 = 0x0200_0000;
    const PAL: u32 = 0x0201_0000;
    const CL: u32 = 0x0202_0000;
    const LIST: u32 = 0x0203_0000;
    const VALS: u32 = 0x0203_0100;
    const TAGS_AT: u32 = 0x0204_0000;
    const WEAT: u32 = 0x0205_0000;
    const ELEMENTS: u32 = 0x0206_0000;

    /// One visible cluster (2) in weather system 1, whose tag has two elements.
    fn world(m: &mut Mem, player: u32) {
        m.w(SCENARIO, SC);
        m.w(SC + 0x84, 2);
        m.w(SC + 0x88, PAL);
        m.w(SC + 0xa0, CL);
        m.w(CLUSTERS, LIST);
        m.w(LIST + 4, 1);
        m.w(LIST + 0xc, VALS);
        m.w(VALS, 2);
        m.w(CL + 2 * CLUSTER_STRIDE + 0x74, 1 << 16);
        m.w(PAL + PALETTE_STRIDE + 0x24, 0xe700_0003);
        m.w(PAL + 0x24, u32::MAX);
        m.w(TAGS, TAGS_AT);
        m.w(TAGS_AT + 3 * 16 + 8, WEAT);
        m.w(WEAT + 8, 2);
        m.w(WEAT + 0xc, ELEMENTS);
        m.w(PLAYER, player);
        // Window: left 10, top 20, right 650, bottom 500.
        m.w(WINDOW_TOP_LEFT, 10 << 16 | 20);
        m.w(WINDOW_BOTTOM_RIGHT, 650 << 16 | 500);
        for e in 0..2 {
            let at = ELEMENTS + e * ELEMENT_STRIDE;
            m.w(at + 4, 0xe600_0000 + e);
            m.w(at + 0xc, 0xe600_0010);
            m.w(at + 0x14, 0xe600_0020);
            for i in 0..3 {
                m.f(at + 0x48 + 4 * i, 2.0);
            }
            m.f(at + 0x80, 1.5);
            m.f(at + 0x84, 2.5);
            for p in 0..4 {
                let s = at + PLAYER_STRIDE * p;
                for i in 0..3 {
                    m.f(s + 0x11c + 8 * i, (p * 10 + i) as f32);
                    m.f(s + 0x120 + 8 * i, (p * 10 + i) as f32 + 0.5);
                    m.f(s + 0x134 + 4 * i, 3.0);
                }
                m.f(s + 0xec, 2.0);
                m.f(s + 0xf0, 0.5);
                m.f(s + 0xf4, -1.0);
                m.f(s + 0x148, 0.5);
                m.f(s + 0x14c, 1.0);
            }
        }
    }

    fn plates(player: u32) -> Vec<WeatherPlate> {
        let mut m = Mem::default();
        world(&mut m, player);
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(
            ViewKey { kind: ViewKind::Game, player: player as u8, view: 0 },
            frame_scene::RenderCamera {
                position: [0.0; 3],
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                fov: 1.0,
                projection_xy: None,
                viewport: [0, 0, 480, 640],
                near: 0.1,
                far: 100.0,
            },
        )
        .unwrap();
        assert_eq!(extract(&m, &mut b).unwrap(), Some((2, 0)));
        let s = b.seal(Stamp::default()).unwrap();
        s.views[0].effects.plates().cloned().collect()
    }

    #[test]
    fn a_plate_scrolls_per_player() {
        let (p0, p2) = (plates(0), plates(2));
        assert_eq!(p0.iter().map(|p| p.element).collect::<Vec<_>>(), [0, 1]);
        // Offsets are the player state's, times the layer's scale.
        assert_eq!(p0[0].scroll, [[0.0, 1.0], [2.0, 3.0], [4.0, 5.0]]);
        assert_eq!(p2[0].scroll, [[40.0, 41.0], [42.0, 43.0], [44.0, 45.0]]);
        assert_eq!(p2[1].player, 2);
        // The sheet's colour: state x gain, clamped to [0, 1].
        assert_eq!(p0[0].tint, [1.0, 0.25, 0.0]);
        assert_eq!((p0[0].window, p0[0].extent), ([10, 20, 650, 500], [1.5, 2.5]));
        assert_eq!(p0[1].bitmaps, [0xe600_0001, 0xe600_0010, 0xe600_0020]);
    }
}
