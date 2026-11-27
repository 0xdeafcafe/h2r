//! The scene's decals as draw groups (docs/graphics/native-decals-research.md,
//! "Effective render state"): which slot each draws in, and the depth and
//! blend state of each mode, as the guest's state wrapper `0x00043a40`
//! sets them. The groups are what a host records on the device in slots 2,
//! 5 and 7; this module holds no GPU object and reads no guest memory.
//!
//! An item a host cannot draw is a named `scene_dropped_decal_<cause>`
//! ([`Dropped`]); the guest's decal draw is never the answer.

use std::sync::atomic::{AtomicU64, Ordering};

use frame_scene::decal::DecalItem;
use frame_scene::{FrameScene, View, ViewSlot};

use crate::decal::Mode;

/// Depth state of a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    /// W-buffered, `ZFUNC` ALWAYS, z write on, with the solid polygon
    /// offset (the constant at `[0x0045e48c]`): modes 0 to 5.
    AlwaysWithOffset,
    /// Depth off, no offset: mode 6.
    Off,
}

/// Blend state of a mode (`SRCBLEND` / `DESTBLEND` and the op).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    /// `ONE / INVSRCALPHA`, add: modes 0, 1 and 5.
    Over,
    /// `DESTCOLOR / SRCCOLOR`, add: mode 2.
    Multiply,
    /// `DESTCOLOR / ZERO`, add: mode 3.
    Darken,
    /// The max op (the factors do not apply): mode 4.
    Maximum,
    /// Blending off: mode 6.
    Opaque,
}

/// A mode's state; colour writes are RGB only in every mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct State {
    pub depth: Depth,
    pub blend: Blend,
}

/// The state the wrapper sets for `mode`.
pub fn state(mode: Mode) -> State {
    let (depth, blend) = match mode {
        Mode::Alpha0 | Mode::Alpha1 | Mode::Alpha5 => (Depth::AlwaysWithOffset, Blend::Over),
        Mode::Multiply => (Depth::AlwaysWithOffset, Blend::Multiply),
        Mode::Darken => (Depth::AlwaysWithOffset, Blend::Darken),
        Mode::Maximum => (Depth::AlwaysWithOffset, Blend::Maximum),
        Mode::Opaque => (Depth::Off, Blend::Opaque),
    };
    State { depth, blend }
}

/// The items of one mode of a view, in the guest's order.
#[derive(Debug)]
pub struct Group<'a> {
    pub mode: Mode,
    pub slot: ViewSlot,
    pub items: Vec<&'a DecalItem>,
}

/// The view's decals as groups in draw order: slot 2 (mode 0), slot 5
/// (mode 1), then slot 7's modes 2 to 6. Each group keeps the items'
/// order; a mode without items has no group. An item whose mode is not one
/// of the seven is counted [`Dropped::Mode`].
pub fn groups<'a>(scene: &'a FrameScene, view: &View) -> Vec<Group<'a>> {
    let mut by_mode: [Vec<&DecalItem>; Mode::COUNT] = Default::default();
    for item in scene.decals_of(view) {
        match Mode::from_guest(i16::from(item.mode)) {
            Some(mode) => by_mode[mode.index() as usize].push(item),
            None => Dropped::Mode.count(1),
        }
    }
    let mut out: Vec<Group<'a>> = by_mode
        .into_iter()
        .enumerate()
        .filter(|(_, items)| !items.is_empty())
        .map(|(n, items)| {
            let mode = Mode::from_guest(n as i16).expect("a mode index");
            Group { mode, slot: ViewSlot::decal(n as u8).expect("a decal mode has a slot"), items }
        })
        .collect();
    out.sort_by_key(|g| g.slot);
    out
}

/// Whether the host draws `slot`'s decals: a slot a decal mode maps to.
pub fn is_decal_slot(slot: ViewSlot) -> bool {
    matches!(slot, ViewSlot::DecalsMode0 | ViewSlot::DecalsMode1 | ViewSlot::DecalsLate(_))
}

