//! The loaded map's name for the host-side panel (`h2 play`'s watermark).
//!
//! Each game start (the main menu is one too) publishes the cache header's
//! name to `kernel::level`, so the panel shows the map the game has loaded
//! rather than the last `.map` file it opened. One read per map load; the
//! panel's draw only reads the published slot.

use std::sync::{Arc, Mutex, Weak};

use guest_layout::memory::GuestMemory;
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

/// Publishes the name in the loaded cache header, if it can be read: to
/// the panel, and to the D3D pipeline cache, which records and prewarms
/// programs by map (docs/graphics/graphics.md, "Pipeline compiles
/// (`compile.rs`) and the pipeline cache").
pub fn republish(m: &dyn GuestMemory) {
    if let Ok(name) = crate::health::loaded_map_name(m) {
        kernel::level::publish(&name);
        #[cfg(feature = "gpu")]
        gfx::pipeline_store::set_map(&name);
    }
    publish_file(m);
}

/// Publishes the file the loaded map's slot was copied from: every native
/// reader opens that file rather than a path guessed from the name.
fn publish_file(m: &dyn GuestMemory) {
    #[cfg(feature = "gpu")]
    if let Ok(name) = crate::health::loaded_map_name(m) {
        crate::native_map::set_loaded_slot(
            &name,
            crate::map_slots::active_file(&crate::layout_glue::ReadGm(m)),
        );
    }
    #[cfg(not(feature = "gpu"))]
    let _ = m;
}

type MapChange = Box<dyn Fn(Option<&vfs::HostFile>) + Send + Sync>;

/// Derived map data stays with the activated file. Publishing a DVD source
/// alone cannot change which skulls, symbols or navigation the guest uses.
pub struct EditorSources {
    mount: Weak<Mutex<vfs::Vfs>>,
    binding: vfs::MapRebind,
    last: Mutex<Option<std::path::PathBuf>>,
    observers: Mutex<Vec<MapChange>>,
}

impl EditorSources {
    pub fn new(mount: &Arc<Mutex<vfs::Vfs>>, binding: vfs::MapRebind) -> Self {
        Self {
            mount: Arc::downgrade(mount),
            binding,
            last: Mutex::default(),
            observers: Mutex::default(),
        }
    }

    pub fn on_change(&self, f: impl Fn(Option<&vfs::HostFile>) + Send + Sync + 'static) {
        self.observers.lock().unwrap_or_else(|e| e.into_inner()).push(Box::new(f));
    }

    pub fn activated(&self, guest: &dyn GuestMemory) {
        // The areas the map was saved with, before the rules runtime's
        // start reads the census (`crate::map_areas`).
        if let Some(vfs) = self.mount.upgrade() {
            let file = crate::map_slots::active_file(&crate::layout_glue::ReadGm(guest));
            crate::map_areas::activate(&vfs, file.as_deref());
            // Its play forms are read again with the record in hand.
            crate::forge_glue::markers_world::play_stale();
            // The map's tag paths, for a script's tag reads.
            #[cfg(feature = "rules")]
            crate::rules::world::map_tags::activate(&vfs, file.as_deref());
        }
        if !self.binding.has_ever_bound() {
            return;
        }
        let path = crate::map_slots::active_file(&crate::layout_glue::ReadGm(guest));
        let file = path.as_deref()
            .and_then(|path| self.mount.upgrade()?.lock().ok()?.host_file(path))
            .filter(|file| file.generation.is_some());
        #[cfg(feature = "gpu")]
        if let Ok(name) = crate::health::loaded_map_name(guest) {
            crate::native_map::set_active_source(&name, path, file.clone());
        }
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        if last.as_ref() == file.as_ref().map(|file| &file.path) {
            return;
        }
        *last = file.as_ref().map(|file| file.path.clone());
        drop(last);
        // GAME_ACTIVATE already ran. Retry native jobs that stood down while
        // the DVD had the replacement but this full start had not returned.
        #[cfg(feature = "gpu")]
        render_bridge::native_map_work::reset();
        for notify in self.observers.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            notify(file.as_ref());
        }
    }
}

/// The feature's name, for features that open the map at game start.
pub const FEATURE: &str = "loaded_map";

/// The game-start observer that publishes the loaded map's name.
pub struct LoadedMap;

impl Feature for LoadedMap {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            FEATURE,
            "Publishes the loaded map's name to the host panel",
        )
        .doc("docs/runtime/level-boot.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // The recorder samples these existing host-side publications. A game
        // without an agent tick observer still publishes its loaded level.
        mem_watch::register_game_context(|| {
            (kernel::level::last_map(), agent::render::HOSTED.last_tick())
        });
        // A read-only observer on the game-start point, like `hud`'s and
        // `health`'s; ordered after `hud`'s.
        // The map's file is published before the start too: the HUD kit
        // opens the map ahead of the activation's return.
        b.raw()
            .before(&hooks::points::GAME_ACTIVATE, "publish the loaded map's file", |c| {
                let rt = std::sync::Arc::clone(c.ctx.runtime());
                publish_file(&*rt.memory());
                Ok(hooks::Flow::Continue)
            })
            // Every reader at the start (the HUD kit, Firefight, the H1
            // swaps) may open the map, so the path is published in the
            // first class, after the class's other members (which only
            // reset state and never read the path).
            .first()
            .after_feature("data-salts")
            .after_feature("object-pool-clear")
            .after_feature("game-variants")
            .after_feature("h1-scope-zoom");
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
    use guest_layout::testing::FakeGuest;

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
