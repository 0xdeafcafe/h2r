//! Button layouts as symbolic action-to-button tables.
//!
//! Halo 2 turns the physical pad into player actions through a small table of
//! sixteen action slots, each naming the physical button that drives it
//! (`docs/input/controller-layouts.md` has the addresses and how it was read). The
//! player profile holds a layout number and the game selects the table from
//! it. This module is that table in our own words: [`Layout`] says which
//! [`Button`] drives each [`Action`], and [`Layout::action_map`] encodes it as
//! the sixteen-byte slot array the game uses. Nothing here is retail data; the
//! retail layouts are defined by what they change relative to Default, which
//! is public (Halopedia's "H2:Control schemes").
//!
//! Button names are the Xbox ones. The host maps its right bumper to Black and
//! its left bumper to White ([`crate::host`]), so "bumper jumper" puts Jump on
//! White (left bumper) and Melee on Black (right bumper).
//!
//! Sources for the community layouts, which Halo 2 on Xbox does not have:
//! Halo 3 (Bumper Jumper, Walkie Talkie,
//! <https://www.halopedia.org/H3:Control_schemes>) and Halo: Reach (Bumper
//! Jumper, Recon, <https://www.halopedia.org/Reach:Control_schemes>). Halo 2
//! has no armor ability or equipment, so Flashlight takes that role. These
//! are our transplants, not layouts Bungie shipped for this game.

/// A physical Xbox control, numbered as the game numbers them: the eight
/// analog buttons first (`bAnalogButtons` order), then the eight digital bits
/// of `wButtons` (d-pad up, down, left, right, Start, Back, left thumb, right
/// thumb).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Button {
    A = 0,
    B = 1,
    X = 2,
    Y = 3,
    Black = 4,
    White = 5,
    LeftTrigger = 6,
    RightTrigger = 7,
    DpadUp = 8,
    DpadDown = 9,
    DpadLeft = 10,
    DpadRight = 11,
    Start = 12,
    Back = 13,
    LeftThumb = 14,
    RightThumb = 15,
}

impl Button {
    pub const fn index(self) -> u8 {
        self as u8
    }
}

/// A player action, in the order of the game's sixteen slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Action {
    Jump = 0,
    SwitchGrenades = 1,
    ActionReload = 2,
    SwitchWeapon = 3,
    Melee = 4,
    Flashlight = 5,
    /// Throw a grenade, or fire the left weapon when dual wielding.
    Grenade = 6,
    Fire = 7,
    Pause = 8,
    Score = 9,
    Crouch = 10,
    Zoom = 11,
    LeanLeft = 12,
    LeanRight = 13,
    /// Menu confirm and cancel; fixed on A and B in every layout.
    MenuAccept = 14,
    MenuBack = 15,
}

impl Action {
    pub const ALL: [Action; 16] = [
        Action::Jump,
        Action::SwitchGrenades,
        Action::ActionReload,
        Action::SwitchWeapon,
        Action::Melee,
        Action::Flashlight,
        Action::Grenade,
        Action::Fire,
        Action::Pause,
        Action::Score,
        Action::Crouch,
        Action::Zoom,
        Action::LeanLeft,
        Action::LeanRight,
        Action::MenuAccept,
        Action::MenuBack,
    ];

    /// The actions a layout may move. The rest are fixed.
    pub const GAMEPLAY: [Action; 12] = [
        Action::Jump,
        Action::SwitchGrenades,
        Action::ActionReload,
        Action::SwitchWeapon,
        Action::Melee,
        Action::Flashlight,
        Action::Grenade,
        Action::Fire,
        Action::Pause,
        Action::Score,
        Action::Crouch,
        Action::Zoom,
    ];
}

/// A button layout. The first four are Halo 2's own; the profile stores their
/// number (0 to 3). The others are ours and take the next numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Layout {
    Default,
    Southpaw,
    Boxer,
    GreenThumb,
    BumperJumper,
    Recon,
    WalkieTalkie,
}

