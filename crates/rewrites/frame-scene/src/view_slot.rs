//! The traced draw order of one game view as data (docs/graphics/framescene.md,
//! section 4.2.1; native-only design section 2). `render_scene_passes`
//! `0x0002bcd0` is straight-line, so its call sites give the order. A slot is a
//! place in that order; what draws in it is the pass graph's business.

use std::cmp::Ordering;

use frame_types::carried::Slot;
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
    /// call site `0x0002c19a`. The sorted list's camouflage and hologram
    /// entries draw layers 18, 19 and 21 here, each after its view snapshot
    /// ([`ViewSlot::sorted_layer`]).
    Transparents,
    /// Slot 10: first person `0x0003fdf0`(0), its lights `0x000404b0`,
    /// `0x0003fdf0`(1), its sorted pass `0x00040870`; call sites `0x0002c2b3`
    /// and `0x0002c2b9`. Its sorted pass draws its own layers 18, 19 and 21.
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
    /// Overlay: the HUD, `0x001917eb`.
    Hud,
    /// Overlay: the fade, `0x00040890` (a `Fill`).
    Fade,
    /// Overlay: the UI, `0x00147af1`.
    Ui,
}

impl ViewSlot {
    /// The pass graph's passes in draw order, one per slot: slots 1 to 13
    /// (slot 7 as `DecalsLate(2)`, its modes drawing in it in mode order),
    /// then the overlay's three.
    pub const PASSES: &[ViewSlot; 16] = &[
        Self::OpaqueBase,
        Self::DecalsMode0,
        Self::ShadowsAndLayer7,
        Self::LightAccumulation,
        Self::DecalsMode1,
        Self::Layer2,
        Self::DecalsLate(2),
        Self::Late,
        Self::Transparents,
        Self::FirstPerson,
        Self::FlareQuery,
        Self::Layer17,
        Self::Post,
        Self::Hud,
        Self::Fade,
        Self::Ui,
    ];

    /// The pass this slot draws in ([`Self::PASSES`]): itself, and slot 7's
    /// decal modes all in one.
    pub fn pass(self) -> Self {
        match self {
            Self::DecalsLate(_) => Self::DecalsLate(2),
            other => other,
        }
    }

    /// The passes a carried slot spans, in order. The frame step moves the
    /// carried slot on five entries (frame-types `SlotTracker`), so each
    /// spans the traced slots between them: the view's entry to the light
    /// pass is slots 1 to 3, the light pass 4, its exit to the sorted
    /// transparents 5 to 9 (layer 15 and the late passes draw before
    /// `0x0001e370`), the sorted transparents to the post's exit 9 to 13,
    /// after the post 13 (the flare draw is late in it). Outside a game
    /// view there is none.
    pub fn carried(slot: Slot) -> &'static [ViewSlot] {
        match slot {
            Slot::Opaque => &Self::PASSES[0..3],
            Slot::AfterOpaque => &Self::PASSES[3..4],
            Slot::AfterLights => &Self::PASSES[4..9],
            Slot::Transparents => &Self::PASSES[8..13],
            Slot::AfterPost => &Self::PASSES[12..13],
            Slot::Overlay => &Self::PASSES[13..16],
            Slot::Outside => &[],
        }
    }

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

    /// The slot a sorted handler's layer 18, 19 or 21 draws in: the sorted
    /// list `0x0001e370` runs in slot 9, and again for the first person
    /// (`0x00040870`) in slot 10 (docs/graphics/notes/2026-11-27-snapshot-slot.md).
    /// `None` for any other layer.
    pub fn sorted_layer(layer: u8, first_person: bool) -> Option<Self> {
        match layer {
            18 | 19 | 21 if first_person => Some(Self::FirstPerson),
            18 | 19 | 21 => Some(Self::Transparents),
            _ => None,
        }
    }

    /// The slot's number, 1 to 13, or `None` for the overlay.
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
            Self::Hud | Self::Fade | Self::Ui => return None,
        })
    }

    /// Whether the slot is in the per-frame overlay, after every view.
    pub fn is_overlay(self) -> bool {
        matches!(self, Self::Hud | Self::Fade | Self::Ui)
    }

    fn key(self) -> (u8, u8) {
        match self {
            Self::DecalsLate(mode) => (7, mode),
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
        ];
        for (i, pair) in order.windows(2).enumerate() {
            assert!(pair[0] < pair[1], "{:?} before {:?}", pair[0], pair[1]);
            assert_eq!(pair[0].number(), Some(i as u8 + 1));
        }
        assert_eq!(ViewSlot::Post.number(), Some(13));
        assert!(ViewSlot::DecalsLate(2) < ViewSlot::DecalsLate(6));
    }

    #[test]
    fn each_carried_slot_spans_its_traced_slots() {
        let numbers = |slot| ViewSlot::carried(slot).iter().map(|s| s.number()).collect::<Vec<_>>();
        assert_eq!(numbers(Slot::Opaque), [Some(1), Some(2), Some(3)]);
        assert_eq!(numbers(Slot::AfterOpaque), [Some(4)]);
        assert_eq!(numbers(Slot::AfterLights), [Some(5), Some(6), Some(7), Some(8), Some(9)]);
        assert_eq!(numbers(Slot::Transparents), [Some(9), Some(10), Some(11), Some(12), Some(13)]);
        assert_eq!(numbers(Slot::AfterPost), [Some(13)]);
        assert_eq!(ViewSlot::carried(Slot::Overlay), [ViewSlot::Hud, ViewSlot::Fade, ViewSlot::Ui]);
        assert!(ViewSlot::carried(Slot::Outside).is_empty());
        // Every pass is in some game view's or the overlay's span, in order.
        let spanned: Vec<ViewSlot> = Slot::ALL.iter().flat_map(|s| ViewSlot::carried(*s).iter().copied()).collect();
        assert!(ViewSlot::PASSES.iter().all(|p| spanned.contains(p)));
        assert!(ViewSlot::PASSES.windows(2).all(|w| w[0] < w[1]));
        for mode in 2..=6 {
            assert_eq!(ViewSlot::DecalsLate(mode).pass(), ViewSlot::PASSES[6], "mode {mode} draws in slot 7's pass");
        }
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
    fn the_snapshot_layers_draw_in_their_sorted_lists_slot() {
        for layer in [18, 19, 21] {
            assert_eq!(ViewSlot::sorted_layer(layer, false), Some(ViewSlot::Transparents));
            assert_eq!(ViewSlot::sorted_layer(layer, true), Some(ViewSlot::FirstPerson));
        }
        assert_eq!(ViewSlot::sorted_layer(17, false), None, "layer 17 is slot 12's");
    }

    #[test]
    fn the_overlay_draws_hud_then_fade_then_ui() {
        assert!(ViewSlot::Post < ViewSlot::Hud);
        assert!(ViewSlot::Hud < ViewSlot::Fade);
        assert!(ViewSlot::Fade < ViewSlot::Ui);
        assert!(ViewSlot::Hud.is_overlay() && !ViewSlot::Post.is_overlay());
    }
}
