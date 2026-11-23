//! The HUDs the kit knows: the preview's (`h2 hud preview`, `h2 hud
//! sheet`) and the checks' list, and what the game's runtime compiles.

use crate::model::Describe;

pub mod forge_readout;
pub mod pill_block;
pub mod superbounce;
pub mod superbounce_all;
pub mod superbounce_guide;
pub mod superbounce_segments;
pub mod superbounce_readout;

/// Every registered HUD.
pub fn all() -> Vec<Box<dyn Describe>> {
    vec![
        Box::new(superbounce::Superbounce),
        Box::new(superbounce_readout::Readout),
        Box::new(forge_readout::Readout),
    ]
}

/// The registered HUD named `name`.
pub fn find(name: &str) -> Option<Box<dyn Describe>> {
    all().into_iter().find(|d| d.hud().name == name)
}

/// What `h2 hud preview` draws: the registered HUDs and the preview-only
/// combined ones ([`superbounce_all`]).
pub fn previews() -> Vec<Box<dyn Describe>> {
    let mut v = all();
    v.push(Box::new(superbounce_all::All));
    v
}