impl Layout {
    pub const ALL: [Layout; 7] = [
        Layout::Default,
        Layout::Southpaw,
        Layout::Boxer,
        Layout::GreenThumb,
        Layout::BumperJumper,
        Layout::Recon,
        Layout::WalkieTalkie,
    ];

    /// The layouts the retail game offers.
    pub const RETAIL: [Layout; 4] =
        [Layout::Default, Layout::Southpaw, Layout::Boxer, Layout::GreenThumb];

    /// The number stored in the profile's button-layout byte.
    pub const fn preset(self) -> u8 {
        match self {
            Layout::Default => 0,
            Layout::Southpaw => 1,
            Layout::Boxer => 2,
            Layout::GreenThumb => 3,
            Layout::BumperJumper => 4,
            Layout::Recon => 5,
            Layout::WalkieTalkie => 6,
        }
    }

    /// The layout for a stored number. Unknown numbers are `None`; the game
    /// treats them as Default.
    pub fn from_preset(n: u8) -> Option<Layout> {
        Layout::ALL.into_iter().find(|l| l.preset() == n)
    }

    pub const fn is_retail(self) -> bool {
        self.preset() < 4
    }

    /// The physical button that drives `action` in this layout.
    pub fn button(self, action: Action) -> Button {
        use Action::*;
        use Button::*;
        let default = match action {
            Jump => A,
            SwitchGrenades => Black,
            ActionReload => X,
            SwitchWeapon => Y,
            Melee => B,
            Flashlight => White,
            Grenade => LeftTrigger,
            Fire => RightTrigger,
            Pause => Start,
            Score => Back,
            Crouch => LeftThumb,
            Zoom => RightThumb,
            LeanLeft => DpadLeft,
            LeanRight => DpadRight,
            MenuAccept => A,
            MenuBack => B,
        };
        let changed = match (self, action) {
            (Layout::Default, _) => None,
            // Triggers swapped.
            (Layout::Southpaw, Grenade) => Some(RightTrigger),
            (Layout::Southpaw, Fire) => Some(LeftTrigger),
            // Melee on the left trigger, grenade on B.
            (Layout::Boxer, Melee) => Some(LeftTrigger),
            (Layout::Boxer, Grenade) => Some(B),
            // Melee on the right thumbstick click, zoom on B.
            (Layout::GreenThumb, Melee) => Some(RightThumb),
            (Layout::GreenThumb, Zoom) => Some(B),
            // Jump and melee on the bumpers: Jump on White (left bumper),
            // Melee on Black (right bumper); grenade swap, action/reload and
            // flashlight move onto the face buttons.
            (Layout::BumperJumper, Jump) => Some(White),
            (Layout::BumperJumper, Melee) => Some(Black),
            (Layout::BumperJumper, SwitchGrenades) => Some(A),
            (Layout::BumperJumper, ActionReload) => Some(B),
            (Layout::BumperJumper, Flashlight) => Some(X),
            // Action/reload on the right bumper, grenade swap on X.
            (Layout::Recon, ActionReload) => Some(Black),
            (Layout::Recon, SwitchGrenades) => Some(X),
            // Melee on the right bumper, action/reload on B, grenade swap on X.
            (Layout::WalkieTalkie, Melee) => Some(Black),
            (Layout::WalkieTalkie, ActionReload) => Some(B),
            (Layout::WalkieTalkie, SwitchGrenades) => Some(X),
            _ => None,
        };
        changed.unwrap_or(default)
    }

