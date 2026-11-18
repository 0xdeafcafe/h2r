//! Which HUD a game shows: Halo 2's own, or Halo 1's on a Halo 1 map when
//! the player chose it (the `halo1.hud` setting, on the Halo 1 options
//! screens, `crate::h1_options`).
//!
//! This is the switch, the way `crate::health::set_mode` is the health
//! model's: the choice is kept as the player makes it ([`set_choice`]), and
//! each game start picks the style from the loaded map ([`style_for_map`],
//! the `hud` feature's game-start observer). The Halo 1 HUD itself is
//! another lane's; it reads [`style`] and draws Halo 1's when it says so.
//! Until then nothing reads it, and Halo 2's HUD shows whatever it says.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use feature::{BuildError, Builder, Manifest};

/// A HUD style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudStyle {
    Halo1,
    Halo2,
}

impl HudStyle {
    fn code(self) -> u8 {
        match self {
            HudStyle::Halo1 => 1,
            HudStyle::Halo2 => 2,
        }
    }
    fn from_code(c: u8) -> Option<HudStyle> {
        match c {
            1 => Some(HudStyle::Halo1),
            2 => Some(HudStyle::Halo2),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            HudStyle::Halo1 => "Halo 1",
            HudStyle::Halo2 => "Halo 2",
        }
    }
}

/// Tests that move the switch hold this, so they don't race.
#[cfg(test)]
pub static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The player's choice for Halo 1 maps (`halo1.hud`): Halo 1's by default.
static HALO1_CHOSEN: AtomicBool = AtomicBool::new(true);
/// The style of the game started last; 0 before any.
static ACTIVE: AtomicU8 = AtomicU8::new(0);

/// Records the player's choice for Halo 1 maps. Safe from any thread; it
/// counts from the next game start ([`start`]).
pub fn set_choice(halo1: bool) {
    HALO1_CHOSEN.store(halo1, Ordering::SeqCst);
}

/// The player's choice for Halo 1 maps.
pub fn halo1_chosen() -> bool {
    HALO1_CHOSEN.load(Ordering::SeqCst)
}

/// The style a map shows by its name (the cache header's), given the
/// choice: Halo 1's only on a Halo 1 map (a converted multiplayer map,
/// `h1_<map>`, or one of Halo 1's campaign levels) with Halo 1's chosen.
pub fn style_for_map(name: &str, halo1_chosen: bool) -> HudStyle {
    let name = name.trim().to_ascii_lowercase();
    let halo1_map =
        name.starts_with("h1_") || crate::health::HALO1_LEVELS.contains(&name.as_str());
    if halo1_map && halo1_chosen {
        HudStyle::Halo1
    } else {
        HudStyle::Halo2
    }
}

/// A game is starting on map `name`: its style from now on.
pub fn start(name: &str) -> HudStyle {
    let s = style_for_map(name, halo1_chosen());
    ACTIVE.store(s.code(), Ordering::SeqCst);
    s
}

/// The style of the game in progress (Halo 2's before any game).
pub fn style() -> HudStyle {
    HudStyle::from_code(ACTIVE.load(Ordering::SeqCst)).unwrap_or(HudStyle::Halo2)
}

/// The game-start observer that picks the style.
pub struct Hud;

impl feature::Feature for Hud {
    fn manifest(&self) -> Manifest {
        Manifest::new("hud", "Which HUD a game shows: Halo 1's or Halo 2's")
            .doc("docs/game/h1-multiplayer.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.game().on_start("pick the HUD style", |o| {
            match crate::health::loaded_map_name(&*o.guest()) {
                Ok(name) => {
                    let s = start(&name);
                    eprintln!("hud: {name}: {} HUD", s.name());
                }
                Err(e) => eprintln!("hud: the map's name could not be read ({e:?}); style unchanged"),
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halo1_maps_show_the_chosen_hud_and_the_rest_halo2s() {
        assert_eq!(style_for_map("h1_bloodgulch", true), HudStyle::Halo1);
        assert_eq!(style_for_map("H1_Bloodgulch ", true), HudStyle::Halo1);
        assert_eq!(style_for_map("a30", true), HudStyle::Halo1);
        assert_eq!(style_for_map("h1_bloodgulch", false), HudStyle::Halo2);
        assert_eq!(style_for_map("a30", false), HudStyle::Halo2);
        assert_eq!(style_for_map("lockout", true), HudStyle::Halo2);
        assert_eq!(style_for_map("03a_oldmombasa", true), HudStyle::Halo2);
    }

    #[test]
    fn the_choice_counts_from_the_next_start() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_choice(true);
        assert_eq!(start("a30"), HudStyle::Halo1);
        set_choice(false);
        assert_eq!(style(), HudStyle::Halo1, "the game in progress keeps it");
        assert_eq!(start("a30"), HudStyle::Halo2);
        assert_eq!(style(), HudStyle::Halo2);
        set_choice(true);
    }
}
