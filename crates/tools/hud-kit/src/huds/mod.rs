//! The HUDs the kit knows, for `h2 hud preview` and `h2 hud sheet` and
//! the checks. Each moves beside its feature when its runtime lands
//! (docs/game/hud-kit.md, "Build plan").

use crate::model::Describe;

pub mod superbounce;

/// Every registered HUD.
pub fn all() -> Vec<Box<dyn Describe>> {
    vec![Box::new(superbounce::Superbounce)]
}

/// The registered HUD named `name`.
pub fn find(name: &str) -> Option<Box<dyn Describe>> {
    all().into_iter().find(|d| d.hud().name == name)
}