    /// The sixteen slots as the game stores them: slot `i` holds the index of
    /// the physical button that drives action `i`.
    pub fn action_map(self) -> [u8; 16] {
        Action::ALL.map(|a| self.button(a).index())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;
    use Button::*;

    fn changed(l: Layout) -> Vec<(Action, Button)> {
        Action::ALL
            .into_iter()
            .filter(|a| l.button(*a) != Layout::Default.button(*a))
            .map(|a| (a, l.button(a)))
            .collect()
    }

    #[test]
    fn default_is_the_documented_halo_2_layout() {
        // Halopedia H2:Control schemes, Default column.
        let d = Layout::Default;
        assert_eq!(d.button(Jump), A);
        assert_eq!(d.button(Melee), B);
        assert_eq!(d.button(ActionReload), X);
        assert_eq!(d.button(SwitchWeapon), Y);
        assert_eq!(d.button(Fire), RightTrigger);
        assert_eq!(d.button(Grenade), LeftTrigger);
        assert_eq!(d.button(Crouch), LeftThumb);
        assert_eq!(d.button(Zoom), RightThumb);
        assert_eq!(d.button(SwitchGrenades), Black);
        assert_eq!(d.button(Flashlight), White);
        assert_eq!(d.button(Score), Back);
        assert_eq!(d.button(Pause), Start);
    }

    #[test]
    fn retail_layouts_change_only_what_halopedia_says() {
        assert_eq!(changed(Layout::Default), []);
        assert_eq!(
            changed(Layout::Southpaw),
            [(Grenade, RightTrigger), (Fire, LeftTrigger)]
        );
        assert_eq!(changed(Layout::Boxer), [(Melee, LeftTrigger), (Grenade, B)]);
        assert_eq!(
            changed(Layout::GreenThumb),
            [(Melee, RightThumb), (Zoom, B)]
        );
    }

    #[test]
    fn bumper_jumper_puts_jump_on_the_left_bumper_and_melee_on_the_right() {
        let l = Layout::BumperJumper;
        // White is the left bumper, Black the right (crates/input host mapping).
        assert_eq!(l.button(Jump), White);
        assert_eq!(l.button(Melee), Black);
        assert_eq!(l.button(SwitchGrenades), A);
        assert_eq!(l.button(ActionReload), B);
        assert_eq!(l.button(Flashlight), X);
        // Everything else stays put, so triggers and sticks play as before.
        for a in [SwitchWeapon, Grenade, Fire, Crouch, Zoom, Pause, Score] {
            assert_eq!(l.button(a), Layout::Default.button(a), "{a:?}");
        }
    }

    #[test]
    fn recon_and_walkie_talkie_match_their_source_layouts() {
        assert_eq!(
            changed(Layout::Recon),
            [(SwitchGrenades, X), (ActionReload, Black)]
        );
        assert_eq!(
            changed(Layout::WalkieTalkie),
            [(SwitchGrenades, X), (ActionReload, B), (Melee, Black)]
        );
    }

    #[test]
    fn every_layout_gives_each_gameplay_action_its_own_button() {
        for l in Layout::ALL {
            let mut seen = std::collections::HashSet::new();
            for a in Action::GAMEPLAY {
                assert!(seen.insert(l.button(a)), "{l:?}: {a:?} shares a button");
            }
        }
    }

    #[test]
    fn menu_confirm_and_cancel_never_move() {
        for l in Layout::ALL {
            assert_eq!(l.button(MenuAccept), A, "{l:?}");
            assert_eq!(l.button(MenuBack), B, "{l:?}");
        }
    }

    #[test]
    fn action_map_has_one_slot_per_action_in_order() {
        for l in Layout::ALL {
            let m = l.action_map();
            for a in Action::ALL {
                assert_eq!(m[a as usize], l.button(a).index());
            }
        }
        assert_eq!(Layout::BumperJumper.action_map()[Jump as usize], White.index());
    }

    #[test]
    fn preset_numbers_round_trip_and_retail_numbers_do_not_move() {
        for (n, l) in Layout::RETAIL.into_iter().enumerate() {
            assert_eq!(l.preset() as usize, n);
            assert!(l.is_retail());
        }
        for l in Layout::ALL {
            assert_eq!(Layout::from_preset(l.preset()), Some(l));
        }
        assert_eq!(Layout::from_preset(7), None);
        assert_eq!(Layout::from_preset(255), None);
        assert!(!Layout::BumperJumper.is_retail());
    }
}
