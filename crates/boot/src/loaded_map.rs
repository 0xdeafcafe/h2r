//! The loaded map's name for the host-side panel (`h2 play`'s watermark).
//!
//! Each game start (the main menu is one too) publishes the cache header's
//! name to `kernel::level`, so the panel shows the map the game has loaded
//! rather than the last `.map` file it opened. One read per map load; the
//! panel's draw only reads the published slot.

use engine::memory::GuestMemory;
use feature::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

/// Publishes the name in the loaded cache header, if it can be read.
pub fn republish(m: &dyn GuestMemory) {
    if let Ok(name) = crate::health::loaded_map_name(m) {
        kernel::level::publish(&name);
    }
}

/// The game-start observer that publishes the loaded map's name.
pub struct LoadedMap;

impl Feature for LoadedMap {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "loaded_map",
            "Publishes the loaded map's name to the host panel",
        )
        .doc("docs/runtime/level-boot.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // A read-only observer on the game-start point, like `hud`'s and
        // `health`'s; ordered after `hud`'s.
        b.game()
            .on_start("publish the loaded map", |o| republish(&*o.guest()))
            .after_feature("hud");
        // Nothing saved: a load publishes the loaded map's name again, so
        // the panel shows the snapshot's map (docs/runtime/snapshots.md).
        b.snapshot(SnapshotPolicy::section(Vec::new, |_, rt| {
            republish(&*rt.memory());
            Ok(())
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::CACHE_MAP_NAME;
    use engine::testing::FakeGuest;

    #[test]
    fn a_load_publishes_the_loaded_maps_name() {
        let mut g = FakeGuest::new();
        g.map(CACHE_MAP_NAME, 0x20);
        g.put(CACHE_MAP_NAME, b"03a_oldmombasa\0");
        republish(&g);
        assert_eq!(kernel::level::last_map().as_deref(), Some("03a_oldmombasa"));
        // No name to read: the last published one stays.
        republish(&FakeGuest::new());
        assert_eq!(kernel::level::last_map().as_deref(), Some("03a_oldmombasa"));
    }
}
