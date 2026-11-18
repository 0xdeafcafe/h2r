//! The loaded map's name for the host-side panel (`h2 play`'s watermark).
//!
//! Each game start (the main menu is one too) publishes the cache header's
//! name to `kernel::level`, so the panel shows the map the game has loaded
//! rather than the last `.map` file it opened. One read per map load; the
//! panel's draw only reads the published slot.

use feature::{BuildError, Builder, Feature, Manifest};

/// The game-start observer that publishes the loaded map's name.
pub struct LoadedMap;

impl Feature for LoadedMap {
    fn manifest(&self) -> Manifest {
        Manifest::new("loaded_map", "Publishes the loaded map's name to the host panel")
            .doc("docs/runtime/level-boot.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // A read-only observer on the game-start point, like `hud`'s and
        // `health`'s; ordered after `hud`'s.
        b.game()
            .on_start("publish the loaded map", |o| {
                if let Ok(name) = crate::health::loaded_map_name(&*o.guest()) {
                    kernel::level::publish(&name);
                }
            })
            .after_feature("hud");
        Ok(())
    }
}
