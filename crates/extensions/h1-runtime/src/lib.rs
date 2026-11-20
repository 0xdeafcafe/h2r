//! Hooks that only converted Halo 1 maps need (docs/halo1/README.md):
//! projectile velocity curves and the scope zoom. The rest of Halo 1 in
//! the game (AI control, conversations, HUD text, script builtins,
//! cinematics) is `game`'s `h1_*` modules.

pub mod h1_projectiles;
pub mod h1_scope_zoom;