/// Why a decal is not drawn. Each is a named count, never a D3D draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// The record did not read at extraction (the view's decal remainder).
    Unread,
    /// A mode the guest does not have.
    Mode,
    /// The bitmap is not resident or not a 2D image.
    Bitmap,
    /// The device could not take the pass in its slot.
    Reservation,
    /// The geometry does not fit the draw (not whole quads, empty).
    Geometry,
}

impl Dropped {
    pub const ALL: [Dropped; 5] = [Dropped::Unread, Dropped::Mode, Dropped::Bitmap, Dropped::Reservation, Dropped::Geometry];

    /// The count's name in `render.scene`.
    pub fn name(self) -> &'static str {
        match self {
            Dropped::Unread => "scene_dropped_decal_unread",
            Dropped::Mode => "scene_dropped_decal_mode",
            Dropped::Bitmap => "scene_dropped_decal_bitmap",
            Dropped::Reservation => "scene_dropped_decal_reservation",
            Dropped::Geometry => "scene_dropped_decal_geometry",
        }
    }

    pub fn count(self, n: u64) {
        DROPS[self as usize].fetch_add(n, Ordering::Relaxed);
    }
}

static DROPS: [AtomicU64; Dropped::ALL.len()] = [const { AtomicU64::new(0) }; Dropped::ALL.len()];

/// The decal drop counts, by name.
pub fn counters() -> impl Iterator<Item = (&'static str, u64)> {
    Dropped::ALL.into_iter().map(|c| (c.name(), DROPS[c as usize].load(Ordering::Relaxed)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::decal::{DecalItem, DecalVertex};
    use frame_scene::{RenderCamera, SceneBuilder, SceneId, Stamp, ViewKey, ViewKind};

    fn item(mode: u8, record: u16) -> DecalItem {
        DecalItem { mode, cluster: 0, record, bitmap: 0xe600_0001, image: 0, colour: u32::MAX, vertices: 0..0 }
    }

    fn scene(modes: &[u8]) -> std::sync::Arc<FrameScene> {
        let mut b = SceneBuilder::default();
        b.begin(SceneId::default());
        b.open_view(
            ViewKey { kind: ViewKind::Game, player: 0, view: 0 },
            RenderCamera {
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
        let quad = [DecalVertex { position: [0; 3], packed: 0 }; 4];
        for (n, &mode) in modes.iter().enumerate() {
            b.push_decal(item(mode, n as u16), &quad).unwrap();
        }
        b.seal(Stamp::default()).unwrap()
    }

    #[test]
    fn decal_modes_draw_in_their_three_slots() {
        let s = scene(&[6, 2, 0, 3, 1, 2, 5, 4]);
        let g = groups(&s, &s.views[0]);
        let slots: Vec<_> = g.iter().map(|g| (g.mode.index(), g.slot.number())).collect();
        assert_eq!(slots, [(0, Some(2)), (1, Some(5)), (2, Some(7)), (3, Some(7)), (4, Some(7)), (5, Some(7)), (6, Some(7))]);
        // Items keep the guest's order inside a mode.
        assert_eq!(g[2].items.iter().map(|i| i.record).collect::<Vec<_>>(), [1, 5]);
        assert!(g.iter().all(|g| is_decal_slot(g.slot)));
        // A mode the guest does not have is a named drop.
        let before: u64 = counters().find(|c| c.0 == "scene_dropped_decal_mode").unwrap().1;
        let s = scene(&[9, 0]);
        assert_eq!(groups(&s, &s.views[0]).len(), 1);
        assert!(counters().find(|c| c.0 == "scene_dropped_decal_mode").unwrap().1 > before);
    }

    #[test]
    fn each_mode_has_the_wrappers_state() {
        use Blend::*;
        use Depth::*;
        let got: Vec<_> = (0..7).map(|m| state(Mode::from_guest(m).unwrap())).map(|s| (s.depth, s.blend)).collect();
        assert_eq!(
            got,
            [
                (AlwaysWithOffset, Over),
                (AlwaysWithOffset, Over),
                (AlwaysWithOffset, Multiply),
                (AlwaysWithOffset, Darken),
                (AlwaysWithOffset, Maximum),
                (AlwaysWithOffset, Over),
                (Off, Opaque),
            ]
        );
    }
}
