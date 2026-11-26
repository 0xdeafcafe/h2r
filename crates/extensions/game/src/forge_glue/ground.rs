//! **Build AI ground** and **Test waves** in a Forge session
//! (docs/game/forge-map-variants.md, "AI ground").
//!
//! The session asks for a build ([`forge::Session::build_ground`]) and the
//! app supplies the builder ([`install`]): the Firefight kit's ground over
//! the materialised play form, run off the game thread. The builder hands
//! the map it made to [`super::reload::keep_ground_copy`], and the driver
//! reloads the running map onto it, the session going on over it with its
//! document, the monitor's place and the tool (`super::reload`). This build
//! holds no generator, so with none installed both rows say so and change
//! nothing. Nothing here runs while placing.
//!
//! The build runs on a worker, as a file job of the editor
//! (`editing::Editor::start_ground`): it takes a copy of the session, builds
//! on the copy with the installed builder, and the editor takes the copy back
//! when the worker is done. Edits wait meanwhile, so the copy is the session
//! as it was. A builder holds this module's lock for the whole build, so the
//! game thread only ever asks [`available`], which does not wait for it.
use std::sync::{Mutex, TryLockError};

use forge::{GroundBuilder, GroundReport, Session};

/// What a row says when no builder is installed.
pub const NO_BUILDER: &str = "This game cannot build AI ground.";

/// Tests that install the process-wide builder run one at a time.
#[cfg(test)]
pub(super) static TEST_GUARD: Mutex<()> = Mutex::new(());

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

/// Whether a builder is installed. A build in progress holds the lock, and
/// the builder is then there, so this never waits for one.
pub fn available() -> bool {
    match BUILDER.try_lock() {
        Ok(guard) => guard.is_some(),
        Err(TryLockError::WouldBlock) => true,
        Err(TryLockError::Poisoned(e)) => e.into_inner().is_some(),
    }
}

fn with<T>(f: impl FnOnce(&mut Builder) -> Result<T, String>) -> Result<T, String> {
    let mut guard = BUILDER.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.as_mut().ok_or(NO_BUILDER)?)
}

/// **Build AI ground**: build now, current or not.
pub(super) fn build(session: &mut Session) -> Result<(), String> {
    with(|b| session.build_ground(&mut **b).map(|_| ()))
}

/// **Test waves**: build when the ground is stale or never built, or, in a
/// game that reloads the map, when the running map does not have it.
/// Whether it built.
pub(super) fn test_waves(session: &mut Session) -> Result<bool, String> {
    with(|b| {
        if super::reload::reloads() && !session.ground_loaded() {
            return session.build_ground(&mut **b).map(|_| true);
        }
        session.ensure_ground(&mut **b)
    })
}

/// The build a worker runs on its copy of the session: **Test waves** when
/// `if_stale`, else **Build AI ground**. Whether it built.
pub(super) fn run(session: &mut Session, if_stale: bool) -> Result<bool, String> {
    if if_stale {
        test_waves(session)
    } else {
        build(session).map(|()| true)
    }
}

/// The sentence the HUD says when a build is done, from the session the
/// worker gave back.
pub(super) fn notice(report: Option<&GroundReport>, built: bool) -> String {
    let Some(report) = report.filter(|_| built) else {
        return "AI ground is current.".into();
    };
    let problems = match report.problems.len() {
        0 => String::new(),
        1 => ", 1 problem".into(),
        n => format!(", {n} problems"),
    };
    let tail = if super::reload::reloads() {
        " Loading the map with it..."
    } else {
        " The running map is not changed."
    };
    format!(
        "AI ground built: {} zones, {} squad slots{problems}.{tail}",
        report.zones, report.squads
    )
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
            waves: None,
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

    // The builder is process-wide: see TEST_GUARD.
    #[test]
    fn the_rows_refuse_with_no_builder_and_build_with_one() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
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
        assert!(test_waves(&mut s).unwrap());
        assert!(!test_waves(&mut s).unwrap());
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Build AI ground builds again, current or not.
        build(&mut s).unwrap();
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(s.ground_state(), GroundState::Current);
        uninstall();
    }

    #[test]
    fn the_notice_says_what_was_built_and_whether_the_map_changed() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let report = GroundReport {
            zones: 3,
            squads: 16,
            problems: vec![],
            receipt: None,
        };
        crate::forge_glue::reload::set_reloads(false);
        assert_eq!(
            notice(Some(&report), true),
            "AI ground built: 3 zones, 16 squad slots. The running map is not changed."
        );
        let problem = forge::GroundProblem {
            label: "ff_drop".into(),
            position: [0.; 3],
            reason: "off the walkable area".into(),
        };
        let two = GroundReport {
            problems: vec![problem.clone(), problem.clone()],
            ..report.clone()
        };
        crate::forge_glue::reload::set_reloads(true);
        assert_eq!(
            notice(Some(&two), true),
            "AI ground built: 3 zones, 16 squad slots, 2 problems. Loading the map with it..."
        );
        let one = GroundReport {
            problems: vec![problem],
            ..report
        };
        assert!(notice(Some(&one), true).contains(", 1 problem."));
        // A build that was not needed says the ground stands.
        assert_eq!(notice(Some(&one), false), "AI ground is current.");
        assert_eq!(notice(None, false), "AI ground is current.");
        crate::forge_glue::reload::set_reloads(false);
    }

    #[test]
    fn asking_whether_a_builder_is_installed_does_not_wait_for_a_build() {
        struct Slow(std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>);

        impl GroundBuilder for Slow {
            fn recipe(&self) -> GroundRecipe {
                GroundRecipe {
                    kit_version: 1,
                    home: 0,
                    probe: false,
                }
            }

            fn build(&mut self, _: &ForgeDoc) -> Result<GroundReport, String> {
                self.1.send(()).unwrap();
                self.0.recv().unwrap();
                Ok(GroundReport::default())
            }
        }
        // The builder is process-wide, as in the test above, so these two
        // tests must not interleave.
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let (release, wait) = std::sync::mpsc::channel();
        let (started, running) = std::sync::mpsc::channel();
        install(Box::new(Slow(wait, started)));
        let mut s = session();
        let worker = std::thread::spawn(move || {
            let built = run(&mut s, false);
            (built, s)
        });
        running.recv().unwrap();
        assert!(available(), "a build in progress still has its builder");
        release.send(()).unwrap();
        let (built, s) = worker.join().unwrap();
        assert_eq!(built, Ok(true));
        assert_eq!(s.ground_state(), GroundState::Current);
        uninstall();
    }
}
