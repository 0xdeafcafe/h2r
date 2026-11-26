//! The traced draw order of one game view as data (docs/graphics/framescene.md,
//! section 4.2.1; native-only design section 2). `render_scene_passes`
//! `0x0002bcd0` is straight-line, so its call sites give the order. A slot is a
//! place in that order; what draws in it is the pass graph's business.

use std::cmp::Ordering;

use serde::Serialize;

/// A place in the draw order: the thirteen per-view slots, then the
/// per-frame overlay. `Ord` is draw order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum ViewSlot {
    /// Slot 1: opaque base, layers 1 (with the clear), 3, 5, 8 and 20.
    OpaqueBase,
    /// Slot 2: decals mode 0, call site `0x0002c370`.
    DecalsMode0,
    /// Slot 3: object shadows `0x000335e0`, then layer 7, call site `0x0002c021`.
    ShadowsAndLayer7,
    /// Slot 4: light accumulation, per light layers 10, 11, 14, 12, 13,
    /// call site `0x0002c03e`.
    LightAccumulation,
    /// Slot 5: decals mode 1, return address `0x0002c04d`.
    DecalsMode1,
    /// Slot 6: layer 2, call site `0x0002c05b`.
    Layer2,
    /// Slot 7: decals modes 2 to 6, in mode order.
    DecalsLate(u8),
    /// Slot 8, not kind 2: sky `0x0003ede0`; fog `0x00025d50` and layer 16;
    /// water `0x0003ba90` and layer 6 (or `0x0003c3c0`); `0x00025dc0`, `0x0002c340`.
    Late,
    /// Slot 9, all kinds: transparents and effects `0x000c3e90`, `0x00036b50`,
    /// layer 15, `0x001595d0`, contrails `0x00042b20`, sorted `0x0001e370`;
    /// call site `0x0002c19a`.
    Transparents,
    /// Slot 10: first person `0x0003fdf0`(0), its lights `0x000404b0`,
    /// `0x0003fdf0`(1), its sorted pass `0x00040870`; call sites `0x0002c2b3`
    /// and `0x0002c2b9`.
    FirstPerson,
    /// Slot 11: flare query `0x0002dd30`, which tests the final depth; call
    /// site `0x0002c2e4`.
    FlareQuery,
    /// Slot 12: layer 17 (bloom in S; colour gate).
    Layer17,
    /// Slot 13: post `0x00013870`: colour glow `0x000214f0`, screen effects
    /// `0x0002d140`, flare draw `0x0002df00` and glow `0x00046ea0`; call sites
    /// `0x0002c322` and `0x00013b7e`.
    Post,
    /// Snapshots and layers 18, 19 and 21. **Unplaced**: the slot is unknown
    /// until research item R-a, so it sorts after slot 13, before the overlay.
    Snapshot,
    /// Overlay: the HUD, `0x001917eb`.
    Hud,
    /// Overlay: the fade, `0x00040890` (a `Fill`).
    Fade,
    /// Overlay: the UI, `0x00147af1`.
    Ui,
}

impl ViewSlot {
    /// The slot a decal mode draws in: mode 0 in slot 2, mode 1 in slot 5,
    /// modes 2 to 6 in slot 7. `None` for a mode the guest does not have.
    pub fn decal(mode: u8) -> Option<Self> {
        match mode {
            0 => Some(Self::DecalsMode0),
            1 => Some(Self::DecalsMode1),
            2..=6 => Some(Self::DecalsLate(mode)),
            _ => None,
        }
    }

    /// The slot's number, 1 to 13, or `None` for the unplaced snapshot
    /// slot and the overlay.
    pub fn number(self) -> Option<u8> {
        Some(match self {
            Self::OpaqueBase => 1,
            Self::DecalsMode0 => 2,
            Self::ShadowsAndLayer7 => 3,
            Self::LightAccumulation => 4,
            Self::DecalsMode1 => 5,
            Self::Layer2 => 6,
            Self::DecalsLate(_) => 7,
            Self::Late => 8,
            Self::Transparents => 9,
            Self::FirstPerson => 10,
            Self::FlareQuery => 11,
            Self::Layer17 => 12,
            Self::Post => 13,
            Self::Snapshot | Self::Hud | Self::Fade | Self::Ui => return None,
        })
    }

    /// Whether the slot's place in the order is known. False only for
    /// [`ViewSlot::Snapshot`] (research item R-a).
    pub fn placed(self) -> bool {
        self != Self::Snapshot
    }

    /// Whether the slot is in the per-frame overlay, after every view.
    pub fn is_overlay(self) -> bool {
        matches!(self, Self::Hud | Self::Fade | Self::Ui)
    }

    fn key(self) -> (u8, u8) {
        match self {
            Self::DecalsLate(mode) => (7, mode),
            Self::Snapshot => (14, 0),
            Self::Hud => (15, 0),
            Self::Fade => (16, 0),
            Self::Ui => (17, 0),
            other => (other.number().unwrap_or(0), 0),
        }
    }
}

impl PartialOrd for ViewSlot {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ViewSlot {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_slots_follow_the_traced_order() {
        let order = [
            ViewSlot::OpaqueBase,
            ViewSlot::DecalsMode0,
            ViewSlot::ShadowsAndLayer7,
            ViewSlot::LightAccumulation,
            ViewSlot::DecalsMode1,
            ViewSlot::Layer2,
            ViewSlot::DecalsLate(2),
            ViewSlot::Late,
            ViewSlot::Transparents,
            ViewSlot::FirstPerson,
            ViewSlot::FlareQuery,
            ViewSlot::Layer17,
            ViewSlot::Post,
            ViewSlot::Snapshot,
        ];
        for (i, pair) in order.windows(2).enumerate() {
            assert!(pair[0] < pair[1], "{:?} before {:?}", pair[0], pair[1]);
            assert_eq!(pair[0].number(), Some(i as u8 + 1));
        }
        assert_eq!(ViewSlot::Post.number(), Some(13));
        assert!(!ViewSlot::Snapshot.placed());
        assert!(ViewSlot::Post.placed());
        assert!(ViewSlot::DecalsLate(2) < ViewSlot::DecalsLate(6));
    }

    #[test]
    fn every_decal_mode_has_a_slot() {
        let slot = |m| ViewSlot::decal(m).and_then(ViewSlot::number);
        assert_eq!(slot(0), Some(2));
        assert_eq!(slot(1), Some(5));
        for mode in 2..=6 {
            assert_eq!(slot(mode), Some(7), "mode {mode}");
        }
        assert_eq!(ViewSlot::decal(7), None);
    }

    #[test]
    fn the_overlay_draws_hud_then_fade_then_ui() {
        assert!(ViewSlot::Post < ViewSlot::Hud);
        assert!(ViewSlot::Snapshot < ViewSlot::Hud);
        assert!(ViewSlot::Hud < ViewSlot::Fade);
        assert!(ViewSlot::Fade < ViewSlot::Ui);
        assert!(ViewSlot::Hud.is_overlay() && !ViewSlot::Post.is_overlay());
    }
}
