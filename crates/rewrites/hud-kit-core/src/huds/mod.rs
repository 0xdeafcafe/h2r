//! The HUDs the kit knows: the preview's (`h2 hud preview`, `h2 hud
//! sheet`) and the checks' list, and what the game's runtime compiles.

use crate::model::Describe;

pub mod superbounce;
pub mod superbounce_readout;

/// Every registered HUD.
pub fn all() -> Vec<Box<dyn Describe>> {
    vec![
        Box::new(superbounce::Superbounce),
        Box::new(superbounce_readout::Readout),
    ]
}

/// The registered HUD named `name`.
pub fn find(name: &str) -> Option<Box<dyn Describe>> {
    all().into_iter().find(|d| d.hud().name == name)
}
