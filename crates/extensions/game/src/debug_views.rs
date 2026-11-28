//! Debug-menu controls and their world-space geometry share one state per window.
use agent::debug_views::{self as views, DebugMaps, DebugViews};
use std::sync::Arc;

/// The socket and the debug menu both dispatch through these methods, including
/// when a host cannot display world lines.
pub fn install(
    agent: &agent::Agent,
    controls: Option<Arc<DebugViews>>,
    unavailable: Option<&'static str>,
) {
    #[cfg(feature = "gpu")]
    let maps = controls
        .as_ref()
        .filter(|_| unavailable.is_none())
        .map(|_| Arc::new(gpu::MountedMaps) as Arc<dyn DebugMaps>);
    #[cfg(not(feature = "gpu"))]
    let maps: Option<Arc<dyn DebugMaps>> = None;
    views::install(agent, controls, maps, unavailable);
}

#[cfg(feature = "gpu")]
pub use gpu::register;

#[cfg(feature = "gpu")]
mod gpu {
    use super::*;
    use crate::world_lines::{ImageMarks, WorldLine, WorldLineSource};
    use guest_layout::GuestMemory;
    use views::{DebugBsp, DebugHitboxes, DebugImage};

    /// Called by the window host before boot installs the shared scene observer.
    pub fn register(host: &mut crate::Host) {
        if host.debug_views.is_none() {
            let controls = Arc::new(DebugViews::default());
            host.add_world_line_source(controls.clone());
            host.debug_views = Some(controls);
        }
    }

    impl WorldLineSource for DebugViews {
        fn active(&self) -> bool {
            DebugViews::active(self)
        }

        fn capture(&self, guest: &dyn GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            Some(Arc::new(Sample(DebugViews::capture(self, guest)?)))
        }
    }

    #[derive(Debug)]
    struct Sample(DebugImage);

    impl ImageMarks for Sample {
        fn lines(&self, visitor: &mut dyn FnMut(WorldLine<'_>)) {
            self.0.lines(&mut |edges, color, dashed| {
                if let Some(first) = edges.first() {
                    visitor(WorldLine {
                        edges,
                        color,
                        dashed,
                        anchor: first[0],
                        label: None,
                        style: Default::default(),
                    });
                }
            });
        }
    }

    pub(super) struct MountedMaps;

    impl MountedMaps {
        fn open(
            name: &str,
            checksum: Option<u32>,
        ) -> Result<(crate::native_map::NativeMap, assets::ResourceFiles), String> {
            if name.is_empty()
                || name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            {
                return Err("invalid cache short name".into());
            }
            let (map, files) = crate::native_map::open(name).map_err(|e| e.to_string())?;
            if !map.header().name.eq_ignore_ascii_case(name)
                || checksum.is_some_and(|c| c != map.header().checksum)
            {
                return Err("mounted file does not match the loaded cache header".into());
            }
            Ok((map, files))
        }
    }

    impl DebugMaps for MountedMaps {
        fn generation(&self) -> u64 {
            render_bridge::native_map_work::generation()
        }

        fn bsp(&self, name: &str, checksum: Option<u32>, index: usize) -> Result<DebugBsp, String> {
            let (mut map, files) = Self::open(name, checksum)?;
            DebugBsp::build(&mut map, &mut crate::native_world_glue::Raw(files), index)
                .map_err(|e| e.to_string())
        }

        fn hitboxes(&self, name: &str, checksum: Option<u32>) -> Result<DebugHitboxes, String> {
            let (mut map, _) = Self::open(name, checksum)?;
            DebugHitboxes::read(&mut map).map_err(|e| e.to_string())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn window_registers_its_controls_as_one_inactive_source() {
            let mut host = crate::Host::default();
            register(&mut host);
            register(&mut host);
            assert_eq!(host.world_line_sources.len(), 1);
            let rpc: Arc<dyn WorldLineSource> = host.debug_views.as_ref().unwrap().clone();
            assert!(Arc::ptr_eq(&rpc, &host.world_line_sources[0]));
            assert!(!host.world_line_sources[0].active());
        }
    }
}
