//! The Skulls page's model picture (docs/game/skulls.md, "The model
//! picture"): with the native renderer on, the picture right of the list is
//! Halo 2's own skull model, read from the user's disc at run time, lit,
//! swaying slowly, tinted gold or silver bone, with the focused skull's
//! glyph ([`crate::skull_glyphs`]) pressed into its forehead. A skull not yet
//! collected is a dark silhouette with a faint rim and no glyph.
//!
//! How it joins the page: the list class notes the picture widget and the
//! focused row's [`Look`] at each row text ([`note`], from
//! `list_class::Rows::model_picture`), and the native UI walk asks the host
//! at each widget whether it draws that widget itself
//! (`render_bridge::native_hud::Host::replace_widget`). For the noted
//! picture it queues a pass ([`gpu`]) and the game's own bitmap draw is
//! skipped. Until the model has loaded, without the native renderer, or
//! with the feature off, the game's picture draws as before.
//!
//! Feature `skulls-model`, on by default (`H2_FEATURES=skulls-model=off`).

use std::sync::Mutex;
use std::time::Instant;

use crate::skull_glyphs::Stroke;
use crate::skulls::{ABOUT, SkullTier};

/// The feature id (`H2_FEATURES=skulls-model=off`).
pub const FEATURE: &str = "skulls-model";

/// Whether the model picture is on in `switches`.
pub fn on(switches: &hooks::Switches) -> bool {
    switches.feature(FEATURE, true)
}

/// The bone colours: the base map's brightness times these.
pub const GOLD_BONE: [f32; 3] = [1.0, 0.8, 0.42];
pub const SILVER_BONE: [f32; 3] = [0.82, 0.86, 0.93];

/// What the picture shows for a row: which skull and whether the profile
/// has collected it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// The skull's index (`skulls::ABOUT`).
    pub skull: usize,
    pub collected: bool,
}

impl Look {
    pub fn tier(&self) -> Option<SkullTier> {
        ABOUT.get(self.skull).map(|a| a.tier)
    }

    /// The bone's colour for the skull's tier.
    pub fn tint(&self) -> [f32; 3] {
        match self.tier() {
            Some(SkullTier::Gold) => GOLD_BONE,
            _ => SILVER_BONE,
        }
    }

    /// The glyph on the forehead: none until the skull is collected, so a
    /// locked skull gives nothing away.
    pub fn glyph(&self) -> Option<&'static [Stroke]> {
        if self.collected {
            crate::skull_glyphs::glyph(self.skull)
        } else {
            None
        }
    }

    /// Drawn as a dark shape: not collected.
    pub fn silhouette(&self) -> bool {
        !self.collected
    }
}

/// The picture widget the page showed last and what it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Noted {
    screen: u32,
    widget: u32,
    /// The widget's block (`+0x70`, `list_class::WIDGET_BLOCK`): the
    /// picture's block copy in our class's table page. A later widget at
    /// the same address with another block is not ours.
    block: u32,
    look: Look,
    /// When this page first showed a model: the sway runs from here, so a
    /// focus change does not jerk the turn.
    since: Instant,
}

static NOTED: Mutex<Option<Noted>> = Mutex::new(None);

/// Tests that note a picture hold this: [`NOTED`] is one per process.
#[cfg(test)]
pub(crate) static TEST_SERIAL: Mutex<()> = Mutex::new(());

/// Screen `screen`'s picture widget `widget` (its block `block`) shows
/// `look`; `None` (a row with no skull, an unreadable block) shows the
/// game's own picture again.
pub fn note(screen: u32, widget: u32, block: Option<u32>, look: Option<Look>) {
    let mut noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
    match (block, look) {
        (Some(block), Some(look)) => {
            let since = noted
                .filter(|n| n.screen == screen && n.widget == widget)
                .map_or_else(Instant::now, |n| n.since);
            *noted = Some(Noted {
                screen,
                widget,
                block,
                look,
                since,
            });
        }
        _ => {
            if noted.is_some_and(|n| n.widget == widget) {
                *noted = None;
            }
        }
    }
}

/// Screen `screen` was freed: its picture is gone.
pub fn forget(screen: u32) {
    let mut noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
    if noted.is_some_and(|n| n.screen == screen) {
        *noted = None;
    }
}

/// What widget `widget` (its block now `block`) shows, and the seconds
/// since its page first showed a model; `None` for any widget but the
/// noted picture.
pub fn noted(widget: u32, block: u32) -> Option<(Look, f64)> {
    let noted = NOTED.lock().unwrap_or_else(|e| e.into_inner());
    noted
        .filter(|n| n.widget == widget && n.block == block)
        .map(|n| (n.look, n.since.elapsed().as_secs_f64()))
}

/// A widget's alpha, which the game's bitmap draw multiplies in
/// (`list_class::WIDGET_TINT`'s note: `0x2b0565` reads it at `+0x64`): the
/// page's fade in and out.
pub const WIDGET_ALPHA: u32 = 0x64;

