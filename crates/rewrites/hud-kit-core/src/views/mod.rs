//! The HUDs the kit knows: the preview's (`h2 hud preview`, `h2 hud
//! sheet`) and the checks' list, and what the game's runtime compiles.

use crate::model::Describe;

pub mod firefight;
pub mod forge;
pub mod forge_all;
pub mod forge_readout;
pub mod objective;
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
        Box::new(forge::Forge),
        Box::new(forge_readout::Readout),
        Box::new(firefight::Firefight),
        Box::new(objective::Base),
        Box::new(objective::Ctf),
        Box::new(objective::Assault),
        Box::new(objective::Oddball),
        Box::new(objective::Koth),
        Box::new(objective::Territories),
    ]
}

/// The registered HUD named `name`.
pub fn find(name: &str) -> Option<Box<dyn Describe>> {
    all().into_iter().find(|d| d.hud().name == name)
}

/// What `h2 hud preview` draws: the registered HUDs and the preview-only
/// combined ones ([`superbounce_all`], [`forge_all`]).
pub fn previews() -> Vec<Box<dyn Describe>> {
    let mut v = all();
    v.push(Box::new(superbounce_all::All));
    v.push(Box::new(forge_all::All));
    v
}
