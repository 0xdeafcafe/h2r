//! The scene's weather plates as draw plans (`0x00037a60`): the plate's
//! corners, layers, tint and the state it draws with. A plate is a
//! view-sized sheet, depth tested GREATER against the scene's depth
//! (it draws only where the scene is further than the sheet's depth word,
//! so it falls behind nearer geometry) and blended `ONE / INVSRCALPHA`
//! over the scene colour, in slot 9 ([`WeatherPlate::SLOT`]).
//!
//! A plate samples the view's colour target, and the guest places it with
//! the test `0x0003a5c0`. A host without either draws no plate, and each
//! undrawn plate is counted `scene_dropped_weather_plate` under its cause
//! ([`Dropped`]); the guest's plate draw is never the answer.

use std::sync::atomic::{AtomicU64, Ordering};

use frame_scene::effect::WeatherPlate;

/// The depth comparison of a plate: `GREATER`, no depth write.
pub const DEPTH_GREATER: bool = true;

/// One layer of a plate: its bitmap and its scroll in texture space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layer {
    pub bitmap: u32,
    pub scroll: [f32; 2],
}

/// What a host draws for one plate.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The sheet's four corners in view pixels, half a pixel in from the
    /// window's edges as the guest sets them: top left, top right, bottom
    /// right, bottom left.
    pub corners: [[f32; 2]; 4],
    pub layers: [Layer; 3],
    pub tint: [f32; 3],
    /// Depth test GREATER against the scene depth.
    pub depth_greater: bool,
}

/// The plan of `plate`.
pub fn plan(plate: &WeatherPlate) -> Plan {
    let [left, top, right, bottom] = plate.window.map(|v| f32::from(v) - 0.5);
    Plan {
        corners: [[left, top], [right, top], [right, bottom], [left, bottom]],
        layers: std::array::from_fn(|i| Layer { bitmap: plate.bitmaps[i], scroll: plate.scroll[i] }),
        tint: plate.tint,
        depth_greater: DEPTH_GREATER,
    }
}

/// Why a plate is not drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// The placement test `0x0003a5c0` is not native: the plate's sub-rect
    /// and fade are unknown.
    Placement,
    /// No snapshot of the view's colour for the plate to sample.
    Snapshot,
    /// A bitmap of its layers is not resident.
    Bitmap,
    /// The device could not take the pass in slot 9.
    Reservation,
}

impl Dropped {
    pub const ALL: [Dropped; 4] = [Dropped::Placement, Dropped::Snapshot, Dropped::Bitmap, Dropped::Reservation];

    pub fn name(self) -> &'static str {
        match self {
            Dropped::Placement => "scene_dropped_weather_plate_placement",
            Dropped::Snapshot => "scene_dropped_weather_plate_snapshot",
            Dropped::Bitmap => "scene_dropped_weather_plate_bitmap",
            Dropped::Reservation => "scene_dropped_weather_plate_reservation",
        }
    }
}

static DROPS: [AtomicU64; Dropped::ALL.len()] = [const { AtomicU64::new(0) }; Dropped::ALL.len()];
static DROPPED: AtomicU64 = AtomicU64::new(0);

/// Counts `n` undrawn plates under `cause` and in `scene_dropped_weather_plate`.
pub fn drop_plates(cause: Dropped, n: u64) {
    DROPS[cause as usize].fetch_add(n, Ordering::Relaxed);
    DROPPED.fetch_add(n, Ordering::Relaxed);
}

/// The plate counts: the total, then each cause.
pub fn counters() -> impl Iterator<Item = (&'static str, u64)> {
    std::iter::once(("scene_dropped_weather_plate", DROPPED.load(Ordering::Relaxed)))
        .chain(Dropped::ALL.into_iter().map(|c| (c.name(), DROPS[c as usize].load(Ordering::Relaxed))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plate(player: u8, scroll_x: f32) -> WeatherPlate {
        WeatherPlate {
            element: 0,
            player,
            window: [10, 20, 650, 500],
            bitmaps: [0xe600_0001, 0xe600_0002, 0xe600_0003],
            layer_scale: [1.0; 3],
            scroll: [[scroll_x, 0.5]; 3],
            extent: [1.0, 1.0],
            size: [1.0; 3],
            tint: [1.0, 0.5, 0.25],
        }
    }

    #[test]
    fn a_weather_plate_tests_against_scene_depth() {
        let p = plan(&plate(0, 0.0));
        assert!(p.depth_greater);
        assert_eq!(p.corners, [[9.5, 19.5], [649.5, 19.5], [649.5, 499.5], [9.5, 499.5]]);
        assert_eq!(p.layers.map(|l| l.bitmap), [0xe600_0001, 0xe600_0002, 0xe600_0003]);
        assert_eq!(p.tint, [1.0, 0.5, 0.25]);
        assert_eq!(WeatherPlate::SLOT.number(), Some(9));
    }

    #[test]
    fn a_plate_scrolls_per_player() {
        let (a, b) = (plan(&plate(0, 0.25)), plan(&plate(1, 8.0)));
        assert_eq!(a.layers[2].scroll, [0.25, 0.5]);
        assert_eq!(b.layers[0].scroll, [8.0, 0.5]);
        assert_ne!(a.layers, b.layers);
    }

    #[test]
    fn an_undrawn_plate_counts_scene_dropped_weather_plate() {
        let total = |name: &str| counters().find(|c| c.0 == name).unwrap().1;
        let (all, snap) = (total("scene_dropped_weather_plate"), total("scene_dropped_weather_plate_snapshot"));
        drop_plates(Dropped::Snapshot, 2);
        drop_plates(Dropped::Placement, 1);
        assert_eq!(total("scene_dropped_weather_plate"), all + 3);
        assert_eq!(total("scene_dropped_weather_plate_snapshot"), snap + 2);
    }
}