/// The alpha read at `+0x64`, held to 0..1; a word that is not a number
/// draws opaque.
pub fn alpha(word: u32) -> f32 {
    let a = f32::from_bits(word);
    if a.is_finite() {
        a.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// The UI's own size: 640 by 480 units, origin at the centre, y up.
pub const UI_SIZE: (f32, f32) = (640.0, 480.0);

/// `ui_box` (`top, left, bottom, right` in UI units) in pixels of a
/// `guest` sized target: `x, y, width, height` from the top left. The UI's
/// height fills the target's; it is centred across.
pub fn guest_rect(ui_box: [i16; 4], guest: (u32, u32)) -> [f32; 4] {
    let [top, left, bottom, right] = ui_box.map(f32::from);
    let scale = guest.1 as f32 / UI_SIZE.1;
    let (cx, cy) = (guest.0 as f32 / 2.0, guest.1 as f32 / 2.0);
    [
        cx + left * scale,
        cy - top * scale,
        (right - left) * scale,
        (top - bottom) * scale,
    ]
}

#[cfg(feature = "gpu")]
pub mod gpu;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skulls::TOTAL;

    #[test]
    fn a_locked_skull_shows_no_glyph_and_a_collected_one_shows_its_own() {
        for skull in 0..TOTAL {
            let locked = Look {
                skull,
                collected: false,
            };
            assert!(locked.silhouette());
            assert!(
                locked.glyph().is_none(),
                "{} reveals its glyph",
                ABOUT[skull].name
            );
            let open = Look {
                skull,
                collected: true,
            };
            assert!(!open.silhouette());
            assert_eq!(open.glyph(), crate::skull_glyphs::glyph(skull));
            assert!(open.glyph().is_some());
        }
    }

    #[test]
    fn the_tint_follows_the_tier() {
        let gold = ABOUT
            .iter()
            .position(|a| a.tier == SkullTier::Gold)
            .unwrap();
        let silver = ABOUT
            .iter()
            .position(|a| a.tier == SkullTier::Silver)
            .unwrap();
        assert_eq!(
            Look {
                skull: gold,
                collected: true
            }
            .tint(),
            GOLD_BONE
        );
        assert_eq!(
            Look {
                skull: silver,
                collected: false
            }
            .tint(),
            SILVER_BONE
        );
        assert!(GOLD_BONE[0] > GOLD_BONE[2] && SILVER_BONE[2] > SILVER_BONE[0]);
    }

    #[test]
    fn only_the_noted_picture_is_replaced_and_freeing_its_screen_forgets_it() {
        let _guard = TEST_SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let look = Look {
            skull: 12,
            collected: true,
        };
        note(0x7ff0_0100, 0x7ff0_2000, Some(0x7ff0_3000), Some(look));
        assert_eq!(noted(0x7ff0_2000, 0x7ff0_3000).map(|n| n.0), Some(look));
        assert!(noted(0x7ff0_2004, 0x7ff0_3000).is_none(), "another widget");
        assert!(
            noted(0x7ff0_2000, 0x7ff0_3004).is_none(),
            "the same address with another block"
        );
        // A focus change keeps the sway's start.
        let before = noted(0x7ff0_2000, 0x7ff0_3000).unwrap().1;
        let other = Look {
            skull: 4,
            collected: false,
        };
        note(0x7ff0_0100, 0x7ff0_2000, Some(0x7ff0_3000), Some(other));
        let (now, after) = noted(0x7ff0_2000, 0x7ff0_3000).unwrap();
        assert_eq!(now, other);
        assert!(after >= before);
        // A row with no skull hands the picture back to the game.
        note(0x7ff0_0100, 0x7ff0_2000, Some(0x7ff0_3000), None);
        assert!(noted(0x7ff0_2000, 0x7ff0_3000).is_none());
        note(0x7ff0_0100, 0x7ff0_2000, Some(0x7ff0_3000), Some(look));
        forget(0x7ff0_0104);
        assert!(
            noted(0x7ff0_2000, 0x7ff0_3000).is_some(),
            "another screen freed"
        );
        forget(0x7ff0_0100);
        assert!(noted(0x7ff0_2000, 0x7ff0_3000).is_none());
    }

    #[test]
    fn the_picture_box_maps_to_the_targets_pixels() {
        let box_ = crate::skulls_menu::PICTURE_BOX;
        assert_eq!(guest_rect(box_, (640, 480)), [359.0, 63.0, 220.0, 207.0]);
        // A wider target keeps the UI's height and centres it.
        assert_eq!(guest_rect(box_, (1280, 960)), [718.0, 126.0, 440.0, 414.0]);
        assert_eq!(guest_rect(box_, (854, 480)), [466.0, 63.0, 220.0, 207.0]);
    }

    #[test]
    fn the_alpha_is_held_to_its_range() {
        assert_eq!(alpha(0.5f32.to_bits()), 0.5);
        assert_eq!(alpha(2.0f32.to_bits()), 1.0);
        assert_eq!(alpha((-1.0f32).to_bits()), 0.0);
        assert_eq!(alpha(f32::NAN.to_bits()), 1.0);
    }
}
