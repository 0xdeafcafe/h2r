//! **Build AI ground** and **Test waves** in a Forge session
//! (docs/game/forge-map-variants.md, "AI ground").
//!
//! The session asks for a build ([`forge::Session::build_ground`]) and the
//! app supplies the builder ([`install`]): the Firefight kit's ground over
//! the materialised play form, run off the game thread, and the map
//! reloaded with the document, the monitor's place and the tool kept. This
//! build holds no generator, so with none installed both rows say so and
//! change nothing. Nothing here runs while placing.
use std::sync::Mutex;

use forge::{GroundBuilder, Session};

const NO_BUILDER: &str = "This game cannot build AI ground.";

type Builder = Box<dyn GroundBuilder + Send>;

static BUILDER: Mutex<Option<Builder>> = Mutex::new(None);

/// Supply the builder the rows use, replacing any.
pub fn install(builder: Builder) {
    *BUILDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(builder);
}

/// The builder is gone: the rows refuse again.
pub fn uninstall() {
    *BUILDER.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Whether a builder is installed.
pub fn available() -> bool {
    BUILDER.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

fn with<T>(f: impl FnOnce(&mut Builder) -> Result<T, String>) -> Result<T, String> {
    let mut guard = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.as_mut().ok_or(NO_BUILDER)?)
}

/// **Build AI ground**: build now, current or not.
pub(super) fn build(session: &mut Session) -> Result<(), String> {
    with(|b| session.build_ground(&mut **b).map(|_| ()))
}

/// **Test waves**: build when the ground is stale or never built.
pub(super) fn test_waves(session: &mut Session) -> Result<(), String> {
    with(|b| session.ensure_ground(&mut **b).map(|_| ()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge::{GroundRecipe, GroundReport, GroundState};
    use forge_doc::ForgeDoc;

    struct Counting(std::sync::Arc<std::sync::atomic::AtomicUsize>);

    impl GroundBuilder for Counting {
        fn recipe(&self) -> GroundRecipe {
            GroundRecipe {
                kit_version: 1,
                home: 0,
                probe: false,
            }
        }

        fn build(&mut self, _: &ForgeDoc) -> Result<GroundReport, String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(GroundReport::default())
        }
    }

    fn session() -> Session {
        use forge_doc::{BuilderVersions, Limits, MapRef, Sha256, WorldBounds};
        let base = ForgeDoc {
            format: forge_doc::FORMAT,
            builder: BuilderVersions::CURRENT,
            base: MapRef {
                map: "court".into(),
                sha256: Sha256::of(b"an invented court"),
            },
            sources: vec![],
            title: "Waves".into(),
            author: "Builder".into(),
            description: String::new(),
            recipes: vec![],
            imports: vec![],
            ops: vec![],
            expect: None,
        };
        let limits = Limits {
            base_counts: std::collections::BTreeMap::from([(forge::BlockKind::NetgameFlags, 3)]),
            world: Some(WorldBounds {
                min: [-30.; 3],
                max: [30.; 3],
            }),
            ..Default::default()
        };
        Session::new(base, limits, vec![], 4).unwrap()
    }

    // One test: the builder is process-wide.
    #[test]
    fn the_rows_refuse_with_no_builder_and_build_with_one() {
        uninstall();
        let mut s = session();
        assert_eq!(build(&mut s), Err(NO_BUILDER.to_string()));
        assert_eq!(test_waves(&mut s), Err(NO_BUILDER.to_string()));
        assert!(!available());
        assert_eq!(s.ground_state(), GroundState::NotBuilt);

        let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        install(Box::new(Counting(count.clone())));
        assert!(available());
        // Test waves builds a ground that was never built, once.
        test_waves(&mut s).unwrap();
        test_waves(&mut s).unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Build AI ground builds again, current or not.
        build(&mut s).unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(s.ground_state(), GroundState::Current);
        uninstall();
    }
}
